use std::collections::HashMap;
use std::sync::atomic::Ordering;

use tauri::{Emitter, Manager, State};

use crate::app_state::AppState;
use crate::db::QuantityChange;
use crate::inventory_state::{is_unique_path, load_inventory_state_cache};
use crate::memory_scanner;

#[derive(serde::Serialize, Clone)]
pub(crate) struct CraftingJob {
    pub unique_name: String,
    pub item_name: String,
    pub completion_ms: i64,
}

#[derive(serde::Serialize, Clone)]
pub(crate) struct BlobStatusPayload {
    pub stage: String, // "scanning" | "done" | "error"
    pub detail: String, // human-readable detail
}

#[derive(serde::Serialize, Clone)]
pub(crate) struct InventoryUpdate {
    pub quantities: HashMap<String, i64>,
    pub crafting: Vec<CraftingJob>,
    pub mastery_rank: Option<u32>,
    pub mastery_data: HashMap<String, u32>,
    pub changes: Vec<QuantityChange>,
    pub warframe_running: bool,
    pub scanned_at: i64,
    /// Warframe unique-name paths from InfestedFoundry.ConsumedSuits (Helminth subsumed).
    /// Non-empty only when the memory scanner found the ConsumedSuits array this window.
    pub consumed_suits: Vec<String>,
    /// Mod/arcane inventory: unique_name → {total, by_rank}.
    /// Empty when no scan data available yet; scanner-sourced until API provides rank detail.
    pub mods: HashMap<String, crate::memory_scanner::ModCount>,
    /// Warframe unique-name → socketed Archon Shards read from memory.
    /// Only populated for warframes where ArchonCrystalUpgrades was found.
    pub socketed_shards: HashMap<String, Vec<crate::memory_scanner::ArchonShard>>,
    /// Item unique-name → number of Forma applied (polarized count from blob).
    /// Only populated for items that have at least one Forma applied.
    pub forma_counts: HashMap<String, u32>,
    /// True only on the end-of-full-pass emit. Frontend should REPLACE archonShards
    /// state instead of merging so stale entries are cleaned up.
    pub is_full_pass: bool,
    /// Local Warframe account name ("Logged in NAME" from EE.log). None until detected.
    pub player_name: Option<String>,
}

#[tauri::command]
pub(crate) fn stop_monitor(state: State<AppState>) {
    state.monitor_active.store(false, Ordering::SeqCst);
}

#[tauri::command]
pub(crate) fn poke_scan(state: State<AppState>) {
    state.force_pid_check.store(true, Ordering::SeqCst);
}

#[tauri::command]
pub(crate) fn set_relic_pick_enabled(state: State<AppState>, enabled: bool) {
    state.relic_pick_overlay_enabled.store(enabled, Ordering::SeqCst);
}

#[tauri::command]
pub(crate) fn set_mem_trigger_enabled(state: State<AppState>, enabled: bool) {
    state.mem_trigger_enabled.store(enabled, Ordering::SeqCst);
}

#[tauri::command]
pub(crate) fn get_monitor_status(state: State<AppState>) -> bool {
    state.monitor_active.load(Ordering::SeqCst)
}

