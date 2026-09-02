use tracing::warn;
use tauri::{Emitter, State};
use crate::app_state::AppState;
use crate::cache::atomic_write;
use crate::credentials::wfm_delete_credentials;
use crate::paths;

/// Wipe all app data and restart — factory reset.
/// Wipes all user data and restarts the app into a clean state.
///
/// The DB files cannot be deleted while the current process holds them open,
/// so we write a marker to %TEMP% and finish the deletion on the next launch
/// (before a new connection is opened).  Everything else is deleted immediately.
#[tauri::command]
pub(crate) async fn factory_reset(app: tauri::AppHandle, _state: State<'_, AppState>) -> Result<(), String> {
    let _ = wfm_delete_credentials().await;

    let config_dir = paths::config_dir();
    let data_dir   = paths::data_dir();
    let cache_dir  = paths::cache_dir();

    // Delete user-editable config files.
    for name in &["settings.json", "corrections.json"] {
        let _ = std::fs::remove_file(config_dir.join(name));
    }
    // Delete user state that isn't the DB.
    let _ = std::fs::remove_file(data_dir.join("auction_ids.json"));
    // Wipe the entire cache tree — everything in it refetches on next launch.
    let _ = std::fs::remove_dir_all(&cache_dir);

    // Ask the next launch to delete the DB once it can (before opening a connection).
    let marker = std::env::temp_dir().join("frameforge_factory_reset");
    std::fs::write(&marker, b"").map_err(|e| e.to_string())?;

    app.restart();
}

fn update_version_in_file(path: &std::path::Path, version: &str) -> Result<(), String> {
    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    // Replace first occurrence of "version": "x.y.z"
    let marker = "\"version\": \"";
    if let Some(start) = content.find(marker) {
        let after = start + marker.len();
        if let Some(end) = content[after..].find('"') {
            let mut updated = content.clone();
            updated.replace_range(after..after + end, version);
            std::fs::write(path, updated).map_err(|e| e.to_string())?;
            return Ok(());
        }
    }
    Err(format!("Version field not found in {}", path.display()))
}

#[tauri::command]
pub(crate) fn get_app_version() -> String {
    // In dev mode the source tauri.conf.json is in the current directory
    let config = std::path::Path::new("src-tauri/tauri.conf.json");
    if config.exists() {
        if let Ok(text) = std::fs::read_to_string(config) {
            let marker = "\"version\": \"";
            if let Some(start) = text.find(marker) {
                let after = start + marker.len();
                if let Some(end) = text[after..].find('"') {
                    return text[after..after + end].to_string();
                }
            }
        }
    }
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
pub(crate) fn set_app_version(version: String) -> Result<(), String> {
    let tauri_conf = std::path::Path::new("src-tauri/tauri.conf.json");
    let package_json = std::path::Path::new("package.json");
    if tauri_conf.exists() { update_version_in_file(tauri_conf, &version)?; }
    if package_json.exists() { update_version_in_file(package_json, &version)?; }
    Ok(())
}

/// Hard-exit the process. Called from the frontend close handler when destroy()
/// is unreliable (e.g. after a Promise.race timeout on a hanging WFM API call).
#[tauri::command]
pub(crate) fn force_quit() {
    std::process::exit(0);
}

#[tauri::command]
pub(crate) fn load_settings(state: State<AppState>) -> String {
    std::fs::read_to_string(&state.settings_path).unwrap_or_default()
}

// settings.json is written from several threads (the save_settings command,
// window-event handlers on every move/resize). Every writer must go through
// merge_settings; an unserialized or non-atomic write used to tear the file
// and wipe all settings on the next merge.
static SETTINGS_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn read_settings_map(path: &std::path::Path) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    if raw.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(serde_json::Value::Object(m)) => Ok(m),
        _ => Err(format!("{} exists but is not a valid JSON object; refusing to overwrite it", path.display())),
    }
}

