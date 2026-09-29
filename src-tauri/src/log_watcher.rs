use std::collections::HashMap;
use tracing::{info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, Manager};
use crate::app_state::AppState;
use crate::monitor::{append_to_file, now_hms};
use crate::catalogue::sanitize_chat_item_name;
use crate::relic_pick::{build_relic_pick_payload, relic_pick_show, relic_pick_hide, show_overlay};
use crate::{db, events, log_parser};

// ==============================================================================
// EE.log wake-up source
// ==============================================================================
//
// Both log watchers want to react the instant Warframe flushes a line. There is
// no directory-change notification wired up here, so both poll at a fixed
// interval — a few extra wake-ups per second, but the loops stay simple.
//
// ponytail: polling; switch to inotify if wake-up latency matters.

/// Parses outside the database lock and takes it only for the writes. A write
/// that fails leaves its runs queued in the recorder, so this logs and returns
/// rather than tearing the watcher thread down over a busy database.
///
/// The overlay is raised before the runs are written, deliberately: the
/// summary comes from the log, not the row, and a run whose insert failed is
/// still queued for the next attempt. Waiting on the write would only lose
/// the overlay to a transient database failure.
fn record_arbitration_runs(
    app: &tauri::AppHandle,
    recorder: &mut db::ArbitrationRecorder,
    chunk: String,
    live: bool,
) {
    let ended = recorder.parse(chunk);

    let state = app.state::<AppState>();
    let overlay_on = state.arbitration_overlay_enabled.load(Ordering::SeqCst);
    if let Some(summary) = db::live_run_summary(&ended, live, overlay_on) {
        show_overlay(app, "arbitration-overlay");
        if let Err(e) = app.emit(events::ARBITRATION_RUN_ENDED, &summary) {
            warn!(error = %e, "arbitration overlay event not delivered");
        }
    }

    let stored = {
        let conn = match state.conn.lock() {
            Ok(conn) => conn,
            Err(e) => {
                warn!(error = %e, "database lock poisoned; arbitration runs held back");
                return;
            }
        };
        recorder.store(&conn)
    };
    match stored {
        Ok(0) => {}
        Ok(stored) => {
            info!(runs = stored, "arbitration runs recorded");
            app.emit(events::ARBITRATION_RUNS_CHANGED, ()).ok();
        }
        Err(e) => warn!(error = %e, "storing arbitration runs failed; retrying on the next read"),
    }
}