/// Scan the process heap for a live EE.log trigger string.
///
/// The full scan caches the static-string address; later scans use the narrow,
/// writable-only range around it to avoid scanning read-only .rodata repeatedly.
pub(crate) fn scan_heap_for_trigger(
    pid: u32,
    pat: &[u8],
    cached_bare: Option<u64>,
) -> (bool, String, Option<u64>) {
    use crate::platform::{Platform, ProcessAccess};

    const FULL_MIN: u64 = 0x0000_0001_0000_0000; // 4 GB
    const FULL_MAX: u64 = 0x0000_8000_0000_0000; // 512 TB (covers DLL image range)
    const NARROW_R: u64 = 128 * 1024 * 1024; // +/-128 MB around bare_hit
    const REGION_MAX: usize = 32 * 1024 * 1024; // skip regions > 32 MB

    let bare_pat = &pat[..pat.len().saturating_sub(1)];
    let (scan_min, scan_max, rw_only) = match cached_bare {
        Some(ba) => (ba.saturating_sub(NARROW_R), ba.saturating_add(NARROW_R), true),
        None => (FULL_MIN, FULL_MAX, false),
    };

    let handle = match Platform::open_process(pid) {
        Ok(h) => h,
        Err(e) => return (false, format!("OpenProcess failed pid={}: {}", pid, e), cached_bare),
    };

    let mut found = false;
    let mut regions_read = 0u32;
    let mut bare_hit: Option<u64> = None;
    let mut addr = scan_min as usize;
    let t = std::time::Instant::now();

    loop {
        if addr >= scan_max as usize { break; }

        let regions: Vec<_> = handle.regions_from(addr).collect();
        if regions.is_empty() { break; }

        for region in &regions {
            let base = region.base_address;
            let size = region.region_size;
            addr = base + size;

            if base < scan_min as usize || base >= scan_max as usize || !region.is_committed || !region.is_readable {
                continue;
            }
            if rw_only && !region.is_writable { continue; }
            if size > REGION_MAX { continue; }

            let buf = match handle.read(base, size) {
                Some(r) => r,
                None => continue,
            };
            if buf.is_empty() { continue; }

            regions_read += 1;
            if bare_hit.is_none() {
                if let Some(off) = buf.windows(bare_pat.len()).position(|w| w == bare_pat) {
                    bare_hit = Some(base as u64 + off as u64);
                }
            }
            if buf.windows(pat.len()).any(|w| w == pat) {
                found = true;
                break;
            }
        }
        if found { break; }
    }

    let mode = if cached_bare.is_some() { "narrow" } else { "full" };
    let diag = format!(
        "{} scan in {}ms: {} regions, bare={}, live={}",
        mode,
        t.elapsed().as_millis(),
        regions_read,
        bare_hit.map_or("none".to_string(), |a| format!("{:#x}", a)),
        found
    );
    (found, diag, if cached_bare.is_none() { bare_hit } else { cached_bare })
}

/// Start the memory-based relic reward trigger alongside the EE.log watcher.
pub(crate) fn start_memory_trigger(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let session_log = std::env::temp_dir().join("frameforge_overlay_session.txt");
        let mut was_open = false;
        let mut open_at: Option<std::time::Instant> = None;
        // Include \r so we only match live EE.log ring-buffer entries
        // (Windows line ending: \r\n). The static .rodata copy ends with \n\0.
        const OPEN_PAT: &[u8] = b"VoidProjections: GetVoidProjectionRewards\r";
        // 200 ms is fine in narrow mode (each scan < 10 ms).
        // The first scan (full mode, ~30 s) will block here once per game session.
        const POLL_MS: u64 = 200;
        // Auto-reset after 90 s regardless (reward screen max duration).
        const AUTO_RESET_SECS: u64 = 90;

        // Two-phase scan state. Reset when the game PID changes (ASLR re-randomizes).
        let mut cached_bare: Option<u64> = None;
        let mut last_pid: u32 = 0;

        loop {
            std::thread::sleep(std::time::Duration::from_millis(POLL_MS));

            let state = app.state::<AppState>();
            if !state.monitor_active.load(Ordering::SeqCst) {
                break;
            }
            if !state.mem_trigger_enabled.load(Ordering::SeqCst) {
                was_open = false;
                open_at = None;
                continue;
            }

            // Auto-reset open state after the max reward window duration.
            if was_open {
                if open_at.is_some_and(|t| t.elapsed().as_secs() >= AUTO_RESET_SECS) {
                    was_open = false;
                    open_at = None;
                } else {
                    continue;
                }
            }

            let pid = match crate::memory_scanner::find_warframe_pid_pub() {
                Some(p) => p,
                None => {
                    was_open = false;
                    open_at = None;
                    cached_bare = None;
                    last_pid = 0;
                    continue;
                }
            };
            // Game restart → ASLR changed all addresses; start over with a full scan.
            if pid != last_pid {
                cached_bare = None;
                last_pid = pid;
            }

            let (found, diag, new_bare) = scan_heap_for_trigger(pid, OPEN_PAT, cached_bare);
            // Promote bare_hit from a full scan; preserve across narrow scans.
            if cached_bare.is_none() {
                cached_bare = new_bare;
            }

            let ts = chrono::Local::now().format("%H:%M:%S%.3f");
            let _ = std::fs::OpenOptions::new()
                .append(true)
                .open(&session_log)
                .and_then(|mut f| {
                    use std::io::Write;
                    writeln!(f, "[MEM SCAN] @ {} — {}", ts, diag)
                });
            if found {
                was_open = true;
                open_at = Some(std::time::Instant::now());
                let _ = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&session_log)
                    .and_then(|mut f| {
                        use std::io::Write;
                        writeln!(f, "[MEM TRIGGER] Open detected @ {}", ts)
                    });
                let _ = app.emit("ff-status", "🔍 [MEM] Relic reward screen detected");
                let _ = app.emit("relic-trigger", ());
            }
        }
    });
}