fn merge_settings(path: &std::path::Path, apply: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>)) -> Result<(), String> {
    // Poison recovery is safe: the lock guards the file, not the map, and a
    // panicking closure bails before the write, leaving the file untouched.
    let _guard = SETTINGS_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut map = read_settings_map(path)?;
    apply(&mut map);
    atomic_write(path, serde_json::Value::Object(map).to_string().as_bytes()).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn save_settings(app: tauri::AppHandle, state: State<AppState>, json: String) -> Result<(), String> {
    // Merge over existing file so geometry fields written by save_window_state are never erased
    let new_vals: serde_json::Value = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    merge_settings(&state.settings_path, |existing| {
        if let serde_json::Value::Object(new_map) = new_vals {
            for (k, v) in new_map { existing.insert(k, v); }
        }
    })?;
    app.emit("settings-updated", ()).ok();
    Ok(())
}

pub(crate) fn save_window_state(window: &tauri::WebviewWindow, settings_path: &std::path::Path, prefix: &str) {
    let maximized = window.is_maximized().unwrap_or(false);
    let minimized = window.is_minimized().unwrap_or(false);
    let pos  = window.outer_position().ok();
    let size = window.outer_size().ok();

    let result = merge_settings(settings_path, |map| {
        map.insert(format!("{}Maximized", prefix), maximized.into());
        // Only overwrite position/size when not maximised/minimised.
        // Also guard against the Windows minimized sentinel (-32000,-32000) and dummy size (160×28)
        // which can slip through when is_minimized() is unreliable at CloseRequested time.
        if !maximized && !minimized {
            if let Some(p) = pos {
                if p.x > -10_000 && p.y > -10_000 {
                    map.insert(format!("{}X", prefix), p.x.into());
                    map.insert(format!("{}Y", prefix), p.y.into());
                }
            }
            if let Some(s) = size {
                if s.width >= 100 && s.height >= 50 {
                    map.insert(format!("{}Width",  prefix), (s.width  as i64).into());
                    map.insert(format!("{}Height", prefix), (s.height as i64).into());
                }
            }
        }
    });
    if let Err(e) = result {
        warn!(error = %e, "not saving window state");
    }
}

pub(crate) fn restore_window_state(app: &tauri::AppHandle, window: &tauri::WebviewWindow, settings_path: &std::path::Path, prefix: &str, min_w: u32, min_h: u32) {
    let Ok(map) = read_settings_map(settings_path) else { return };

    let maximized = map.get(&format!("{}Maximized", prefix)).and_then(|v| v.as_bool()).unwrap_or(false);
    if maximized {
        let _ = window.maximize();
        return;
    }

    let x = map.get(&format!("{}X", prefix)).and_then(|v| v.as_i64());
    let y = map.get(&format!("{}Y", prefix)).and_then(|v| v.as_i64());
    let w = map.get(&format!("{}Width",  prefix)).and_then(|v| v.as_i64()).map(|v| v as u32);
    let h = map.get(&format!("{}Height", prefix)).and_then(|v| v.as_i64()).map(|v| v as u32);

    if let (Some(x), Some(y)) = (x, y) {
        // Guard against Windows' minimized-window sentinel (-32000, -32000) and positions
        // that fall outside every connected monitor (e.g. secondary unplugged since last run).
        if x > -10_000 && y > -10_000 {
            let monitors = app.available_monitors().unwrap_or_default();
            let on_screen = monitors.iter().any(|m| {
                let mp = m.position();
                let ms = m.size();
                x >= mp.x as i64 && x < (mp.x as i64 + ms.width as i64) &&
                y >= mp.y as i64 && y < (mp.y as i64 + ms.height as i64)
            });
            if on_screen {
                let _ = window.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
            }
            // If off-screen, leave the window at its default centered position.
        }
    }
    if let (Some(w), Some(h)) = (w, h) {
        if w >= min_w && h >= min_h {
            // Clamp to the monitor that contains the window's top-left corner so the
            // bottom edge never ends up off-screen (e.g. a session saved on a 1440p monitor
            // restored on a 768p monitor would otherwise put the bottom 432px off-screen,
            // making the scrollbar unreachable and the bottom of every page inaccessible).
            let monitors = app.available_monitors().unwrap_or_default();
            let wx = x.unwrap_or(0);
            let wy = y.unwrap_or(0);
            let max_h = monitors.iter()
                .find(|m| {
                    let mp = m.position();
                    let ms = m.size();
                    wx >= mp.x as i64 && wx < (mp.x as i64 + ms.width as i64) &&
                    wy >= mp.y as i64 && wy < (mp.y as i64 + ms.height as i64)
                })
                .map(|m| {
                    // Leave 60px for the Windows taskbar (physical pixels, before DPI scale).
                    m.size().height.saturating_sub(60)
                });
            let clamped_h = if let Some(max) = max_h { h.min(max) } else { h };
            let max_w = monitors.iter()
                .find(|m| {
                    let mp = m.position();
                    let ms = m.size();
                    wx >= mp.x as i64 && wx < (mp.x as i64 + ms.width as i64) &&
                    wy >= mp.y as i64 && wy < (mp.y as i64 + ms.height as i64)
                })
                .map(|m| m.size().width);
            let clamped_w = if let Some(max) = max_w { w.min(max) } else { w };
            let _ = window.set_size(tauri::PhysicalSize::new(clamped_w, clamped_h));
        }
    }
}

#[cfg(test)]
mod settings_merge_tests {
    use super::{merge_settings, read_settings_map};
    use std::path::PathBuf;

    /// Each test gets its own file so they can run in parallel.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("frameforge-settings-tests");
        std::fs::create_dir_all(&dir).expect("temp dir is always writable");
        let path = dir.join(format!("{name}.json"));
        let _ = std::fs::remove_file(&path);
        path
    }

    /// A truncated settings.json (crash or app kill mid-write, e.g. during an
    /// update) used to parse as "no settings", and the next merge rewrote the
    /// file from an empty map, wiping tracked and favorites. A file that
    /// exists but does not parse must be left exactly as it is.
    #[test]
    fn a_corrupt_settings_file_is_never_replaced() {
        let path = scratch("corrupt");
        let truncated = r#"{"tracked":["/Lotus/Weapons/Boar"],"favorites":["/Lo"#;
        std::fs::write(&path, truncated).expect("scratch file is writable");

        let result = merge_settings(&path, |map| {
            map.insert("windowX".into(), 10.into());
        });

        assert!(result.is_err(), "merging over an unparseable file must refuse, not wipe");
        let after = std::fs::read_to_string(&path).expect("file still exists");
        assert_eq!(after, truncated, "the corrupt file must be preserved for recovery");
    }

    /// A missing or empty file is an ordinary first launch, not corruption.
    #[test]
    fn a_missing_or_empty_file_is_a_fresh_start() {
        let path = scratch("fresh");
        assert!(read_settings_map(&path).expect("missing file is fine").is_empty());
        std::fs::write(&path, "").expect("scratch file is writable");
        assert!(read_settings_map(&path).expect("empty file is fine").is_empty());
        merge_settings(&path, |map| {
            map.insert("tracked".into(), serde_json::json!(["a"]));
        })
        .expect("merging into a fresh file succeeds");
    }

    #[test]
    fn merging_preserves_unrelated_keys() {
        let path = scratch("preserve");
        std::fs::write(&path, r#"{"tracked":["a"],"favorites":["b"]}"#).expect("scratch file is writable");
        merge_settings(&path, |map| {
            map.insert("windowX".into(), 42.into());
        })
        .expect("merge succeeds");
        let map = read_settings_map(&path).expect("file parses");
        assert_eq!(map["tracked"], serde_json::json!(["a"]));
        assert_eq!(map["favorites"], serde_json::json!(["b"]));
        assert_eq!(map["windowX"], serde_json::json!(42));
    }

    /// save_window_state fires on every window move while save_settings runs on
    /// the command thread. Unserialized, one writer read the file mid-truncate
    /// of the other and resurrected a stale or empty map.
    #[test]
    fn concurrent_merges_do_not_lose_keys() {
        let path = scratch("concurrent");
        std::fs::write(&path, r#"{"tracked":["a"]}"#).expect("scratch file is writable");
        std::thread::scope(|s| {
            for t in 0..8 {
                let path = &path;
                s.spawn(move || {
                    for i in 0..25 {
                        merge_settings(path, |map| {
                            map.insert(format!("k{t}_{i}"), i.into());
                        })
                        .expect("merge never fails on a valid file");
                    }
                });
            }
        });
        let map = read_settings_map(&path).expect("file parses after the storm");
        assert_eq!(map["tracked"], serde_json::json!(["a"]));
        for t in 0..8 {
            for i in 0..25 {
                assert!(map.contains_key(&format!("k{t}_{i}")), "lost k{t}_{i}");
            }
        }
    }
}
