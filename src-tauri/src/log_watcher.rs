use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager};
use tracing::{info, warn};

use crate::app_state::AppState;
use crate::diagnostics::{append_to_diag, write_bmp};
use crate::relic_pick::{build_relic_pick_payload, relic_pick_hide, relic_pick_show};
use crate::{append_to_file, ocr, sanitize_chat_item_name, OcrParams};

pub(crate) type RewardOcrResult = (bool, bool, Vec<String>, Vec<f32>, String);

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
    let with_player = raw.find("will receive from ")
        .and_then(|i| { let a = &raw[i + 18..]; a.find(" the following").map(|j| a[..j].trim().to_string()) })?;
    let offered_raw = raw.find("You are offering:")
        .and_then(|i| { let a = &raw[i + 17..]; a.find("and will receive from").map(|j| a[..j].trim().to_string()) })
        .unwrap_or_default();
    let received_raw = raw.find("will receive from ")
        .and_then(|i| { let a = &raw[i + 18..]; a.find(" the following:").map(|j| a[j + 15..].trim().to_string()) })
        .unwrap_or_default();

    let received_raw = received_raw.find("the following:")
        .and_then(|i| { let a = &received_raw[i + 14..]; a.find(", title=").map(|j| a[..j].trim().to_string()) })
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

