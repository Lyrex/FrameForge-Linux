use tracing::{info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, Manager};
use crate::app_state::AppState;
use crate::monitor::{append_to_file, now_hms};
use crate::catalogue::sanitize_chat_item_name;
use crate::relic_pick::{build_relic_pick_payload, relic_pick_show, relic_pick_hide, show_overlay};
use crate::trade_log::parse_trade_dialog;
use crate::{db, log_parser};

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
        if let Err(e) = app.emit("arbitration-run-ended", &summary) {
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
            app.emit("arbitration-runs-changed", ()).ok();
        }
        Err(e) => warn!(error = %e, "storing arbitration runs failed; retrying on the next read"),
    }
}

/// Start a lightweight EE.log watcher for features that don't need the memory scanner:
/// riven reroll detection, trade completion detection, WFM whisper detection.
/// Called unconditionally at app startup — EE.log is plain file I/O, not memory reading.
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

        let mut pending_trade: Option<String> = None;
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
                let _ = app.emit("riven-screen-open", ());
                let _ = app.emit("ff-status", "🎲 Riven screen detected");
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
                    let _ = app.emit("riven-screen-close", ());
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
                    let _ = app.emit("riven-screen-close", ());
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
                let _ = app.emit("wfm-whisper", serde_json::json!({
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
                            let _ = app_clone.emit("relic-pick-open", payload);
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
                let _ = app.emit("relic-pick-close", ());
            }

            // ── In-game trade completion ──────────────────────────────────────
            if lower.contains("dialog::createokcancel") && lower.contains("you are offering") {
                pending_trade = Some(buf.clone());
            }
            if lower.contains("the trade was successful") {
                if let Some(ref trade_raw) = pending_trade.clone() {
                    if let Some(t) = parse_trade_dialog(trade_raw) {
                        let _ = app.emit("trade-completed", serde_json::json!({
                            "sessionId":     t.session_id,
                            "withPlayer":    t.with_player,
                            "tradeType":     t.trade_type,
                            "offeredItems":  t.offered_items.iter().map(|(n, q)| serde_json::json!({"name": n, "qty": q})).collect::<Vec<_>>(),
                            "offeredPlat":   t.offered_plat,
                            "receivedItems": t.received_items.iter().map(|(n, q)| serde_json::json!({"name": n, "qty": q})).collect::<Vec<_>>(),
                            "receivedPlat":  t.received_plat,
                            "timestamp":     t.timestamp,
                        }));
                    }
                }
                pending_trade = None;
            }
        }
    });
    Ok(())
}
