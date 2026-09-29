use std::collections::HashMap;
use tracing::warn;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager, State};
use crate::app_state::AppState;
use crate::catalogue::fix_category;
use crate::db::QuantityChange;
use crate::inventory_state::{load_inventory_state_cache, inventory_path_aliases};
use crate::mastery_rules;
use crate::platform::{Platform, ProcessAccess};
use crate::{events, memory_scanner};

// ─── Live monitor ─────────────────────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
pub struct CraftingJob {
    pub unique_name: String,
    pub item_name: String,
    pub completion_ms: i64,
}

#[derive(serde::Serialize, Clone)]
pub struct BlobStatusPayload {
    pub stage:   String,  // "scanning" | "done" | "error"
    pub detail:  String,  // human-readable detail
}

#[derive(serde::Serialize, Clone)]
pub struct InventoryUpdate {
    pub quantities: HashMap<String, i64>,
    pub crafting: Vec<CraftingJob>,
    pub mastery_rank: Option<u32>,
    pub mastery_data: HashMap<String, u32>,
    pub owned_levels: HashMap<String, Vec<u32>>,
    pub changes: Vec<QuantityChange>,
    pub warframe_running: bool,
    pub scanned_at: i64,
    /// Warframe unique-name paths from InfestedFoundry.ConsumedSuits (Helminth subsumed).
    /// Non-empty only when the memory scanner found the ConsumedSuits array this window.
    pub consumed_suits: Vec<String>,
    /// Mod/arcane inventory: unique_name → {total, by_rank}.
    /// Empty when no scan data available yet; scanner-sourced until API provides rank detail.
    pub mods: HashMap<String, memory_scanner::ModCount>,
    /// Warframe unique-name → socketed Archon Shards read from memory.
    /// Only populated for warframes where ArchonCrystalUpgrades was found.
    pub socketed_shards: HashMap<String, Vec<memory_scanner::ArchonShard>>,
    /// Item unique-name → number of Forma applied (polarized count from blob).
    /// Only populated for items that have at least one Forma applied.
    pub forma_counts: HashMap<String, u32>,
    /// True only on the end-of-full-pass emit. Frontend should REPLACE archonShards
    /// state instead of merging so stale entries are cleaned up.
    pub is_full_pass: bool,
    /// Local Warframe account name ("Logged in NAME" from EE.log). None until detected.
    pub player_name: Option<String>,
}

pub(crate) fn now_hms() -> String {
    chrono::Local::now().format("%H:%M:%S%.3f").to_string()
}

pub(crate) fn append_to_file(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(text.as_bytes())
}

#[tracing::instrument(level = "debug", skip_all)]
#[tauri::command]
pub(crate) fn stop_monitor(state: State<AppState>) {
    state.monitor_active.store(false, Ordering::SeqCst);
}

#[tracing::instrument(level = "debug", skip_all)]
#[tauri::command]
pub(crate) fn poke_scan(state: State<AppState>) {
    state.force_pid_check.store(true, Ordering::SeqCst);
}

#[tracing::instrument(level = "debug", skip_all)]
#[tauri::command]
pub(crate) fn set_relic_pick_enabled(state: State<AppState>, enabled: bool) {
    state.relic_pick_overlay_enabled.store(enabled, Ordering::SeqCst);
}

#[tracing::instrument(level = "debug", skip_all)]
#[tauri::command]
pub(crate) fn set_mem_trigger_enabled(state: State<AppState>, enabled: bool) {
    state.mem_trigger_enabled.store(enabled, Ordering::SeqCst);
}

#[tracing::instrument(level = "debug", skip_all)]
#[tauri::command]
pub(crate) fn get_monitor_status(state: State<AppState>) -> bool {
    state.monitor_active.load(Ordering::SeqCst)
}

// ── Monitor catalog ───────────────────────────────────────────────────────────

pub(crate) struct MonitorCatalog {
    pub path_to_name: HashMap<String, String>,
    pub path_to_ducat: HashMap<String, u32>,
    pub path_to_vaulted: HashMap<String, bool>,
    pub path_to_tradable: HashMap<String, bool>,
    pub path_to_masterable: HashMap<String, bool>,
    pub path_to_max_level_cap: HashMap<String, u32>,
    pub path_to_category: HashMap<String, String>,
    pub path_to_item_type: HashMap<String, String>,
    pub path_to_product_category: HashMap<String, String>,
    pub path_to_wfcd_cat: HashMap<String, String>,
    pub alias_excluded: std::collections::HashSet<String>,
    pub ignored_paths: std::collections::HashSet<String>,
    pub unique_names: Vec<String>,
    pub display_names: Vec<String>,
    pub relic_drops_snapshot: HashMap<String, Vec<String>>,
}

