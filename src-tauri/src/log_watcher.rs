use tracing::{info, warn};
use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager};
use crate::app_state::AppState;
use crate::monitor::append_to_file;
use crate::relic_pick::{build_relic_pick_payload, relic_pick_show, relic_pick_hide};
use crate::trade_log::parse_trade_dialog;
use crate::{log_parser, paths};

// ==============================================================================
// EE.log wake-up source
// ==============================================================================
//
// Both log watchers want to react the instant Warframe flushes a line. Windows
// gets that for free from FindFirstChangeNotificationW, which blocks until the
// log directory is written. Linux has no equivalent wired up here, so it falls
// back to polling at the caller's interval — a few extra wake-ups per second,
// but the surrounding loop stays identical on both platforms.
//
// ponytail: polling on Linux; switch to inotify if wake-up latency matters.

/// Open a directory-change notification for EE.log's folder, or `None` when the
/// platform or the call cannot provide one (the caller then polls).
#[cfg(target_os = "windows")]
pub(crate) fn open_log_notifier(log_path: &std::path::Path) -> Option<isize> {
    use windows_sys::Win32::Storage::FileSystem::{
        FindFirstChangeNotificationW, FILE_NOTIFY_CHANGE_LAST_WRITE,
    };
    let dir = log_path.parent().unwrap_or(std::path::Path::new("."));
    let dir_wide: Vec<u16> = dir.to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe { FindFirstChangeNotificationW(dir_wide.as_ptr(), 0, FILE_NOTIFY_CHANGE_LAST_WRITE) };
    (handle != -1).then_some(handle) // -1 = INVALID_HANDLE_VALUE
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn open_log_notifier(_log_path: &std::path::Path) -> Option<isize> { None }

/// Block until EE.log's directory is written, or until `poll` elapses when no
/// notifier is available. The 500 ms notification timeout keeps loops that check
/// a stop flag responsive even while the game is idle.
#[cfg(target_os = "windows")]
pub(crate) fn wait_for_log_change(notifier: Option<isize>, poll: std::time::Duration) {
    use windows_sys::Win32::Storage::FileSystem::FindNextChangeNotification;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    let Some(handle) = notifier else {
        std::thread::sleep(poll);
        return;
    };
    unsafe {
        WaitForSingleObject(handle, 500);
        FindNextChangeNotification(handle);
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn wait_for_log_change(_notifier: Option<isize>, poll: std::time::Duration) {
    std::thread::sleep(poll);
}

/// Start a lightweight EE.log watcher for features that don't need the memory scanner:
/// riven reroll detection, trade completion detection, WFM whisper detection.
/// Called unconditionally at app startup — EE.log is plain file I/O, not memory reading.
#[tauri::command]
pub(crate) fn start_log_watcher(app: tauri::AppHandle) -> Result<(), String> {
    let log_path =
        log_parser::watched_log_path().ok_or("Cannot find the local data directory")?;
    if !log_path.is_file() {
        warn!(path = %log_path.display(), "EE.log not found; log-driven features stay idle until it appears");
    }

    std::thread::spawn(move || {
        use std::io::{Read, Seek, SeekFrom};
        let mut file_pos: u64 = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
        let mut pending_trade: Option<String> = None;
        // Cooldown: don't fire riven-screen-open again within 4 seconds of the last fire.
        // Guards against the same EE.log buffer being processed twice by React StrictMode listeners.
        let mut last_riven_fire: Option<std::time::Instant> = None;
        // Cooldown: prevent spawning multiple OCR threads if the trigger fires rapidly.
        let mut last_relic_pick_trigger: Option<std::time::Instant> = None;

        // Wake up the instant EE.log is written instead of sleeping and polling.
        // This is how Overwolf achieves low latency.
        let notifier = open_log_notifier(&log_path);

        loop {
            wait_for_log_change(notifier, std::time::Duration::from_millis(50));
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
                .map_or(true, |t| t.elapsed().as_secs() >= 4);

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
                let riven_active = last_riven_fire.map_or(false, |t| {
                    let e = t.elapsed().as_secs();
                    e >= 1 && e < 600
                });
                if riven_active {
                    last_riven_fire = None;
                    let riven_log = paths::state_dir().join("frameforge_riven_session.txt");
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
                let riven_active = last_riven_fire.map_or(false, |t| {
                    let e = t.elapsed().as_secs();
                    e >= 3 && e < 600
                });
                if riven_active {
                    last_riven_fire = None;
                    let riven_log = paths::state_dir().join("frameforge_riven_session.txt");
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
                    raw.find(p).and_then(|i| { let r=&raw[i+p.len()..]; r.find(s).map(|j| r[..j].to_string()) })
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
                    .map_or(true, |t| now.duration_since(t).as_secs() >= 5);
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
