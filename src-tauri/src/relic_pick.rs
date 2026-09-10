use std::collections::HashMap;
use tracing::{debug, info, warn};
use tauri::{Emitter, Manager, State};
use crate::app_state::AppState;
use std::sync::atomic::Ordering;
use crate::{db, log_parser, overlay_linux};


// ── Overlay windows ───────────────────────────────────────────────────────────

/// Only shows; the window positions itself once it knows its rendered size,
/// because the overlay scale that decides that size lives in the frontend.
pub(crate) fn show_overlay(app: &tauri::AppHandle, label: &str) {
    use tauri::Manager;
    let Some(win) = app.get_webview_window(label) else {
        warn!(label, "overlay window missing; check tauri.conf.json and the capability list");
        return;
    };
    if let Err(e) = win.show() {
        warn!(label, error = %e, "overlay window did not show");
    }
}

// ── Relic pick overlay ────────────────────────────────────────────────────────

pub(crate) fn relic_pick_show(app: &tauri::AppHandle) {
    show_overlay(app, "relic-pick-overlay");
}

pub(crate) fn relic_pick_hide(app: &tauri::AppHandle) {
    use tauri::Manager;
    let Some(win) = app.get_webview_window("relic-pick-overlay") else { return };
    let _ = win.hide();
}

// ── Arbitration post-run overlay ──────────────────────────────────────────────

#[tauri::command]
pub(crate) fn set_arbitration_overlay_enabled(state: State<AppState>, enabled: bool) {
    state.arbitration_overlay_enabled.store(enabled, Ordering::SeqCst);
}

/// Debug: fire the post-run overlay with a made-up completed run.
#[tauri::command]
pub(crate) fn test_arbitration_overlay(app: tauri::AppHandle) -> String {
    let summary = db::RunSummary {
        node: "Stöfler (Lua)".into(),
        mission_type: "defense",
        duration_sec: 1487.0,
        rotations: 5,
        waves: 15,
        kills: 612,
        drone_kills: 23,
        host_telemetry: true,
        vitus_mean: 41.3,
        vitus_per_minute: 1.67,
    };
    show_overlay(&app, "arbitration-overlay");
    let _ = app.emit("arbitration-run-ended", &summary);
    "Emitted arbitration-run-ended with a sample run".to_string()
}

/// Debug: run OCR on the top-left quarter of the Warframe window and report the detected era.
#[tauri::command]
pub(crate) fn debug_detect_fissure_era() -> String {
    match crate::ocr::detect_fissure_era() {
        Some(era) => format!("Detected era: {}", era),
        None => "No era detected — OCR found no known fissure era label in the top-left quarter.".to_string(),
    }
}

/// Debug: manually fire the relic pick overlay for a given era.
#[tauri::command]
pub(crate) fn test_relic_pick_overlay(era: String, app: tauri::AppHandle) -> String {
    let payload = build_relic_pick_payload(&era, &app);
    let relic_count = payload["relics"].as_array().map_or(0, |a| a.len());
    relic_pick_show(&app);
    let _ = app.emit("relic-pick-open", &payload);
    format!("Emitted relic-pick-open: era={}, {} relics in inventory", era, relic_count)
}

/// Debug: return the last ~4 KB of EE.log so we can see what strings appear when
/// opening the relic selection screen. Call this immediately after opening the screen.
#[tauri::command]
pub(crate) fn debug_ee_log_tail() -> String {
    use std::io::{Read, Seek, SeekFrom};
    let log_path = match log_parser::watched_log_path() {
        Some(p) => p,
        None => return "Cannot find EE.log path".to_string(),
    };
    let mut f = match std::fs::File::open(&log_path) {
        Ok(f) => f,
        Err(e) => return format!("Cannot open EE.log: {}", e),
    };
    let len = f.seek(SeekFrom::End(0)).unwrap_or(0);
    let start = len.saturating_sub(4096);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return "Seek failed".to_string();
    }
    let mut buf = String::new();
    let _ = f.read_to_string(&mut buf);
    // Skip partial first line if we started mid-file
    let tail = if start > 0 {
        buf.find('\n').map(|i| &buf[i+1..]).unwrap_or(&buf)
    } else {
        &buf
    };
    tail.to_string()
}

#[derive(serde::Serialize, Clone)]
struct RelicPickReward {
    name:      String,
    rarity:    String,   // "Bronze" | "Silver" | "Gold"
    drop_rate: f64,      // relic_drop_rate(rarity, refinement) for this relic
    ducats:    u32,
    plat:      u32,
    vaulted:   bool,
    owned:     bool,
}

#[derive(serde::Serialize, Clone)]
struct RelicPickRelic {
    name:          String,   // "Lith A1 Intact"
    base_name:     String,   // "Lith A1"
    refinement:    String,   // "intact" | "exceptional" | "flawless" | "radiant"
    count:         i64,
    unowned_score: f64,
    ducat_score:   f64,
    plat_score:    f64,
    rewards:       Vec<RelicPickReward>,
}