pub(crate) fn start_legacy_reward_worker(
    monitor_active: std::sync::Arc<std::sync::atomic::AtomicBool>,
    debug_path: std::path::PathBuf,
    last_found_path: std::path::PathBuf,
) {
    std::thread::spawn(move || {
        // Initialize COM (required for Windows OCR / WinRT APIs).
        // std::thread::spawn creates a raw OS thread with no COM apartment;
        // WinRT calls silently fail without this, returning empty strings.
        <crate::platform::Platform as crate::platform::ComInit>::initialize_com();

        while monitor_active.load(Ordering::SeqCst) {
            let _relic_screen = false;
            let mut debug = String::new();
            let ts = chrono::Local::now().format("%H:%M:%S%.3f");
            debug.push_str(&format!("=== {} ===\n", ts));

            // OCR is now triggered by the EE.log watcher (AlecaFrame-style),
            // not by this polling loop. This loop only handles inventory scanning.
            let rewards: Option<serde_json::Value> = None;

            let _ = std::fs::write(&debug_path, &debug);
            if rewards.is_some() {
                let _ = std::fs::write(&last_found_path, &debug);
            }

            // Overlay is controlled entirely by the EE.log watcher — do NOT emit here.
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    });
}

// ── Monitor catalog ───────────────────────────────────────────────────────────

pub(crate) struct MonitorCatalog {
    pub path_to_name: HashMap<String, String>,
    pub path_to_ducat: HashMap<String, u32>,
    pub path_to_vaulted: HashMap<String, bool>,
    pub path_to_tradable: HashMap<String, bool>,
    pub path_to_masterable: HashMap<String, bool>,
    pub path_to_category: HashMap<String, String>,
    pub path_to_item_type: HashMap<String, String>,
    pub path_to_product_category: HashMap<String, String>,
    pub path_to_wfcd_cat: HashMap<String, String>,
    pub alias_excluded: std::collections::HashSet<String>,
    pub ignored_paths: std::collections::HashSet<String>,
    pub stackable_paths: std::collections::HashSet<String>,
    pub path_aliases: HashMap<String, String>,
    pub unique_names: Vec<String>,
    pub display_names: Vec<String>,
    pub relic_drops_snapshot: HashMap<String, Vec<String>>,
}

pub(crate) fn build_monitor_catalog(
    wfcd_items: &[crate::wfcd::WfcdItem],
    corrections: &std::collections::HashMap<String, crate::app_state::CorrectionEntry>,
    relic_drops: &std::collections::HashMap<String, Vec<String>>,
) -> MonitorCatalog {
    let mut unique_names: Vec<String> = wfcd_items.iter().map(|i| i.unique_name.clone()).collect();
    let mut display_names: Vec<String> = wfcd_items.iter().map(|i| i.name.clone()).collect();

    for (path, name) in [
        ("/_currency/Endo",         "Endo"),
        ("/_currency/Credits",      "Credits"),
        ("/_currency/Platinum",     "Platinum"),
        ("/_currency/PlatinumGift", "Platinum (Gift)"),
    ] {
        unique_names.push(path.to_string());
        display_names.push(name.to_string());
    }

    let path_aliases: HashMap<String, String> = [
        ("/Lotus/Powersuits/SiriusOrion/OrionSuit".to_string(),
         "/Lotus/Powersuits/SiriusOrion/SiriusSuit".to_string()),
        ("/Lotus/Powersuits/SiriusOrion/OrionSuitBlueprint".to_string(),
         "/Lotus/Types/Recipes/WarframeRecipes/SiriusOrionBlueprint".to_string()),
    ].into_iter().collect();

    let mut alias_excluded: std::collections::HashSet<String> =
        path_aliases.keys().cloned().collect();

    let mut path_to_name: HashMap<String, String> = unique_names.iter().zip(display_names.iter())
        .map(|(u, d)| (u.clone(), d.clone()))
        .collect();
    for (alt, primary) in &path_aliases {
        if let Some(name) = path_to_name.get(primary).cloned() {
            path_to_name.insert(alt.clone(), name);
        }
    }

    let path_to_ducat: HashMap<String, u32> = wfcd_items.iter()
        .filter_map(|i| i.ducats.map(|d| (i.unique_name.clone(), d)))
        .collect();
    let path_to_vaulted: HashMap<String, bool> = wfcd_items.iter()
        .filter_map(|i| i.vaulted.map(|v| (i.unique_name.clone(), v)))
        .collect();
    let path_to_tradable: HashMap<String, bool> = wfcd_items.iter()
        .filter_map(|i| i.tradable.map(|t| (i.unique_name.clone(), t)))
        .collect();
    let path_to_masterable: HashMap<String, bool> = wfcd_items.iter()
        .filter_map(|i| i.masterable.map(|m| (i.unique_name.clone(), m)))
        .collect();
    let path_to_item_type: HashMap<String, String> = wfcd_items.iter()
        .map(|i| (i.unique_name.clone(), i.item_type.clone())).collect();
    let path_to_product_category: HashMap<String, String> = wfcd_items.iter()
        .map(|i| (i.unique_name.clone(), i.product_category.clone())).collect();
    let path_to_wfcd_cat: HashMap<String, String> = wfcd_items.iter()
        .map(|i| (i.unique_name.clone(), i.category.clone())).collect();
    let mut path_to_category: HashMap<String, String> = wfcd_items.iter()
        .map(|i| (i.unique_name.clone(), crate::catalogue::fix_category(
            &i.name, &i.item_type, &i.product_category, &i.category, &i.unique_name)))
        .collect();

    for (path, name) in [
        ("/_currency/Endo",         "Endo"),
        ("/_currency/Credits",      "Credits"),
        ("/_currency/Platinum",     "Platinum"),
        ("/_currency/PlatinumGift", "Platinum (Gift)"),
    ] {
        path_to_name.insert(path.to_string(), name.to_string());
        path_to_category.insert(path.to_string(), "Miscellaneous".to_string());
    }

    let ignored_paths: std::collections::HashSet<String> = corrections.iter()
        .filter(|(_, c)| c.category.as_deref() == Some("Ignored"))
        .map(|(path, _)| path.clone())
        .collect();
    let stackable_paths: std::collections::HashSet<String> = corrections.iter()
        .filter(|(_, c)| c.is_stackable == Some(true))
        .map(|(path, _)| path.clone())
        .collect();

    for p in &ignored_paths {
        path_to_name.remove(p);
        path_to_category.remove(p);
    }
    for (path, c) in corrections {
        if ignored_paths.contains(path) { continue; }
        if let Some(ref name) = c.name {
            if !name.is_empty() { path_to_name.insert(path.clone(), name.clone()); }
        }
        if let Some(ref cat) = c.category {
            path_to_category.insert(path.clone(), cat.clone());
        }
    }
    alias_excluded.extend(ignored_paths.iter().cloned());

    MonitorCatalog {
        path_to_name, path_to_ducat, path_to_vaulted,
        path_to_tradable, path_to_masterable, path_to_category,
        path_to_item_type, path_to_product_category, path_to_wfcd_cat,
        alias_excluded, ignored_paths, stackable_paths, path_aliases,
        unique_names, display_names,
        relic_drops_snapshot: relic_drops.clone(),
    }
}

// ── Blob-to-state application ────────────────────────────────────────────────

pub(crate) struct InventoryState<'a> {
    pub known: &'a mut HashMap<String, i64>,
    pub unique_stable: &'a mut HashMap<String, u8>,
    pub confirmed_unique: &'a mut std::collections::HashSet<String>,
    pub known_mods: &'a mut HashMap<String, memory_scanner::ModCount>,
    pub current_socketed_shards: &'a mut HashMap<String, Vec<memory_scanner::ArchonShard>>,
    pub current_forma_counts: &'a mut HashMap<String, u32>,
    pub current_mastery_rank: &'a mut Option<u32>,
    pub current_mastery_data: &'a mut HashMap<String, u32>,
    pub current_consumed_suits: &'a mut Vec<String>,
    pub current_recipes: &'a mut Vec<memory_scanner::PendingRecipe>,
}