/// Start a lightweight EE.log watcher for features that don't need the memory scanner:
/// riven reroll detection, trade completion detection, WFM whisper detection.
/// Called unconditionally at app startup — EE.log is plain file I/O, not memory reading.
#[tauri::command]
pub(crate) fn start_log_watcher(app: tauri::AppHandle) -> Result<(), String> {
    let log_path = dirs::data_local_dir()
        .map(|d| d.join("Warframe").join("EE.log"))
        .ok_or("Cannot find LocalAppData")?;

    std::thread::spawn(move || {
        use std::io::{Read, Seek, SeekFrom};
        let mut file_pos: u64 = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
        let mut pending_trade: Option<String> = None;
        // Cooldown: don't fire riven-screen-open again within 4 seconds of the last fire.
        // Guards against the same EE.log buffer being processed twice by React StrictMode listeners.
        let mut last_riven_fire: Option<std::time::Instant> = None;
        // Cooldown: prevent spawning multiple OCR threads if the trigger fires rapidly.
        let mut last_relic_pick_trigger: Option<std::time::Instant> = None;

        // Use FindFirstChangeNotificationW so we wake up the instant EE.log is written,
        // instead of sleeping and polling. This is how Overwolf achieves low latency.
        let change_handle: isize = {
            use windows_sys::Win32::Storage::FileSystem::{
                FindFirstChangeNotificationW, FILE_NOTIFY_CHANGE_LAST_WRITE,
            };
            let dir = log_path.parent().unwrap_or(std::path::Path::new("."));
            let dir_wide: Vec<u16> = dir.to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
            unsafe { FindFirstChangeNotificationW(dir_wide.as_ptr(), 0, FILE_NOTIFY_CHANGE_LAST_WRITE) }
        };
        let use_notify = change_handle != -1; // -1 = INVALID_HANDLE_VALUE

        loop {
            if use_notify {
                use windows_sys::Win32::System::Threading::WaitForSingleObject;
                use windows_sys::Win32::Storage::FileSystem::FindNextChangeNotification;
                // Block until EE.log directory has a write — then process immediately
                unsafe { WaitForSingleObject(change_handle, 500); } // 500ms safety timeout
                unsafe { FindNextChangeNotification(change_handle); }
            } else {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let Ok(mut f) = std::fs::File::open(&log_path) else { continue };
            let len = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
            if len < file_pos { file_pos = 0; }
            if len == file_pos { continue; } // nothing new since last read
            if f.seek(SeekFrom::Start(file_pos)).is_err() { continue; }
            let mut buf = String::new();
            if f.read_to_string(&mut buf).is_err() { continue; }
            file_pos = len;
            if buf.is_empty() { continue; }
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
                    let riven_log = std::env::temp_dir().join("frameforge_riven_session.txt");
                    let ts = chrono::Local::now().format("%H:%M:%S%.3f").to_string();
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
                    let riven_log = std::env::temp_dir().join("frameforge_riven_session.txt");
                    let ts = chrono::Local::now().format("%H:%M:%S%.3f").to_string();
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

/// Extract the local player name from EE.log lines containing "Logged in NAME".
/// Adds the name to shared_squad_names (for OCR filtering) and AppState.local_player_name
/// (for UI display). Safe to call with a single line or the full log contents.
pub(crate) fn parse_logged_in_name(
    text: &str,
    squad_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    app: &tauri::AppHandle,
) {
    // Target: "Sys [Info]: Logged in Sikewyrm"
    // The account-login line has exactly ONE token after "Logged in" and nothing more.
    // Lines like "Logged in to region server" have multiple tokens — skip them.
    // Match "]: Logged in " so we don't trigger on unrelated "Logged in …" phrases.
    const MARKER: &str = "]: Logged in ";
    for line in text.lines().rev() {
        let Some(pos) = line.find(MARKER) else { continue };
        let after = line[pos + MARKER.len()..].trim();
        let name: String = after.chars().take_while(|c| !c.is_whitespace()).collect();
        // Skip if anything follows the name — that means it's "Logged in to X", not an account.
        let remainder = after[name.len()..].trim();
        if name.len() < 3 || !remainder.is_empty() { continue; }
        if let Ok(mut g) = squad_names.lock() {
            if !g.iter().any(|n: &String| n == &name) { g.push(name.clone()); }
        }
        if let Ok(mut n) = app.state::<AppState>().local_player_name.lock() {
            *n = Some(name.clone());
        }
        // Emit immediately so the header updates without waiting for the next scan tick.
        let _ = app.emit("player-name", &name);
        return;
    }
}

/// Seed local-player and squad names from bounded reads of the existing EE.log.
pub(crate) fn seed_ee_log_names(
    log_path: &std::path::Path,
    squad_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    app: &tauri::AppHandle,
) {
    use std::io::{Read, Seek, SeekFrom};

    if let Ok(mut f) = std::fs::File::open(log_path) {
        let mut first = Vec::with_capacity(64 * 1024);
        let _ = (&mut f).take(64 * 1024).read_to_end(&mut first);
        if let Ok(text) = std::str::from_utf8(&first) {
            parse_logged_in_name(text, squad_names, app);
        }

        let file_len = f.seek(SeekFrom::End(0)).unwrap_or(0);
        let read_from = file_len.saturating_sub(1_048_576);
        let _ = f.seek(SeekFrom::Start(read_from));
        let mut buf = Vec::with_capacity(1_048_576);
        let _ = f.read_to_end(&mut buf);
        let start = if read_from > 0 {
            buf.iter().position(|&b| b == b'\n').map_or(0, |i| i + 1)
        } else {
            0
        };
        if let Ok(text) = std::str::from_utf8(&buf[start..]) {
            parse_logged_in_name(text, squad_names, app);
            for line in text.lines() {
                let name = if let Some(after) = line.find("AddSquadMember: ").map(|i| &line[i + 16..]) {
                    after.split(',').next().map(str::trim).filter(|name| !name.is_empty())
                } else if line.contains(" - new avatar: ") {
                    line.find("]: ")
                        .map(|i| &line[i + 3..])
                        .and_then(|after| after.split(" - new avatar:").next())
                        .map(str::trim)
                        .filter(|name| name.len() >= 3 && !name.contains(' '))
                } else {
                    None
                };
                if let Some(name) = name {
                    if let Ok(mut names) = squad_names.lock() {
                        if !names.iter().any(|existing| existing == name) {
                            names.push(name.to_string());
                        }
                    }
                }
            }
        }
    }
}

/// Update the OCR name filter from newly appended EE.log lines.
pub(crate) fn collect_ee_log_names(
    text: &str,
    squad_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    app: &tauri::AppHandle,
) {
    for line in text.lines() {
        let name = if let Some(after) = line.find("AddSquadMember: ").map(|i| &line[i + 16..]) {
            after.split(',').next().map(str::trim).filter(|name| !name.is_empty())
        } else if line.contains(" - new avatar: ") {
            line.find("]: ")
                .map(|i| &line[i + 3..])
                .and_then(|after| after.split(" - new avatar:").next())
                .map(str::trim)
                .filter(|name| name.len() >= 3 && !name.contains(' '))
        } else {
            None
        };
        if let Some(name) = name {
            if let Ok(mut names) = squad_names.lock() {
                if !names.iter().any(|existing| existing == name) {
                    names.push(name.to_string());
                }
            }
        }
        if line.contains("Logged in ") {
            parse_logged_in_name(line, squad_names, app);
        }
    }
}

/// Collect the distinct relic projection paths announced while squad loadouts download.
pub(crate) fn collect_session_relics(text: &str, session_relics: &mut Vec<String>) {
    for line in text.lines() {
        if line.contains("Resource load completed")
            && line.contains("/Lotus/Types/Game/Projections/")
        {
            if let Some(paren) = line.find("(/Lotus/Types/Game/Projections/") {
                let path = line[paren + 1..].split(')').next().unwrap_or("").trim();
                if !path.is_empty() && !session_relics.iter().any(|relic| relic == path) {
                    session_relics.push(path.to_string());
                }
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct VoidProjectionState {
    in_sequence: bool,
    sequence_completed: bool,
    other_ids: std::collections::HashSet<String>,
    own_item: String,
}

impl VoidProjectionState {
    pub(crate) fn consume_sequence_completed(&mut self) -> bool {
        std::mem::take(&mut self.sequence_completed)
    }

    pub(crate) fn take_own_item(&mut self) -> Option<String> {
        if self.own_item.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.own_item))
        }
    }
}

/// Update the VoidProjections reward-handshake state from newly appended EE.log lines.
pub(crate) fn collect_void_projection_state(
    text: &str,
    state: &mut VoidProjectionState,
    squad_size: &std::sync::Arc<std::sync::Mutex<Option<usize>>>,
    session_log_path: &std::path::Path,
) {
    for line in text.lines() {
        let lower = line.to_lowercase();
        if lower.contains("voidprojections: getvoidprojectionreward") {
            state.in_sequence = true;
            state.other_ids.clear();
            state.own_item.clear();
            if let Ok(mut size) = squad_size.lock() {
                *size = None;
            }
        }
        if lower.contains("gets reward /lotus/") {
            if let Some(index) = line.find("/Lotus/") {
                state.own_item = line[index..].trim().to_string();
            }
        }
        if state.in_sequence {
            if lower.contains("still waiting on response from") {
                if let Some(id) = lower.split_whitespace().last() {
                    state.other_ids.insert(id.to_string());
                }
            } else if lower.contains("has reward info for all players now") {
                let squad = (1 + state.other_ids.len()).clamp(1, 4);
                if let Ok(mut size) = squad_size.lock() {
                    *size = Some(squad);
                }
                state.in_sequence = false;
                state.sequence_completed = true;
                let _ = append_to_file(
                    session_log_path,
                    &format!(
                        "[EE.log] VoidProjections squad\n\\
                         ├─ Local item : {}\n\\
                         ├─ Other players (unique IDs) : {}\n\\
                         └─ Squad size : {} total\n\n",
                        if state.own_item.is_empty() { "(not found)" } else { &state.own_item },
                        state.other_ids.len(),
                        squad,
                    ),
                );
            }
        }
    }
}

/// Apply the local player's EE.log reward before the next memory scan completes.
pub(crate) fn apply_reward_inventory_update(
    app: &tauri::AppHandle,
    store_path: String,
    session_log_path: &std::path::Path,
) {
    let inv_path = crate::worldstate::store_to_unique(&store_path);
    let state: tauri::State<AppState> = app.state();
    let (old_qty, new_qty) = {
        let mut quantities = state.current_quantities.lock().unwrap_or_else(|e| e.into_inner());
        let old = *quantities.get(&inv_path).unwrap_or(&0);
        let new = old + 1;
        quantities.insert(inv_path.clone(), new);
        (old, new)
    };
    let item_name = inv_path.split('/').next_back().unwrap_or("?").to_string();
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&state.changes_log_path)
    {
        use std::io::Write;
        let _ = writeln!(
            file,
            "[{}] EE.log Reward | {} | {} → {} (gets reward)",
            timestamp, item_name, old_qty, new_qty
        );
    }
    let _ = app.emit("inventory-reward", serde_json::json!({ "path": inv_path, "qty": new_qty }));
    append_to_diag(
        session_log_path,
        &format!(
            "[REWARD] Inventory updated from EE.log\n\\
             ├─ Store path : {}\n\\
             ├─ Inv path   : {}\n\\
             └─ Qty        : {} → {}\n\n",
            store_path, inv_path, old_qty, new_qty
        ),
    );
}

/// Handle an EE.log reward-screen dismissal and schedule the overlay cleanup.
pub(crate) fn dismiss_relic_rewards(
    app: &tauri::AppHandle,
    text: &str,
    session_log_path: &std::path::Path,
    diag_dir: &std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    reward_screen_active: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    active_since: &mut Option<std::time::Instant>,
    last_dismiss_at: &mut Option<std::time::Instant>,
    session_relics: &mut Vec<String>,
    rewards_emitted_ms: &std::sync::Arc<std::sync::atomic::AtomicU64>,
    projection_state: &mut VoidProjectionState,
) -> bool {
    let lower = text.to_lowercase();
    let is_dismiss = lower.contains("relic reward screen shut down")
        || lower.contains("closevoidprojectionrewardscreen")
        || lower.contains("matchingservice::endsession");
    if !is_dismiss {
        return false;
    }

    let dismiss_line = text.lines()
        .find(|line| {
            let line = line.to_lowercase();
            line.contains("relic reward screen shut down")
                || line.contains("closevoidprojectionrewardscreen")
                || line.contains("matchingservice::endsession")
        })
        .unwrap_or("<unknown dismiss line>")
        .trim();
    let elapsed = active_since.map(|time| time.elapsed().as_secs_f64());
    append_to_diag(
        session_log_path,
        &format!(
            "[STEP 4] DISMISS\n\\
             ├─ Time     : {}\n\\
             ├─ Line     : \"{}\"\n\\
             └─ Open for : {}\n\n",
            chrono::Local::now().format("%H:%M:%S%.3f"),
            dismiss_line,
            elapsed.map(|seconds| format!("{seconds:.1}s")).unwrap_or_else(|| "(unknown)".to_string()),
        ),
    );
    if let Ok(mut guard) = diag_dir.lock() {
        if let Some(folder) = guard.take() {
            let _ = std::fs::copy(session_log_path, folder.join("ocr_session_log.txt"));
        }
    }
    reward_screen_active.store(false, Ordering::SeqCst);
    *active_since = None;
    *last_dismiss_at = Some(std::time::Instant::now());
    if lower.contains("matchingservice::endsession") {
        session_relics.clear();
    }
    if let Some(store_path) = projection_state.take_own_item() {
        apply_reward_inventory_update(app, store_path, session_log_path);
    }

    const MIN_DISPLAY_MS: u64 = 5_000;
    let emitted_at = rewards_emitted_ms.load(Ordering::SeqCst);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let delay_ms = if emitted_at > 0 {
        MIN_DISPLAY_MS.saturating_sub(now_ms.saturating_sub(emitted_at))
    } else {
        0
    };
    rewards_emitted_ms.store(0, Ordering::SeqCst);

    let dismiss_app = app.clone();
    std::thread::spawn(move || {
        if delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        }
        if let Some(window) = dismiss_app.get_webview_window("relic-overlay") {
            let _ = window.set_position(tauri::Position::Physical(
                tauri::PhysicalPosition { x: 0, y: -3000 },
            ));
        }
        if let Ok(mut rewards) = dismiss_app.state::<AppState>().pending_relic_rewards.lock() {
            *rewards = None;
        }
        let _ = dismiss_app.emit("relic-rewards", serde_json::Value::Null);
    });
    true
}

/// Dismiss a reward overlay that remained active beyond its maximum window.
pub(crate) fn auto_dismiss_relic_rewards(
    app: &tauri::AppHandle,
    session_log_path: &std::path::Path,
    diag_dir: &std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    reward_screen_active: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    active_since: &mut Option<std::time::Instant>,
    last_dismiss_at: &mut Option<std::time::Instant>,
) {
    let Some(since) = *active_since else { return };
    if since.elapsed().as_secs() < 20 {
        return;
    }
    append_to_diag(
        session_log_path,
        &format!(
            "[STEP 4] AUTO-DISMISS (20s timeout)\n\\
             ├─ Time     : {}\n\\
             └─ Open for : {:.1}s\n\n",
            chrono::Local::now().format("%H:%M:%S%.3f"),
            since.elapsed().as_secs_f64(),
        ),
    );
    if let Ok(mut guard) = diag_dir.lock() {
        if let Some(folder) = guard.take() {
            let _ = std::fs::copy(session_log_path, folder.join("ocr_session_log.txt"));
        }
    }
    reward_screen_active.store(false, Ordering::SeqCst);
    *active_since = None;
    *last_dismiss_at = Some(std::time::Instant::now());
    if let Some(window) = app.get_webview_window("relic-overlay") {
        let _ = window.set_position(tauri::Position::Physical(
            tauri::PhysicalPosition { x: 0, y: -3000 },
        ));
    }
    if let Ok(mut rewards) = app.state::<AppState>().pending_relic_rewards.lock() {
        *rewards = None;
    }
    let _ = app.emit("relic-rewards", serde_json::Value::Null);
}

/// Narrow the OCR catalog to rewards from relics seen in the current session.
pub(crate) fn build_relic_reward_catalog(
    app: &tauri::AppHandle,
    session_relics: &[String],
    full_catalog: &std::sync::Arc<Vec<(String, String)>>,
) -> (std::sync::Arc<Vec<(String, String)>>, String) {
    if session_relics.is_empty() {
        return (
            std::sync::Arc::clone(full_catalog),
            "  No relics collected — using full catalog (FrameForge started mid-mission?)".to_string(),
        );
    }
    let mut rewards: Vec<(String, String)> = {
        let state = app.state::<AppState>();
        let reward_map = state.relic_rewards.lock().unwrap_or_else(|e| e.into_inner());
        session_relics
            .iter()
            .filter_map(|path| reward_map.get(path.as_str()))
            .flat_map(|rewards| rewards.iter().map(|reward| (reward.unique_name.clone(), reward.name.clone())))
            .filter(|(_, name)| !name.is_empty())
            .collect()
    };
    if rewards.is_empty() {
        return (
            std::sync::Arc::clone(full_catalog),
            format!(
                "  {} relic path(s) found but none matched relic_rewards — using full catalog\n  Paths: {:?}",
                session_relics.len(), session_relics
            ),
        );
    }
    rewards.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    rewards.dedup_by(|a, b| !a.0.is_empty() && a.0 == b.0);
    let names: Vec<&str> = rewards.iter().map(|(_, name)| name.as_str()).collect();
    let sample = &names[..names.len().min(8)];
    let log = format!(
        "  {} relic(s) → {} rewards (direct from Relics.json)\n  Relics: {:?}\n  Rewards: {:?}",
        session_relics.len(), rewards.len(), session_relics, sample
    );
    (std::sync::Arc::new(rewards), log)
}

/// Build a fallback OCR catalog when the initial cache was empty at monitor startup.
pub(crate) fn build_fallback_reward_catalog(
    app: &tauri::AppHandle,
) -> Option<std::sync::Arc<Vec<(String, String)>>> {
    let state = app.state::<AppState>();
    let items = state.wfcd_items.lock().unwrap_or_else(|e| e.into_inner());
    if items.is_empty() {
        return None;
    }
    let blueprints = state.blueprint_to_result.lock().unwrap_or_else(|e| e.into_inner());
    let excluded = [
        "Warframes", "Primary", "Secondary", "Melee", "Companion", "Sentinels", "Archwing",
        "Arch-Gun", "Arch-Melee", "Pets", "Robotic",
    ];
    let mut catalog: Vec<(String, String)> = items
        .iter()
        .filter(|item| {
            let name = item.name.to_lowercase();
            !excluded.contains(&item.category.as_str())
                && !name.ends_with("intact")
                && !name.ends_with("exceptional")
                && !name.ends_with("flawless")
                && !name.ends_with("radiant")
                && (name.contains("prime") || name.starts_with("forma"))
        })
        .map(|item| (item.unique_name.clone(), item.name.clone()))
        .collect();
    for (path, (name, _)) in blueprints.iter() {
        let lower = name.to_lowercase();
        if lower.contains("prime") || lower.starts_with("forma") {
            catalog.push((path.clone(), name.clone()));
        }
    }
    catalog.sort_by(|a, b| a.0.cmp(&b.0));
    catalog.dedup_by(|a, b| a.0 == b.0);
    (!catalog.is_empty()).then(|| std::sync::Arc::new(catalog))
}

/// Start the diagnostic files for a relic-reward OCR session.
pub(crate) fn prepare_reward_session(
    session_log_path: &std::path::Path,
    squad_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    timestamp: &str,
    trigger_line: &str,
    prefilter_log: &str,
    catalog_len: usize,
    auto_capture_dir: &std::path::Path,
    diag_dir: &std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    last_found_path: &std::path::Path,
) {
    let names = squad_names.lock().map(|names| names.clone()).unwrap_or_default();
    let known_names = if names.is_empty() {
        "  (none — names not yet seen in EE.log)".to_string()
    } else {
        names.iter().map(|name| format!("  • {name}")).collect::<Vec<_>>().join("\n")
    };
    if let Err(error) = std::fs::write(
        session_log_path,
        format!(
            "══════════════════════════════════════════════\n\\
             RELIC OVERLAY SESSION — {}\n\\
             ═════════════════════════════════════════════\n\\
             Log path  : {}\n\n\\
             [KNOWN PLAYERS — OCR username filter]\n\\
             {}\n\n\\
             [STEP 1] EE.log TRIGGER\n\\
             ├─ Time     : {}\n\\
             ├─ Line     : \"{}\"\n\\
             ├─ Prefilter: {}\n\\
             └─ Catalog  : {} items\n\n",
            timestamp,
            session_log_path.display(),
            known_names,
            timestamp,
            trigger_line,
            prefilter_log,
            catalog_len,
        ),
    ) {
        warn!(error = %error, "session log write failed");
    }
    let run_dir = auto_capture_dir.join(chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string());
    let _ = std::fs::create_dir_all(&run_dir);
    if let Ok(mut guard) = diag_dir.lock() {
        *guard = Some(run_dir);
    }
    let _ = std::fs::write(
        last_found_path,
        format!("=== {} ===\nEE.log trigger fired\n{}\n", timestamp, trigger_line),
    );
}

/// Prepare OCR hints immediately when a relic reward screen is triggered.
pub(crate) fn prepare_reward_trigger(
    app: &tauri::AppHandle,
    squad_names: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    squad_size: &std::sync::Arc<std::sync::Mutex<Option<usize>>>,
    session_relics: &[String],
) {
    if let Ok(local_player) = app.state::<AppState>().local_player_name.lock() {
        if let Some(name) = local_player.as_ref() {
            if let Ok(mut names) = squad_names.lock() {
                if !names.iter().any(|existing| existing == name) {
                    names.push(name.clone());
                }
            }
        }
    }
    let relic_hint = session_relics.len().min(4);
    if relic_hint >= 1 {
        if let Ok(mut hint) = squad_size.lock() {
            if hint.is_none() {
                *hint = Some(relic_hint);
            }
        }
    }
}

/// Give the VoidProjections sequence a short window to provide an OCR card-count hint.
pub(crate) async fn wait_for_squad_hint(
    squad_size: &std::sync::Arc<std::sync::Mutex<Option<usize>>>,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if squad_size.lock().ok().is_some_and(|hint| hint.is_some()) {
            break;
        }
    }
}

/// Capture the reward area and run OCR after reading the latest EE.log hints.
pub(crate) async fn capture_reward_items(
    app: &tauri::AppHandle,
    catalog: std::sync::Arc<Vec<(String, String)>>,
    squad_size: std::sync::Arc<std::sync::Mutex<Option<usize>>>,
    squad_names: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) -> Option<RewardOcrResult> {
    let frame = std::sync::Arc::clone(&app.state::<AppState>().last_ocr_frame);
    tauri::async_runtime::spawn_blocking(move || {
        let (pixels, width, capture_height, game_height, capture_info) =
            crate::ocr::capture_warframe_reward_area()?;
        // Keep the source frame for diagnostics without another GPU readback.
        if let Ok(mut cached) = frame.lock() {
            *cached = Some((pixels.clone(), width, capture_height));
        }
        let hint_squad_size = squad_size.lock().ok().and_then(|hint| *hint);
        let player_names = squad_names.lock().map(|names| names.clone()).unwrap_or_default();
        Some(ocr::extract_reward_items_twophase(OcrParams {
            pixels: &pixels,
            pix_w: width,
            pix_h: capture_height,
            game_h: game_height,
            catalog: &catalog,
            capture_info: &capture_info,
            hint_squad_size,
            player_names: &player_names,
        }))
    })
    .await
    .ok()
    .flatten()
}

/// Save a delayed desktop screenshot after the relic overlay has animated in.
pub(crate) fn schedule_reward_diagnostic_capture(
    diag_dir: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
) {
    let folder = diag_dir.lock().ok().and_then(|guard| guard.clone());
    if let Some(folder) = folder {
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(4000)).await;
            tauri::async_runtime::spawn_blocking(move || {
                if let Some((pixels, width, height)) = crate::ocr::capture_desktop_for_diag() {
                    let _ = write_bmp(&folder.join("screenshot.bmp"), &pixels, width, height);
                }
            })
            .await
            .ok();
        });
    }
}