fn relic_drop_rate(rarity: &str, refinement: &str) -> f64 {
    match (rarity, refinement) {
        ("Bronze", "intact")      => 0.2533,
        ("Bronze", "exceptional") => 0.2333,
        ("Bronze", "flawless")    => 0.20,
        ("Bronze", "radiant")     => 0.1667,
        ("Silver", "intact")      => 0.11,
        ("Silver", "exceptional") => 0.13,
        ("Silver", "flawless")    => 0.17,
        ("Silver", "radiant")     => 0.20,
        ("Gold",   "intact")      => 0.02,
        ("Gold",   "exceptional") => 0.04,
        ("Gold",   "flawless")    => 0.06,
        ("Gold",   "radiant")     => 0.10,
        _ => 0.0,
    }
}


pub(crate) fn build_relic_pick_payload(era: &str, app: &tauri::AppHandle) -> serde_json::Value {
    let state = app.state::<AppState>();
    // era_prefix matches wfcd_items display names (e.g. "Lith A1 Intact" starts with "Lith ")
    let era_prefix = match era {
        "LITH" => "Lith ",
        "MESO" => "Meso ",
        "NEO"  => "Neo ",
        "AXI"  => "Axi ",
        "ALL"  => "",
        _      => return serde_json::json!({ "era": era, "relics": [] }),
    };

    let quantities    = state.current_quantities.lock().unwrap_or_else(|e| e.into_inner()).clone();
    // relic_rewards is now keyed by display name ("Lith A1 Intact") after the wfcd.rs fix.
    let relic_rewards = state.relic_rewards.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let wfcd_items    = state.wfcd_items.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let plat_prices   = state.relics_run_prices.lock().unwrap_or_else(|e| e.into_inner()).clone();

    info!("relic-pick payload: quantities={} relic_rewards={} wfcd_items={} plat_prices={}",
          quantities.len(), relic_rewards.len(), wfcd_items.len(), plat_prices.len());

    let ducat_map: HashMap<String, u32> = wfcd_items.iter()
        .filter_map(|item| item.ducats.map(|d| (item.name.to_lowercase(), d)))
        .collect();

    let vaulted_map: HashMap<String, bool> = wfcd_items.iter()
        .filter_map(|item| item.vaulted.map(|v| (item.name.to_lowercase(), v)))
        .collect();

    // quantities is keyed by Lotus paths, not display names.
    // Build display-name → Lotus path for direct lookup.
    let name_to_unique: HashMap<String, String> = wfcd_items.iter()
        .map(|item| (item.name.to_lowercase(), item.unique_name.clone()))
        .collect();

    // Build reverse of recipes: component display name (lower) → parent display names.
    // Used to detect "owned" when the part was consumed crafting the parent (e.g. built Daikyu Prime).
    let recipes_lock = state.recipes.lock().unwrap_or_else(|e| e.into_inner());
    let mut comp_to_parents: HashMap<String, Vec<String>> = HashMap::new();
    for (parent_name, components) in recipes_lock.iter() {
        for comp in components {
            comp_to_parents
                .entry(comp.name.to_lowercase())
                .or_default()
                .push(parent_name.clone());
        }
    }
    drop(recipes_lock);

    // Returns true if the item itself OR any parent item (built result) is in inventory.
    let is_owned = |item_name: &str| -> bool {
        let key = item_name.to_lowercase();
        let direct = name_to_unique.get(&key)
            .and_then(|uname| quantities.get(uname))
            .is_some_and(|&q| q > 0);
        if direct { return true; }
        comp_to_parents.get(&key).is_some_and(|parents| {
            parents.iter().any(|p| {
                name_to_unique.get(&p.to_lowercase())
                    .and_then(|uname| quantities.get(uname))
                    .is_some_and(|&q| q > 0)
            })
        })
    };

    // Refinement suffixes as they appear in display names (capitalised).
    const REFINEMENTS: &[(&str, &str)] = &[
        ("Intact",      "intact"),
        ("Exceptional", "exceptional"),
        ("Flawless",    "flawless"),
        ("Radiant",     "radiant"),
    ];

    // Iterate wfcd_items for the relic catalog.
    // unique_name ("/Lotus/Upgrades/Relics/...") matches current_quantities keys from the blob.
    let mut relics: Vec<RelicPickRelic> = wfcd_items.iter()
        .filter(|item| item.category == "Relics")
        .filter(|item| era_prefix.is_empty() || item.name.starts_with(era_prefix))
        .filter_map(|item| {
            let count = *quantities.get(&item.unique_name).unwrap_or(&0);
            if count <= 0 { return None; }

            let (suffix_cap, refinement) = REFINEMENTS.iter()
                .find(|(cap, _)| item.name.ends_with(cap))?;
            let refinement = refinement.to_string();
            let base_name  = item.name[..item.name.len() - suffix_cap.len() - 1].to_string();

            // Rewards keyed by display name in relic_rewards (after wfcd.rs fix).
            let reward_list: Vec<RelicPickReward> = relic_rewards
                .get(&item.name)
                .map(|rewards| rewards.iter().map(|r| {
                    let key       = r.name.to_lowercase();
                    let drop_rate = relic_drop_rate(&r.rarity, &refinement);
                    let ducats    = ducat_map.get(&key).copied().unwrap_or(0);
                    let plat      = plat_prices.get(&key).copied().unwrap_or(0);
                    let vaulted   = vaulted_map.get(&key).copied().unwrap_or(false);
                    let owned     = is_owned(&r.name);
                    RelicPickReward { name: r.name.clone(), rarity: r.rarity.clone(), drop_rate, ducats, plat, vaulted, owned }
                }).collect())
                .unwrap_or_default();

            let unowned_score: f64 = reward_list.iter()
                .filter(|r| !r.owned)
                .map(|r| r.drop_rate)
                .sum();
            let ducat_score: f64 = reward_list.iter()
                .map(|r| r.drop_rate * r.ducats as f64)
                .sum();
            let plat_score: f64 = reward_list.iter()
                .map(|r| r.drop_rate * r.plat as f64)
                .sum();

            Some(RelicPickRelic {
                name: item.name.clone(), base_name, refinement, count,
                unowned_score, ducat_score, plat_score, rewards: reward_list,
            })
        })
        .collect();

    relics.sort_by(|a, b| b.ducat_score.partial_cmp(&a.ducat_score).unwrap_or(std::cmp::Ordering::Equal));
    info!("relic-pick payload: {} relics built for era={}", relics.len(), era);
    serde_json::json!({ "era": era, "relics": relics })
}