pub(crate) fn apply_blob_to_state(
    blob: &memory_scanner::BlobInventory,
    state: &mut InventoryState<'_>,
    path_aliases: &HashMap<String, String>,
    stackable_paths: &std::collections::HashSet<String>,
) {
    // Blob is authoritative — full replacement, not a merge.
    state.known.clear();

    // Currency
    state.known.insert("/_currency/Credits".to_string(),      blob.credits);
    state.known.insert("/_currency/Endo".to_string(),         blob.endo);
    state.known.insert("/_currency/Platinum".to_string(),     blob.platinum - blob.free_platinum);
    state.known.insert("/_currency/PlatinumGift".to_string(), blob.free_platinum);

    // Stackable items
    for entry in &blob.stackable_items {
        state.known.insert(entry.item_type.clone(), entry.item_count);
    }

    // Unique items — full replacement
    state.unique_stable.clear();
    state.confirmed_unique.clear();
    state.current_socketed_shards.clear();
    state.current_forma_counts.clear();
    for entry in &blob.unique_items {
        let canonical = path_aliases.get(entry.item_type.as_str())
            .cloned()
            .unwrap_or_else(|| entry.item_type.clone());
        if blob.consumed_suits.contains(&canonical) { continue; }
        if stackable_paths.contains(&canonical) {
            *state.known.entry(canonical).or_insert(0) += 1;
            continue;
        }
        state.unique_stable.insert(canonical.clone(), 4);
        state.confirmed_unique.insert(canonical.clone());
        if !entry.archon_shards.is_empty() {
            state.current_socketed_shards.insert(canonical.clone(), entry.archon_shards.clone());
        }
        if entry.polarized > 0 {
            state.current_forma_counts.insert(canonical, entry.polarized);
        }
    }

    // Mods — full replacement
    state.known_mods.clear();
    for (path, mc) in &blob.mods {
        state.known_mods.insert(path.clone(), mc.clone());
    }
    // Rivens — group by item_type so they appear in inventory like regular mods
    for riven in &blob.rivens {
        let mc = state.known_mods.entry(riven.item_type.clone()).or_default();
        mc.total += riven.count as i64;
        *mc.by_rank.entry(riven.mod_rank).or_insert(0) += riven.count as i64;
    }

    // Cosmetics (FlavourItems + WeaponSkins) — occurrence-counted, go into known
    for (path, &count) in blob.flavour_items.iter().chain(blob.weapon_skins.iter()) {
        state.known.insert(path.clone(), count);
    }

    // Meta
    *state.current_mastery_rank = Some(blob.mastery_level);
    for (path, &rank) in &blob.mastery_data {
        state.current_mastery_data.insert(path.clone(), rank);
    }
    *state.current_consumed_suits = blob.consumed_suits.clone();
    for suit in &*state.current_consumed_suits {
        state.confirmed_unique.remove(suit);
        state.unique_stable.remove(suit);
    }
    *state.current_recipes = blob.pending_recipes.iter().map(|r| memory_scanner::PendingRecipe {
        unique_name:   r.item_type.clone(),
        completion_ms: r.completion_ms,
    }).collect();
}