/// Schedule the final cleanup when the normal EE.log dismissal never arrives.
pub(crate) fn schedule_reward_safety_cleanup(
    app: tauri::AppHandle,
    session_log_path: std::path::PathBuf,
    diag_dir: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    also_write_diagnostics: bool,
) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        if let Ok(mut rewards) = app.state::<AppState>().pending_relic_rewards.lock() {
            *rewards = None;
        }
        let _ = app.emit("relic-rewards", serde_json::Value::Null);
        if let Some(window) = app.get_webview_window("relic-overlay") {
            let _ = window.set_position(tauri::Position::Physical(
                tauri::PhysicalPosition { x: 0, y: -3000 },
            ));
        }
        if also_write_diagnostics {
            append_to_diag(&session_log_path, "[STEP 4] AUTO-DISMISS (20s safety fallback)\n\n");
        } else {
            let _ = append_to_file(&session_log_path, "[STEP 4] AUTO-DISMISS (20s safety fallback)\n\n");
        }
        if let Ok(mut guard) = diag_dir.lock() {
            if let Some(folder) = guard.take() {
                let _ = std::fs::copy(&session_log_path, folder.join("ocr_session_log.txt"));
            }
        }
    });
}

/// Store the payload for a late overlay mount, then notify the frontend.
pub(crate) fn publish_relic_rewards(
    app: &tauri::AppHandle,
    payload: Option<&serde_json::Value>,
) {
    if let Some(payload) = payload.filter(|payload| !payload.is_null()) {
        if let Ok(mut pending) = app.state::<AppState>().pending_relic_rewards.lock() {
            *pending = Some(payload.clone());
        }
    }
    let _ = app.emit("relic-rewards", payload);
}