/// Start a lightweight EE.log watcher for features that don't need the memory scanner:
/// riven reroll detection, trade completion detection, WFM whisper detection.
/// Called unconditionally at app startup — EE.log is plain file I/O, not memory reading.
#[tracing::instrument(level = "debug", skip_all)]
#[tauri::command]
pub(crate) fn start_log_watcher(app: tauri::AppHandle) -> Result<(), String> {
    let log_path =
        log_parser::watched_log_path().ok_or("Cannot find the local data directory")?;

    // The frontend invokes this from an effect, so a reload or React's
    // double-mount asks for the watcher again. A second thread would replay
    // the whole log through arbitration backfill a second time.
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    // A panic in the loop below would otherwise leave the flag set with no
    // thread behind it, and every later invoke would return into nothing.
    // Dropping runs on the unwind too, so the next invoke starts a watcher.
    struct HoldsTheWatcherSlot;
    impl Drop for HoldsTheWatcherSlot {
        fn drop(&mut self) {
            STARTED.store(false, Ordering::SeqCst);
        }
    }

    if !log_path.is_file() {
        warn!(path = %log_path.display(), "EE.log not found; log-driven features stay idle until it appears");
    }

    std::thread::spawn(move || {
        let _slot = HoldsTheWatcherSlot;

        // Arbitration runs finished while FrameForge was closed are still in
        // the log, so the recorder gets the file from the top before the tail
        // below starts. It keeps its parser afterwards, which is also what
        // lets a run already under way at startup end correctly.
        //
        // The other features below deliberately start from the end instead:
        // replaying an old log would fire reward overlays and trade prompts
        // for missions the player finished hours ago.
        let mut arbitration_runs = db::ArbitrationRecorder::default();
        let mut tail = log_parser::LogTail::from_start(log_path.clone());
        if let Some(backfill) = tail.read() {
            record_arbitration_runs(&app, &mut arbitration_runs, backfill.text, false);
        }

        let mut trade_buffer = String::new();
        // Cooldown: don't fire riven-screen-open again within 4 seconds of the last fire.
        // Guards against the same EE.log buffer being processed twice by React StrictMode listeners.
        let mut last_riven_fire: Option<std::time::Instant> = None;
        // Cooldown: prevent spawning multiple OCR threads if the trigger fires rapidly.
        let mut last_relic_pick_trigger: Option<std::time::Instant> = None;

        loop {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let Some(chunk) = tail.read() else { continue };
            if chunk.restarted {
                // A new launch's log. No run survives it, and a half line held
                // over from the old file would otherwise be glued onto the new
                // file's boot-time header.
                arbitration_runs = db::ArbitrationRecorder::default();
                trade_buffer.clear();
            }
            let buf = chunk.text;
            // A replaced log arrives whole and may hold runs finished long
            // before this read; those are backfill too.
            //
            // ponytail: a stall of this thread while the game keeps writing
            // the same file also hands over old runs, and they count as live.
            // Nothing in practice stalls it for the length of a mission; gate
            // on the run's own wall clock if that changes.
            record_arbitration_runs(&app, &mut arbitration_runs, buf.clone(), !chunk.restarted);
            let lower = buf.to_lowercase();

            // ── Riven reroll / unveil ─────────────────────────────────────────
            let riven_trigger =
                lower.contains("omegarerollselection.swf") ||
                lower.contains("samodeusdioramaloaded");

            let cooldown_ok = last_riven_fire
                .is_none_or(|t| t.elapsed().as_secs() >= 4);

            if riven_trigger && cooldown_ok {
                last_riven_fire = Some(std::time::Instant::now());
                let _ = app.emit(events::RIVEN_SCREEN_OPEN, ());
                let _ = app.emit(events::FF_STATUS, "🎲 Riven screen detected");
            }

            // ── Riven screen close — card UI hidden (primary) ─────────────────
            // DiegeticArtifactCards.lua: DBG: HudVis 0 fires when the mod card
            // overlay is hidden — the most direct signal the riven screen closed.
            // Guard: only fire ≥1 s after the open trigger (so open+close in the
            // same EE.log buffer don't cancel each other out).
            if lower.contains("digeticartifactcards.lua: dbg: hudvis 0") {
                let riven_active = last_riven_fire.is_some_and(|t| {
                    let e = t.elapsed().as_secs();
                    (1..600).contains(&e)
                });
                if riven_active {
                    last_riven_fire = None;
                    let riven_log = app.state::<AppState>().riven_log.clone();
                    let ts = now_hms();
                    let _ = append_to_file(&riven_log, &format!(
                        "[STEP 4] CLOSE (DiegeticArtifactCards HudVis 0) — {}\n\n", ts
                    ));
                    let _ = app.emit(events::RIVEN_SCREEN_CLOSE, ());
                }
            }

            // ── Riven screen close — orbiter scene reload (fallback) ──────────
            // When the player exits the riven screen, the orbiter scene reloads
            // and creates VolumetricFog render targets. Kept as a fallback in case
            // the HudVis 0 trigger is missed.
            if lower.contains("creating render target: /ee/materials/volumetricfog") {
                let riven_active = last_riven_fire.is_some_and(|t| {
                    let e = t.elapsed().as_secs();
                    (3..600).contains(&e)
                });
                if riven_active {
                    last_riven_fire = None;
                    let riven_log = app.state::<AppState>().riven_log.clone();
                    let ts = now_hms();
                    let _ = append_to_file(&riven_log, &format!(
                        "[STEP 4] CLOSE (VolumetricFog render target = orbiter loaded) — {}\n\n", ts
                    ));
                    let _ = app.emit(events::RIVEN_SCREEN_CLOSE, ());
                }
            }

            // ── WFM trade whisper ─────────────────────────────────────────────
            if lower.contains("(warframe.market)") {
                let raw = buf.as_str();
                let from = raw.find("@From ").map(|i| &raw[i+6..])
                    .and_then(|s| s.split(" :").next())
                    .map(|s| s.trim().to_string()).unwrap_or_else(|| "Unknown".to_string());
                let item = { let p="want to buy "; let s=" for ";
                    raw.find(p).and_then(|i| { let r=&raw[i+p.len()..]; r.find(s).map(|j| sanitize_chat_item_name(&r[..j])) })
                };
                let price: Option<u64> = raw.find(" for ").and_then(|i| {
                    let r=&raw[i+5..]; r.find(" platinum").and_then(|j| r[..j].trim().parse().ok())
                });
                let _ = app.emit(events::WFM_WHISPER, serde_json::json!({
                    "from": from, "message": raw.trim(), "item": item, "price": price,
                    "timestamp": chrono::Local::now().format("%H:%M:%S").to_string(),
                }));
            }

            // ── Relic selection screen ───────────────────────────────────────
            // Trigger: relic grid fully loaded → OCR the era from top-left quarter.
            if lower.contains("themedprojectionmanager.lua: populateinventorygrid") {
                info!("relic-pick: PopulateInventoryGrid detected — spawning OCR thread");
                let now = std::time::Instant::now();
                let relic_pick_on = app.state::<AppState>().relic_pick_overlay_enabled.load(Ordering::SeqCst);
                let should_trigger = relic_pick_on && last_relic_pick_trigger
                    .is_none_or(|t| now.duration_since(t).as_secs() >= 5);
                if should_trigger {
                    last_relic_pick_trigger = Some(now);
                    let app_clone = app.clone();
                    std::thread::spawn(move || {
                        // Brief delay for the screen to finish rendering before capture.
                        std::thread::sleep(std::time::Duration::from_millis(400));
                        let era = crate::ocr::detect_fissure_era();
                        info!("relic-pick: OCR result = {:?}", era);
                        if let Some(era) = era {
                            let payload = build_relic_pick_payload(&era, &app_clone);
                            let relic_count = payload["relics"].as_array().map_or(0, |a| a.len());
                            info!("relic-pick: emitting relic-pick-open era={} relics={}", era, relic_count);
                            // Show the overlay window from Rust — more reliable than
                            // calling win.show() from the WebView (avoids timing races).
                            relic_pick_show(&app_clone);
                            let _ = app_clone.emit(events::RELIC_PICK_OPEN, payload);
                        }
                    });
                } else {
                    info!("relic-pick: trigger suppressed by 5-second cooldown");
                }
            }
            // Dismiss: solar map regains input focus (player cancelled or mission started).
            let mapredux_dismiss = lower.contains("subscribing for /lotus/interface/mapredux.swf")
                && lower.contains("mapreduxinputfilter");
            // Candidate: entitlement service completing signals the refinement screen closed.
            let entitlement_dismiss = lower.contains("onentitlementservicecomplete false:");
            if mapredux_dismiss || entitlement_dismiss {
                let which = if entitlement_dismiss { "OnEntitlementServiceComplete" } else { "mapredux" };
                info!("relic-pick: dismiss fired ({})", which);
                relic_pick_hide(&app);
                let _ = app.emit(events::RELIC_PICK_CLOSE, ());
            }

            handle_trade_completion(&app, &buf, &mut trade_buffer);
        }
    });
    Ok(())
}