pub(crate) fn build_crafting_jobs(
    recipes: &[(String, i64)],
    display_names: &[String],
    unique_names: &[String],
) -> Vec<CraftingJob> {
    recipes.iter().map(|(unique_name, completion_ms)| {
        let item_name = display_names.iter().zip(unique_names.iter())
            .find(|(_, u)| **u == *unique_name)
            .map(|(d, _)| d.clone())
            .unwrap_or_else(|| unique_name.split('/').next_back().unwrap_or("?").to_string());
        CraftingJob { unique_name: unique_name.clone(), item_name, completion_ms: *completion_ms }
    }).collect()
}

pub(crate) struct MonitorStartupState {
    pub known: HashMap<String, i64>,
    pub prev_mods: HashMap<String, memory_scanner::ModCount>,
    pub unique_stable: HashMap<String, u8>,
    pub confirmed_unique: std::collections::HashSet<String>,
    pub known_mods: HashMap<String, memory_scanner::ModCount>,
    pub current_mastery_rank: Option<u32>,
    pub current_mastery_data: HashMap<String, u32>,
    pub current_consumed_suits: Vec<String>,
    pub current_socketed_shards: HashMap<String, Vec<memory_scanner::ArchonShard>>,
    pub current_forma_counts: HashMap<String, u32>,
}