pub(crate) fn build_monitor_catalog(
    wfcd_items: &[crate::wfcd::WfcdItem],
    corrections: &HashMap<String, crate::app_state::CorrectionEntry>,
    relic_drops: &HashMap<String, Vec<String>>,
) -> MonitorCatalog {
    let mut unique_names: Vec<String> = wfcd_items.iter().map(|i| i.unique_name.clone()).collect();
    let mut display_names: Vec<String> = wfcd_items.iter().map(|i| i.name.clone()).collect();
    // Virtual catalog entries for currency fields not present in WFCD.
    for (path, name) in [
        ("/_currency/Endo",        "Endo"),
        ("/_currency/Credits",     "Credits"),
        ("/_currency/Platinum",    "Platinum"),
        ("/_currency/PlatinumGift","Platinum (Gift)"),
    ] {
        unique_names.push(path.to_string());
        display_names.push(name.to_string());
    }
    // Items that share a game path with a canonical counterpart (dual-body warframes,
    // renamed items, etc.).  Map  secondary_path → primary_path.
    // The scanner searches for ALL paths, but stores results under the primary so the
    // inventory shows one entry with the canonical display name.
    let path_aliases = inventory_path_aliases();

    // Alias keys (secondary paths) are excluded from the inventory cache entirely —
    // they would show as phantom zero-quantity duplicates of the canonical entry.
    let mut alias_excluded: std::collections::HashSet<String> =
        path_aliases.keys().map(|s| s.to_string()).collect();

    // Build path→name and path→ducat lookups once from the catalog snapshot.
    // Alternate paths in path_aliases resolve to the canonical name.
    let mut path_to_name: HashMap<String, String> = unique_names.iter().zip(display_names.iter())
        .map(|(u, d)| (u.clone(), d.clone()))
        .collect();
    for (alt, primary) in &path_aliases {
        if let Some(name) = path_to_name.get(*primary).cloned() {
            path_to_name.insert(alt.to_string(), name);
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
    let mut path_to_max_level_cap: HashMap<String, u32> = wfcd_items.iter()
        .filter_map(|i| mastery_rules::known_cap(corrections.get(&i.unique_name), i.max_level_cap)
            .map(|cap| (i.unique_name.clone(), cap)))
        .collect();
    let mut path_to_masterable: HashMap<String, bool> = wfcd_items.iter()
        .filter_map(|i| mastery_rules::masterable(corrections.get(&i.unique_name), i.masterable, &i.unique_name)
            .map(|m| (i.unique_name.clone(), m)))
        .collect();
    // Owned maps for debug capture — cloned once, no borrow from `items`.
    let path_to_item_type: HashMap<String, String> = wfcd_items.iter()
        .map(|i| (i.unique_name.clone(), i.item_type.clone())).collect();
    let path_to_product_category: HashMap<String, String> = wfcd_items.iter()
        .map(|i| (i.unique_name.clone(), i.product_category.clone())).collect();
    let path_to_wfcd_cat: HashMap<String, String> = wfcd_items.iter()
        .map(|i| (i.unique_name.clone(), i.category.clone())).collect();
    let mut path_to_category: HashMap<String, String> = wfcd_items.iter()
        .map(|i| (i.unique_name.clone(), fix_category(&i.name, &i.item_type, &i.product_category, &i.category, &i.unique_name)))
        .collect();
    for (path, name) in [
        ("/_currency/Endo",        "Endo"),
        ("/_currency/Credits",     "Credits"),
        ("/_currency/Platinum",    "Platinum"),
        ("/_currency/PlatinumGift","Platinum (Gift)"),
    ] {
        path_to_name.insert(path.to_string(), name.to_string());
        path_to_category.insert(path.to_string(), "Miscellaneous".to_string());
    }

    // ── Apply corrections to path lookups ─────────────────────────────────────
    let ignored_paths: std::collections::HashSet<String> = corrections.iter()
        .filter(|(_, c)| c.category.as_deref() == Some("Ignored"))
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
        // The Plexus has no WFCD entry, so its table row is all the rules see.
        if !path_to_item_type.contains_key(path) {
            if let Some(masterable) = mastery_rules::masterable(Some(c), None, path) {
                path_to_masterable.insert(path.clone(), masterable);
            }
            if let Some(cap) = mastery_rules::known_cap(Some(c), None) {
                path_to_max_level_cap.insert(path.clone(), cap);
            }
        }
    }
    // Ignored paths are suppressed from the inventory cache just like alias secondaries.
    alias_excluded.extend(ignored_paths.iter().cloned());


    MonitorCatalog {
        path_to_name, path_to_ducat, path_to_vaulted, path_to_tradable,
        path_to_masterable, path_to_max_level_cap, path_to_category,
        path_to_item_type, path_to_product_category, path_to_wfcd_cat,
        alias_excluded, ignored_paths, unique_names, display_names,
        relic_drops_snapshot: relic_drops.clone(),
    }
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

/// Holds the inventory as last applied. The blob capture loop seeds it from the
/// previous session's cache and updates it with every blob.
pub(crate) struct MonitorStartupState {
    pub known: HashMap<String, i64>,
    pub unique_quantities: HashMap<String, i64>,
    pub known_mods: HashMap<String, memory_scanner::ModCount>,
    pub prev_mods: HashMap<String, memory_scanner::ModCount>,
    pub current_mastery_rank: Option<u32>,
    pub current_mastery_data: HashMap<String, u32>,
    pub current_owned_levels: HashMap<String, Vec<u32>>,
    pub current_recipes: Vec<memory_scanner::PendingRecipe>,
    pub current_consumed_suits: Vec<String>,
    pub current_socketed_shards: HashMap<String, Vec<memory_scanner::ArchonShard>>,
    pub current_forma_counts: HashMap<String, u32>,
    pub last_snapshot_date: String,
}

pub(crate) fn init_monitor_startup_state(
    shared_quantities: &Arc<Mutex<HashMap<String, i64>>>,
    shared_mods: &Arc<Mutex<HashMap<String, memory_scanner::ModCount>>>,
    inventory_state_cache_path: &std::path::PathBuf,
) -> MonitorStartupState {
    // Start from whatever quantities were last known (survives restarts).
    let mut known: HashMap<String, i64> =
        shared_quantities.lock().unwrap_or_else(|e| e.into_inner()).clone();

    // Load the full inventory state from the last session so the UI shows data
    // immediately on restart without waiting for the first full scan pass.
    let startup_cache = load_inventory_state_cache(inventory_state_cache_path);

    for (path, amount) in startup_cache.stackable_quantities() {
        known.entry(path).or_insert(amount);
    }
    // Keep shared_quantities in sync so the cache-clear detector doesn't misfire.
    {
        let mut q = shared_quantities.lock().unwrap_or_else(|e| e.into_inner());
        if q.is_empty() && !known.is_empty() { *q = known.clone(); }
    }

    let unique_quantities = startup_cache.unique_quantities();

    // Mods: commit hint results directly on every partial pass.
    // The hint is the live inventory-root region and is always authoritative.
    // No stability buffer needed — wrong counts on a bad scan self-correct next pass.
    // Pre-seed from startup cache so mods/arcanes show immediately on restart instead
    // of going blank until the hint scan rediscovers the RawUpgrades region.
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
    let prev_mods: HashMap<String, memory_scanner::ModCount> = known_mods.clone();
    // Track the last date we recorded daily snapshots (YYYY-MM-DD).
    // Initialise to yesterday so the first scan of a new day always fires.
    let last_snapshot_date = String::new();
    let current_mastery_rank: Option<u32> = startup_cache.mastery_rank;
    let current_mastery_data: HashMap<String, u32> = startup_cache.mastery_data();
    let current_owned_levels = startup_cache.owned_levels();
    let current_recipes: Vec<memory_scanner::PendingRecipe> = Vec::new();
    let current_consumed_suits: Vec<String> = startup_cache.consumed_suits();
    let current_socketed_shards: HashMap<String, Vec<memory_scanner::ArchonShard>> = startup_cache.items.iter()
        .filter(|(_, v)| !v.archon_shards.is_empty())
        .map(|(k, v)| (k.clone(), v.archon_shards.clone()))
        .collect();
    let current_forma_counts: HashMap<String, u32> = startup_cache.items.iter()
        .filter_map(|(k, v)| v.forma_count.map(|n| (k.clone(), n)))
        .collect();

    MonitorStartupState {
        known, unique_quantities, known_mods, prev_mods,
        current_mastery_rank, current_mastery_data, current_owned_levels,
        current_recipes, current_consumed_suits, current_socketed_shards,
        current_forma_counts, last_snapshot_date,
    }
}

/// Start the memory-based relic reward trigger alongside the EE.log watcher.
pub(crate) fn start_memory_trigger(app: tauri::AppHandle) {
    // ── Memory trigger thread ────────────────────────────────────────────────
    // Parallel to the EE.log watcher. When mem_trigger_enabled is true this thread
    // polls Warframe's heap memory every second, searching for the same trigger
    // strings EE.log uses. Because the EE.log ring buffer is written before the
    // file is flushed to disk, memory detection can fire the overlay pre-creation
    // event (relic-trigger) earlier than EE.log polling would.
    //
    // What it does:
    //  • Scans small committed heap regions (≤ 2 MB) in address range
    //    0x0001_0000_0000–0x0000_7FF0_0000_0000 for the OPEN trigger string.
    //    Live ring-buffer entries end with \r so we can distinguish them from
    //    the static .rodata copy (which ends with \n\0).
    //  • On open detected → emits relic-trigger + appends timing to session log.
    //  • Auto-resets after 90 s (reward screen max duration).
    //  • Close is still handled by the EE.log watcher (no change there).
    //
    // OCR itself is always started by the EE.log watcher; this thread only races
    // for the overlay pre-creation event. Toggle on/off from Settings.
    {
        let mt_app   = app;
        std::thread::spawn(move || {
            // Inner helper: scan heap for a byte pattern. Two-phase design:
            //
            // Phase 1 — FULL scan (cached_bare = None):
            //   Walk all committed readable regions from 4 GB to 512 TB.
            //   ~30 s but only happens ONCE per game session (first run).
            //   Records the address of the bare string (static .rodata copy)
            //   so phase 2 can narrow the search window.
            //
            // Phase 2 — NARROW scan (cached_bare = Some(addr)):
            //   Walk only ±128 MB around the known static-string address,
            //   skipping read-only regions (.rodata where the static copy lives).
            //   Covers only the writable DLL data sections — typically < 10 ms.
            //
            // Returns (live_found, diag, updated_bare_addr).
            #[cfg(target_os = "windows")]
            fn scan_heap_for_trigger(pid: u32, pat: &[u8], cached_bare: Option<u64>) -> (bool, String, Option<u64>) {
                use windows_sys::Win32::{
                    Foundation::CloseHandle,
                    System::{
                        Diagnostics::Debug::ReadProcessMemory,
                        Memory::{
                            VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT,
                            PAGE_GUARD, PAGE_NOACCESS,
                            PAGE_READWRITE, PAGE_EXECUTE_READWRITE,
                            PAGE_WRITECOPY, PAGE_EXECUTE_WRITECOPY,
                        },
                        Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ},
                    },
                };
                const FULL_MIN: u64   = 0x0000_0001_0000_0000; // 4 GB
                const FULL_MAX: u64   = 0x0000_8000_0000_0000; // 512 TB (covers DLL image range)
                const NARROW_R: u64   = 128 * 1024 * 1024;     // ±128 MB around bare_hit
                const REGION_MAX: usize = 32 * 1024 * 1024;    // skip regions > 32 MB
                // Any writable protection (heap/stack/data). Excludes PAGE_READONLY (.rodata).
                const WRITABLE: u32 = PAGE_READWRITE | PAGE_EXECUTE_READWRITE
                                    | PAGE_WRITECOPY | PAGE_EXECUTE_WRITECOPY;

                let bare_pat = &pat[..pat.len().saturating_sub(1)];

                let (scan_min, scan_max, rw_only) = match cached_bare {
                    Some(ba) => (ba.saturating_sub(NARROW_R), ba.saturating_add(NARROW_R), true),
                    None     => (FULL_MIN, FULL_MAX, false),
                };

                let mut found        = false;
                let mut regions_read = 0u32;
                let mut bare_hit: Option<u64> = None;
                let t = std::time::Instant::now();
                unsafe {
                    let proc = OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, 0, pid);
                    if proc == 0 {
                        return (false, format!("OpenProcess failed pid={}", pid), cached_bare);
                    }
                    let mut addr = scan_min;
                    loop {
                        if addr >= scan_max { break; }
                        let mut mbi: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
                        let ret = VirtualQueryEx(proc, addr as *const _,
                            &mut mbi, std::mem::size_of::<MEMORY_BASIC_INFORMATION>());
                        if ret == 0 { break; }
                        let base = mbi.BaseAddress as u64;
                        let size = mbi.RegionSize;
                        addr = base.saturating_add(size as u64);
                        if base < scan_min || base >= scan_max { continue; }
                        if mbi.State != MEM_COMMIT { continue; }
                        if mbi.Protect & (PAGE_GUARD | PAGE_NOACCESS) != 0 { continue; }
                        // Narrow mode: skip read-only sections (.rodata has the static copy).
                        if rw_only && mbi.Protect & WRITABLE == 0 { continue; }
                        if size > REGION_MAX { continue; }
                        let mut buf = vec![0u8; size];
                        let mut n = 0usize;
                        let ok = ReadProcessMemory(proc, base as *const _,
                            buf.as_mut_ptr() as *mut _, buf.len(), &mut n);
                        if ok == 0 || n == 0 { continue; }
                        buf.truncate(n);
                        regions_read += 1;
                        if bare_hit.is_none() {
                            if let Some(off) = buf.windows(bare_pat.len()).position(|w| w == bare_pat) {
                                bare_hit = Some(base + off as u64);
                            }
                        }
                        if buf.windows(pat.len()).any(|w| w == pat) {
                            found = true;
                            break;
                        }
                    }
                    CloseHandle(proc);
                }
                let mode = if cached_bare.is_some() { "narrow" } else { "full" };
                let diag = format!(
                    "{} scan in {}ms: {} regions, bare={}, live={}",
                    mode, t.elapsed().as_millis(), regions_read,
                    bare_hit.map_or("none".to_string(), |a| format!("{:#x}", a)),
                    found
                );
                // Full mode: return the newly discovered bare_hit.
                // Narrow mode: preserve the cached address (skipped .rodata so bare_hit is None).
                (found, diag, if cached_bare.is_none() { bare_hit } else { cached_bare })
            }

            #[cfg(not(target_os = "windows"))]
            fn scan_heap_for_trigger(_pid: u32, _pat: &[u8], cached_bare: Option<u64>) -> (bool, String, Option<u64>) {
                (false, "non-windows".to_string(), cached_bare)
            }

            let session_log = mt_app.state::<AppState>().overlay_log.clone();
            let mut was_open  = false;
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

                let state = mt_app.state::<AppState>();
                if !state.monitor_active.load(Ordering::SeqCst) { break; }
                if !state.mem_trigger_enabled.load(Ordering::SeqCst) {
                    was_open  = false;
                    open_at   = None;
                    continue;
                }

                // Auto-reset open state after the max reward window duration.
                if was_open {
                    if open_at.is_some_and(|t| t.elapsed().as_secs() >= AUTO_RESET_SECS) {
                        was_open = false;
                        open_at  = None;
                    } else {
                        continue;
                    }
                }

                let pid = match Platform::find_warframe_pid() {
                    Some(p) => p,
                    None    => { was_open = false; open_at = None; cached_bare = None; last_pid = 0; continue; }
                };
                // Game restart → ASLR changed all addresses; start over with a full scan.
                if pid != last_pid {
                    cached_bare = None;
                    last_pid = pid;
                }

                let (found, diag, new_bare) = scan_heap_for_trigger(pid, OPEN_PAT, cached_bare);
                // Promote bare_hit from a full scan; preserve across narrow scans.
                if cached_bare.is_none() { cached_bare = new_bare; }

                let ts = now_hms();
                let _ = std::fs::OpenOptions::new().append(true).open(&session_log)
                    .and_then(|mut f| {
                        use std::io::Write;
                        writeln!(f, "[MEM SCAN] @ {} — {}", ts, diag)
                    });
                if found {
                    was_open = true;
                    open_at  = Some(std::time::Instant::now());
                    let _ = std::fs::OpenOptions::new().append(true).open(&session_log)
                        .and_then(|mut f| {
                            use std::io::Write;
                            writeln!(f, "[MEM TRIGGER] Open detected @ {}", ts)
                        });
                    let _ = mt_app.emit(events::FF_STATUS, "🔍 [MEM] Relic reward screen detected");
                    let _ = mt_app.emit(events::RELIC_TRIGGER, ());
                }
            }
        });
    }
}

pub(crate) fn start_legacy_reward_worker(
    monitor_active: Arc<std::sync::atomic::AtomicBool>,
    debug_path: std::path::PathBuf,
    last_found_path: std::path::PathBuf,
) {
    std::thread::spawn(move || {
        while monitor_active.load(Ordering::SeqCst) {
            let _relic_screen = false;
            let mut debug = String::new();
            let ts = now_hms();
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