/// Emit the best partial result after OCR timed out, hide the overlay, and
/// persist the session log to the diagnostic folder.
pub(crate) fn finalize_reward_ocr_timeout(
    app: &tauri::AppHandle,
    best_payload: Option<serde_json::Value>,
    active: &std::sync::atomic::AtomicBool,
    session_log_path: &std::path::Path,
    diag_dir: &std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
) {
    let emit_val = if active.load(Ordering::SeqCst) {
        best_payload.unwrap_or(serde_json::Value::Null)
    } else {
        serde_json::Value::Null
    };
    publish_relic_rewards(app, Some(&emit_val));
    let _ = append_to_file(
        session_log_path,
        "[STEP 2] OCR TIMEOUT — 45 seconds elapsed, emitting best result\n\n",
    );
    if let Some(win) = app.get_webview_window("relic-overlay") {
        let _ = win.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
            x: 0,
            y: -3000,
        }));
    }
    active.store(false, Ordering::SeqCst);
    if let Ok(mut g) = diag_dir.lock() {
        if let Some(folder) = g.take() {
            let _ = std::fs::copy(session_log_path, folder.join("ocr_session_log.txt"));
        }
    }
}

/// Log that the OCR loop was stopped by an external dismiss signal.
pub(crate) fn log_reward_ocr_stopped(session_log_path: &std::path::Path) {
    let _ = append_to_file(
        session_log_path,
        "[STEP 2] OCR STOPPED — dismiss signal received\n\n",
    );
}