// ── Trade dialog parser ───────────────────────────────────────────────────────

struct ParsedTrade {
    with_player: String,
    trade_type: String,
    offered_items: Vec<(String, i64)>,
    offered_plat: i64,
    received_items: Vec<(String, i64)>,
    received_plat: i64,
    session_id: String,
    timestamp: String,
}

const MAX_TRADE_BUFFER_BYTES: usize = 256 * 1024;
const TRADE_SUCCESS_MARKER: &str = "the trade was successful";

/// Clean a single item line from a trade dialog:
/// strips Warframe PUA rank-dot characters and normalises mod rank suffixes.
fn clean_trade_item(raw: &str) -> String {
    let raw = raw.trim();
    let filled = raw.chars().filter(|&c| c == '\u{E114}').count();
    let total  = raw.chars().filter(|&c| c == '\u{E114}' || c == '\u{E112}').count();
    if total > 0 {
        let base: String = raw.chars().take_while(|&c| c != '\u{E114}' && c != '\u{E112}').collect();
        let base = base.trim();
        return if filled == 0 { format!("{} (R0)", base) } else { format!("{} (R{})", base, filled) };
    }
    if let Some(p) = raw.find(" (") {
        let inside = &raw[p + 2..];
        if let Some(r) = inside.to_lowercase().find("rank ") {
            let rank_n = inside[r + 5..].trim_end_matches(')').trim();
            return format!("{} (R{})", &raw[..p], rank_n);
        }
        return raw[..p].trim().to_string();
    }
    raw.to_string()
}