pub(crate) fn init_monitor_startup_state(
    shared_quantities: &std::sync::Arc<std::sync::Mutex<HashMap<String, i64>>>,
    shared_mods: &std::sync::Arc<std::sync::Mutex<HashMap<String, memory_scanner::ModCount>>>,
    inventory_state_cache_path: &std::path::PathBuf,
) -> MonitorStartupState {
    let mut known: HashMap<String, i64> =
        shared_quantities.lock().unwrap_or_else(|e| e.into_inner()).clone();

    let startup_cache = load_inventory_state_cache(inventory_state_cache_path);

    let prev_mods: HashMap<String, memory_scanner::ModCount> = startup_cache.items.iter()
        .filter(|(_, v)| v.mod_ranks.is_some())
        .map(|(path, v)| {
            let by_rank: HashMap<u8, i64> = v.mod_ranks.as_ref()
                .map(|ranks| ranks.iter()
                    .filter_map(|(r, &c)| r.parse::<u8>().ok().map(|rank| (rank, c)))
                    .collect())
                .unwrap_or_default();
            let total = by_rank.values().sum();
            (path.clone(), memory_scanner::ModCount { total, by_rank })
        })
        .collect();

    for (path, item) in &startup_cache.items {
        if item.amount > 0 && item.mod_ranks.is_none()
            && (item.is_stackable || !is_unique_path(path))
        {
            known.entry(path.to_string()).or_insert(item.amount);
        }
    }
    {
        let mut q = shared_quantities.lock().unwrap_or_else(|e| e.into_inner());
        if q.is_empty() && !known.is_empty() { *q = known.clone(); }
    }

    let mut unique_stable: HashMap<String, u8> = startup_cache.items.iter()
        .filter(|(k, v)| v.mod_ranks.is_none() && v.amount > 0 && !v.subsumed
                      && !v.is_stackable && is_unique_path(k))
        .map(|(k, _)| (k.clone(), 4u8))
        .collect();
    let mut confirmed_unique: std::collections::HashSet<String> =
        unique_stable.keys().cloned().collect();

    let known_mods: HashMap<String, memory_scanner::ModCount> = {
        let from_shared = shared_mods.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if !from_shared.is_empty() {
            from_shared
        } else {
            startup_cache.items.iter()
                .filter(|(_, v)| v.mod_ranks.is_some())
                .map(|(path, v)| {
                    let by_rank: HashMap<u8, i64> = v.mod_ranks.as_ref()
                        .map(|ranks| ranks.iter()
                            .filter_map(|(r, &c)| r.parse::<u8>().ok().map(|rank| (rank, c)))
                            .collect())
                        .unwrap_or_default();
                    let total = by_rank.values().sum();
                    (path.clone(), memory_scanner::ModCount { total, by_rank })
                })
                .collect()
        }
    };

    let current_mastery_rank = startup_cache.mastery_rank;
    let current_mastery_data: HashMap<String, u32> = startup_cache.items.iter()
        .filter(|(_, v)| v.mastery_rank > 0)
        .map(|(k, v)| (k.clone(), v.mastery_rank))
        .collect();
    let current_consumed_suits: Vec<String> = startup_cache.consumed_suits();
    let current_socketed_shards: HashMap<String, Vec<memory_scanner::ArchonShard>> = startup_cache.items.iter()
        .filter(|(_, v)| !v.archon_shards.is_empty())
        .map(|(k, v)| (k.clone(), v.archon_shards.clone()))
        .collect();
    let current_forma_counts: HashMap<String, u32> = startup_cache.items.iter()
        .filter_map(|(k, v)| v.forma_count.map(|n| (k.clone(), n)))
        .collect();

    for suit in &current_consumed_suits {
        confirmed_unique.remove(suit);
        unique_stable.remove(suit);
    }

    MonitorStartupState {
        known, prev_mods, unique_stable, confirmed_unique, known_mods,
        current_mastery_rank, current_mastery_data, current_consumed_suits,
        current_socketed_shards, current_forma_counts,
    }
}