// ── OCR retry-loop helpers ────────────────────────────────────────────────────

pub(crate) fn log_reward_dark_frame(
    app: &tauri::AppHandle,
    attempt: u32,
    ts: &str,
    dbg: &str,
    session_log_path: &std::path::Path,
    last_path: &std::path::Path,
) -> u64 {
    let entry = format!(
        "[STEP 2] OCR ATTEMPT #{}\n\
         ├─ Time     : {}\n\
         └─ RESULT   : {} → PrintWindow returned dark image\n\
            Check %TEMP%\\frameforge_capture_debug.bmp\n\
            Fix: switch Warframe to Borderless Windowed mode\n\
            Retrying in 100ms…\n\n",
        attempt, ts, dbg
    );
    let _ = append_to_file(session_log_path, &entry);
    let _ = std::fs::write(last_path, format!("=== {} ===\n{} — retrying\n", ts, dbg));
    let _ = app.emit("ff-status", format!("⬛ {}", dbg));
    100
}

pub(crate) fn log_reward_ocr_empty(
    app: &tauri::AppHandle,
    attempt: u32,
    ts: &str,
    dbg: &str,
    session_log_path: &std::path::Path,
    last_path: &std::path::Path,
) -> u64 {
    let entry = format!(
        "[STEP 2] OCR ATTEMPT #{}\n\
         ├─ Time     : {}\n\
         └─ RESULT   : {} → image has content but OCR found no text\n\
            Check %TEMP%\\frameforge_capture_debug.bmp\n\
            Retrying in 300ms…\n\n",
        attempt, ts, dbg
    );
    let _ = append_to_file(session_log_path, &entry);
    let _ = std::fs::write(last_path, format!("=== {} ===\n{} — retrying\n", ts, dbg));
    let _ = app.emit("ff-status", format!("⬜ {}", dbg));
    300
}