/// Parse all items from one section of a trade dialog (offered or received).
/// Handles both repeated-line stacking and "Item x N" inline quantities.
fn extract_trade_items(section: &str) -> Vec<(String, i64)> {
    let mut order: Vec<String> = Vec::new();
    let mut counts: HashMap<String, i64> = HashMap::new();
    for line in section.lines() {
        let raw = line.trim();
        if raw.is_empty() || raw.to_lowercase().contains("platinum") { continue; }
        let (raw_name, qty) = if let Some(x_pos) = raw.rfind(" x ") {
            let qty_part = raw[x_pos + 3..].trim();
            if let Ok(n) = qty_part.parse::<i64>() { (&raw[..x_pos], n) } else { (raw, 1i64) }
        } else {
            (raw, 1i64)
        };
        let name = clean_trade_item(raw_name);
        if !name.is_empty() {
            if !counts.contains_key(&name) { order.push(name.clone()); }
            *counts.entry(name).or_insert(0) += qty;
        }
    }
    order.into_iter().map(|k| { let q = counts[&k]; (k, q) }).collect()
}

/// Parse the full trade confirmation dialog from EE.log.
/// Returns None if the dialog doesn't contain the expected markers.
fn parse_trade_dialog(raw: &str) -> Option<ParsedTrade> {
    // Player names in this dialog are sometimes suffixed by a private-use-area
    // glyph (e.g. U+E000, an in-game rank/status icon) with no preceding space —
    // strip trailing PUA codepoints so `with_player` doesn't carry it along.
    let with_player = raw.find("will receive from ")
        .and_then(|i| { let a = &raw[i + 18..]; a.find(" the following").map(|j| {
            a[..j].trim().trim_end_matches(|c: char| ('\u{E000}'..='\u{F8FF}').contains(&c)).trim().to_string()
        }) })?;
    let offered_raw = raw.find("You are offering:")
        .and_then(|i| { let a = &raw[i + 17..]; a.find("and will receive from").map(|j| a[..j].trim().to_string()) })
        .unwrap_or_default();
    let received_raw = raw.find("the following:")
        .and_then(|i| { let a = &raw[i + 14..]; a.find(", title=").map(|j| a[..j].trim().to_string()) })
        .unwrap_or_default();

    let parse_plat = |s: &str| -> i64 {
        s.find("Platinum x ")
            .and_then(|i| s[i + 11..].split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    };

    let offered_plat  = parse_plat(&offered_raw);
    let received_plat = parse_plat(&received_raw);
    let offered_items  = extract_trade_items(&offered_raw);
    let received_items = extract_trade_items(&received_raw);

    if offered_items.is_empty() && received_items.is_empty() && offered_plat == 0 && received_plat == 0 {
        return None;
    }

    let trade_type = if offered_plat > 0 { "purchase" } else if received_plat > 0 { "sale" } else { "trade" };
    let now = chrono::Utc::now();

    Some(ParsedTrade {
        with_player,
        trade_type: trade_type.to_string(),
        offered_items,
        offered_plat,
        received_items,
        received_plat,
        session_id: now.format("%Y%m%dT%H%M%S%3f").to_string(),
        timestamp: now.to_rfc3339(),
    })
}

/// Keep enough raw EE.log text to reconstruct a trade dialog when Windows wakes
/// the tailer while Warframe is still writing the multi-line log entry.
fn collect_trade_completion(
    buf: &str,
    trade_buffer: &mut String,
) -> (bool, Option<ParsedTrade>) {
    trade_buffer.push_str(buf);
    if trade_buffer.len() > MAX_TRADE_BUFFER_BYTES {
        let mut start = trade_buffer.len() - MAX_TRADE_BUFFER_BYTES;
        while !trade_buffer.is_char_boundary(start) {
            start += 1;
        }
        trade_buffer.drain(..start);
    }

    let mut scan_start = trade_buffer
        .len()
        .saturating_sub(buf.len() + TRADE_SUCCESS_MARKER.len());
    while !trade_buffer.is_char_boundary(scan_start) {
        scan_start += 1;
    }
    if !trade_buffer[scan_start..]
        .to_ascii_lowercase()
        .contains(TRADE_SUCCESS_MARKER)
    {
        return (false, None);
    }

    let lower = trade_buffer.to_ascii_lowercase();
    let trade = lower
        .rfind("dialog::createokcancel")
        .and_then(|start| parse_trade_dialog(&trade_buffer[start..]));
    trade_buffer.clear();
    (true, trade)
}

/// Detect an in-game trade offer dialog and its completion from EE.log text.
pub(crate) fn handle_trade_completion(
    app: &tauri::AppHandle,
    buf: &str,
    trade_buffer: &mut String,
) {
    let (completed, trade) = collect_trade_completion(buf, trade_buffer);
    if let Some(t) = trade {
        info!(
            with_player = %t.with_player,
            trade_type = %t.trade_type,
            offered_items = t.offered_items.len(),
            received_items = t.received_items.len(),
            "trade completion detected"
        );
        if let Err(error) = app.emit(events::TRADE_COMPLETED, serde_json::json!({
            "sessionId":     t.session_id,
            "withPlayer":    t.with_player,
            "tradeType":     t.trade_type,
            "offeredItems":  t.offered_items.iter().map(|(n, q)| serde_json::json!({"name": n, "qty": q})).collect::<Vec<_>>(),
            "offeredPlat":   t.offered_plat,
            "receivedItems": t.received_items.iter().map(|(n, q)| serde_json::json!({"name": n, "qty": q})).collect::<Vec<_>>(),
            "receivedPlat":  t.received_plat,
            "timestamp":     t.timestamp,
        })) {
            warn!(%error, "failed to emit trade-completed event");
        }
    } else if completed {
        warn!("trade completion detected, but confirmation dialog could not be parsed");
    }
}

#[cfg(test)]
mod trade_dialog_tests {
    use super::*;

    /// Real (player-redacted) dialog text captured from EE.log for a trade where
    /// the local player sold 4 Sevagoth Prime blueprints for 33 platinum. The
    /// player name is followed by a private-use-area glyph (U+E000) with no
    /// preceding space, and item lines are prefixed with a bare '\r'.
    /// Regression coverage for the "trades not detected" report (2026-09-28):
    /// the backend-modularization refactor (eaaaa42) accidentally turned the
    /// single-stage `received_raw` extraction into a two-stage one that
    /// re-searched the already-stripped substring for "the following:" — a
    /// string that, by construction, could never be found there again, so
    /// every trade's received side (items and/or platinum) silently vanished.
    #[test]
    fn trade_sale_for_platinum_extracts_received_plat_and_offered_items() {
        let raw = "1037.335 Script [Info]: Dialog.lua: Dialog::CreateOkCancel(description=Are you sure you want to accept this trade? You are offering:\n\rSevagoth Prime Chassis Blueprint\n\rSevagoth Prime Neuroptics Blueprint\n\rSevagoth Prime Systems Blueprint\n\rSevagoth Prime Blueprint\r\n\r\nand will receive from Winter.Mine\u{E000} the following:\n\rPlatinum x 33, title= leftItem=/Menu/Confirm_Item_Ok, rightItem=/Menu/Confirm_Item_Cancel)\n";

        let parsed = parse_trade_dialog(raw).expect("dialog should parse");
        assert_eq!(parsed.with_player, "Winter.Mine", "trailing PUA glyph should be stripped from the player name");
        assert_eq!(parsed.trade_type, "sale");
        assert_eq!(parsed.received_plat, 33, "platinum received must survive the received_raw extraction");
        assert_eq!(parsed.offered_plat, 0);
        assert!(parsed.received_items.is_empty());
        assert_eq!(
            parsed.offered_items,
            vec![
                ("Sevagoth Prime Chassis Blueprint".to_string(), 1),
                ("Sevagoth Prime Neuroptics Blueprint".to_string(), 1),
                ("Sevagoth Prime Systems Blueprint".to_string(), 1),
                ("Sevagoth Prime Blueprint".to_string(), 1),
            ]
        );
    }

    /// A purchase (offering platinum, receiving an item) must also keep its
    /// received side — this was silently empty under the same bug, which meant
    /// `useOverlays.ts`'s `tradeType === "purchase"` branch had nothing to log.
    #[test]
    fn trade_purchase_extracts_received_items() {
        let raw = "Dialog::CreateOkCancel(description=Are you sure you want to accept this trade? You are offering:\r\nPlatinum x 20\r\n\r\nand will receive from Buyer123 the following:\r\nAyatan Anasa Sculpture, title= leftItem=/Menu/Confirm_Item_Ok, rightItem=/Menu/Confirm_Item_Cancel)";

        let parsed = parse_trade_dialog(raw).expect("dialog should parse");
        assert_eq!(parsed.trade_type, "purchase");
        assert_eq!(parsed.offered_plat, 20);
        assert_eq!(parsed.received_items, vec![("Ayatan Anasa Sculpture".to_string(), 1)]);
    }

    #[test]
    fn trade_completion_reassembles_split_log_reads() {
        let chunks = [
            "unrelated log text\nDialog::CreateOkCan",
            "cel(description=Are you sure? You are off",
            "ering:\r\nSaryn Prime Chassis Blueprint\r\n\r\nand will receive from Buyer123 the follow",
            "ing:\r\nPlatinum x 15, title= leftItem=/Menu/Confirm_Item_Ok)\nThe trade was succ",
            "essful\n",
        ];
        let mut trade_buffer = String::new();
        let mut result = None;

        for chunk in chunks {
            let (_, parsed) = collect_trade_completion(chunk, &mut trade_buffer);
            if parsed.is_some() {
                result = parsed;
            }
        }

        let parsed = result.expect("split dialog and completion should be reconstructed");
        assert_eq!(parsed.with_player, "Buyer123");
        assert_eq!(parsed.trade_type, "sale");
        assert_eq!(parsed.received_plat, 15);
        assert_eq!(parsed.offered_items, vec![("Saryn Prime Chassis Blueprint".to_string(), 1)]);
        assert!(trade_buffer.is_empty());
    }
}