/// Hide an overlay window without destroying it. Moving it off-screen instead
/// does not work: KWin keeps windows inside the desktop and clamps a negative
/// position back to 0, leaving a transparent overlay over the primary monitor.
pub(crate) fn park_overlay_offscreen(app: &tauri::AppHandle, label: &str) {
    use tauri::Manager;
    let Some(win) = app.get_webview_window(label) else { return };
    let _ = win.hide();
}

/// Reposition the pre-declared relic-overlay window and bring it on screen.
/// The overlay is pre-declared in tauri.conf.json so its webview initialises
/// at app startup. Never created/destroyed — just shown and hidden.
#[tauri::command]
pub(crate) fn show_overlay_window(
    app: tauri::AppHandle,
    x: i32, y: i32, w: u32, h: u32,
) -> Result<(), String> {
    use tauri::Manager;
    let win = app.get_webview_window("relic-overlay")
        .ok_or_else(|| "relic-overlay window not found".to_string())?;
    let _ = win.set_size(tauri::Size::Physical(
        tauri::PhysicalSize { width: w, height: h }
    ));
    let _ = win.set_position(tauri::Position::Physical(
        tauri::PhysicalPosition { x, y }
    ));
    let _ = win.show();
    let _ = win.set_always_on_top(true);
    // The move above is only honoured on a later turn of the GTK main loop, and
    // the window manager owns the window's position until then. Ask X directly
    // so the band is on the game's monitor for its first frame rather than a
    // second later.
    overlay_linux::place(&win, x, y, w, h);

    // The webview may still be on about:blank if the window started hidden;
    // navigate it to the overlay URL now.
    if let Ok(url) = win.url() {
        if url.as_str() == "about:blank" || url.as_str().starts_with("about:") {
            debug!(%url, "deferred load detected, navigating to overlay URL");
            let overlay_url = if cfg!(debug_assertions) {
                "http://localhost:1420/index.html?overlay"
            } else {
                "tauri://localhost/index.html?overlay"
            };
            if let Ok(nav_url) = tauri::Url::parse(overlay_url) {
                let _ = win.navigate(nav_url);
            }
        }
    }

    Ok(())
}

/// Hide the relic-overlay window (visual "close" without destroying it).
#[tauri::command]
pub(crate) fn move_overlay_offscreen(app: tauri::AppHandle) -> Result<(), String> {
    park_overlay_offscreen(&app, "relic-overlay");
    Ok(())
}

/// Pull and clear the last locked relic reward payload { items, positions }.
/// Overlay.tsx calls this on mount so it never misses rewards that arrived before
/// its relic-rewards listener was registered (the tauri://created → React mount gap).
#[tauri::command]
pub(crate) fn get_pending_relic_rewards(state: State<'_, AppState>) -> Option<serde_json::Value> {
    state.pending_relic_rewards.lock().ok()?.take()
}