pub(crate) fn log_reward_capture_failed(
    app: &tauri::AppHandle,
    attempt: u32,
    ts: &str,
    session_log_path: &std::path::Path,
    last_path: &std::path::Path,
) -> u64 {
    let entry = format!(
        "[STEP 2] OCR ATTEMPT #{}\n\
         ├─ Time     : {}\n\
         └─ RESULT   : capture failed — Warframe window not found\n\
            Retrying in 500ms…\n\n",
        attempt, ts
    );
    let _ = append_to_file(session_log_path, &entry);
    let _ = std::fs::write(last_path, format!("=== {} ===\nCapture failed (window not found?)\n", ts));
    let _ = app.emit("ff-status", "⚠️ Capture failed");
    500
}

pub(crate) fn log_reward_no_match(
    app: &tauri::AppHandle,
    attempt: u32,
    ts: &str,
    items: &[String],
    dbg: &str,
    no_match_streak: &mut u32,
    cat: &mut std::sync::Arc<Vec<(String, String)>>,
    fallback_cat: &std::sync::Arc<Vec<(String, String)>>,
    session_log_path: &std::path::Path,
    last_path: &std::path::Path,
    diag_dir: &std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
) -> u64 {
    *no_match_streak += 1;
    let expanded = if *no_match_streak == 3 && cat.len() < fallback_cat.len() {
        *cat = std::sync::Arc::clone(fallback_cat);
        true
    } else {
        false
    };
    let cur_cat_len = cat.len();
    let expand_note = if expanded {
        format!(" [expanded to full catalog: {}]", cur_cat_len)
    } else {
        String::new()
    };
    let entry = format!(
        "[STEP 2] OCR ATTEMPT #{}\n\
         ├─ Time     : {}\n\
         {}\n\
         └─ RESULT   : no catalog match (catalog={}){}\u{2192} retrying in 700ms\n\n",
        attempt, ts, dbg, cur_cat_len, expand_note
    );
    let _ = append_to_file(session_log_path, &entry);
    let _ = std::fs::write(
        last_path,
        format!("=== {} ===\nno match (catalog={}): {:?}\n{}\n", ts, cur_cat_len, items, dbg),
    );
    let _ = app.emit("ff-status", "❌ No catalog match, retrying...");
    if attempt == 1 {
        let frame = app.state::<AppState>().last_ocr_frame.lock()
            .ok().and_then(|g| g.clone());
        let diag_snap = diag_dir.lock().ok().and_then(|g| g.clone());
        if let (Some((px, w, h)), Some(folder)) = (frame, diag_snap) {
            let _ = write_bmp(&folder.join("screenshot.bmp"), &px, w, h);
        }
    }
    700
}

pub(crate) fn log_reward_best_result(
    attempt: u32,
    ts: &str,
    items: &[String],
    dbg: &str,
    complete: bool,
    confirm_ready: bool,
    session_log_path: &std::path::Path,
    last_path: &std::path::Path,
) {
    let label = if complete && confirm_ready { "✅" } else { "⚡" };
    let status_label = if complete && confirm_ready {
        "locked"
    } else if complete {
        "soft-complete, waiting for EE hint"
    } else {
        "waiting"
    };
    let _ = crate::append_to_file(
        session_log_path,
        &format!("{} {} items ({})", label, items.len(), status_label),
    );
    let result_label = if complete && confirm_ready {
        "LOCKED & emitting"
    } else if complete {
        "soft-complete, retrying (waiting for EE hint)"
    } else {
        "saved, retrying"
    };
    let session_entry = format!(
        "[STEP 2] OCR ATTEMPT #{}\n\
         ├─ Time     : {}\n\
         {}\n\
         └─ RESULT   : {} items found \u{2192} {}\n\
         \u{2514}\u{2500} Items    : {:?}\n\n",
        attempt, ts, dbg, items.len(), result_label, items,
    );
    let _ = append_to_file(session_log_path, &session_entry);
    let _ = std::fs::write(last_path, format!("=== {} ===\nItems: {:?}\n{}\n", ts, items, dbg));
}

pub(crate) fn log_reward_confirm_no_improvement(
    attempt: u32,
    ts: &str,
    items: &[String],
    session_log_path: &std::path::Path,
) {
    let _ = crate::append_to_file(
        session_log_path,
        &format!(
            "[STEP 2] OCR ATTEMPT #{} (confirm)\n\
             \u{251c}\u{2500} Time     : {}\n\
             \u{2514}\u{2500} {} items \u{2014} same as before, confirmed\n\n",
            attempt, ts, items.len()
        ),
    );
}

// ── WFM whisper parsing ──────────────────────────────────────────────────────

pub(crate) fn parse_and_emit_wfm_whisper(
    app: &tauri::AppHandle,
    log_line: &str,
) {
    let raw = log_line;
    let from = raw.find("@From ")
        .map(|i| &raw[i+6..])
        .and_then(|s| s.split(" :").next())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "Unknown".to_string());
    let item = {
        let prefix = "want to buy ";
        let suffix = " for ";
        raw.find(prefix).and_then(|i| {
            let rest = &raw[i+prefix.len()..];
            rest.find(suffix).map(|j| crate::sanitize_chat_item_name(&rest[..j]))
        })
    };
    let price: Option<u64> = raw.find(" for ").and_then(|i| {
        let rest = &raw[i+5..];
        rest.find(" platinum").and_then(|j| rest[..j].trim().parse().ok())
    });
    let _ = app.emit("wfm-whisper", serde_json::json!({
        "from": from,
        "message": raw.trim(),
        "item": item,
        "price": price,
        "timestamp": chrono::Local::now().format("%H:%M:%S").to_string(),
    }));
}
