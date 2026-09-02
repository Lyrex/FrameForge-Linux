use std::collections::HashMap;
use tracing::{debug, error, info, warn};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager, State};
use crate::app_state::AppState;
use crate::cache::atomic_write;
use crate::catalogue::{fix_category, DebugUnmatched};
use crate::db::QuantityChange;
use crate::diagnostics::write_bmp;
use crate::inventory_state::{load_inventory_state_cache, is_unique_path, build_inventory_from_blob};
use crate::log_watcher::{open_log_notifier, wait_for_log_change};
use crate::relic_pick::park_overlay_offscreen;
use crate::worldstate::store_to_unique;
use crate::{db, log_parser, memory_scanner, ocr, paths};

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

/// Floor on walk frequency, inherited from the fixed cadence this policy
/// replaced: whatever the probe reports, walking is never worth doing faster.
const WALK_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
/// A client with no blob found yet is usually at the login screen. The marker
/// ends that wait as soon as the inventory arrives, so this interval only
/// applies when no marker reaches us at all.
const WALK_COLD_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
/// Covers the state a probe cannot detect: the game reallocates the blob but
/// the old address still holds a parseable copy of the old bytes, so every
/// probe answers "unchanged". That is rare and a walk costs the player frames,
/// hence the long interval.
const WALK_MAX_INTERVAL: std::time::Duration = std::time::Duration::from_secs(900);
/// Nothing changes the inventory without a sync, and a sync is always logged,
/// so this only covers syncs that both marker sources missed. Kept below
/// [`WALK_MAX_INTERVAL`] so the walk intervals still get evaluated on their own
/// schedule.
const BLOB_PROBE_FALLBACK: std::time::Duration = std::time::Duration::from_secs(60);

/// Whether a full region walk is worth its cost this tick.
///
/// A walk reads gigabytes and drops the player's framerate, so most of these
/// rules exist to skip walks that cannot find anything new. The probe covers
/// the common cases in half a millisecond: a blob that grew in place is
/// `Updated`, one that moved is `CacheMiss`.
///
/// None of this depends on the marker for correctness. Every case has an
/// interval that fires without one, so a missing EE.log or an unlocatable log
/// buffer costs only latency.
fn walk_is_due(
    outcome: &memory_scanner::ScanOutcome,
    sync_seen: bool,
    has_cached_blob: bool,
    since_walk: std::time::Duration,
) -> bool {
    use memory_scanner::ScanOutcome;
    match outcome {
        ScanOutcome::Updated => false,
        ScanOutcome::CacheMiss if sync_seen => true,
        ScanOutcome::CacheMiss if has_cached_blob => since_walk >= WALK_MIN_INTERVAL,
        // A sync that moved nothing looks identical to a stale address still
        // holding the old bytes, and the probe cannot tell them apart.
        ScanOutcome::Unchanged if sync_seen => since_walk >= WALK_MIN_INTERVAL,
        ScanOutcome::CacheMiss => since_walk >= WALK_COLD_INTERVAL,
        ScanOutcome::Unchanged => since_walk >= WALK_MAX_INTERVAL,
    }
}

#[tauri::command]
pub(crate) async fn start_monitor(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if state.monitor_active.swap(true, Ordering::SeqCst) {
        return Ok(()); // already running
    }

    // Capture the Tokio runtime handle while we're in the async context.
    // The monitoring thread (std::thread::spawn) has no COM/WinRT, so all OCR
    // calls are routed through spawn_blocking which runs on Tokio's thread pool
    // (which DOES have COM initialized, same as the Capture debug button).
    let _rt = tokio::runtime::Handle::current();

    let items = state.wfcd_items.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut unique_names: Vec<String> = items.iter().map(|i| i.unique_name.clone()).collect();
    let mut display_names: Vec<String> = items.iter().map(|i| i.name.clone()).collect();
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
    let path_aliases: HashMap<&str, &str> = [
        // Sirius & Orion: two WFCD entries for one warframe.
        // "Orion & Sirius" (OrionSuit) is the alternate; "Sirius & Orion" (SiriusSuit) is canonical.
        ("/Lotus/Powersuits/SiriusOrion/OrionSuit",
         "/Lotus/Powersuits/SiriusOrion/SiriusSuit"),
        // Blueprint has the same duplication — Orion & Sirius Blueprint → Sirius & Orion Blueprint.
        ("/Lotus/Powersuits/SiriusOrion/OrionSuitBlueprint",
         "/Lotus/Types/Recipes/WarframeRecipes/SiriusOrionBlueprint"),
    ].into_iter().collect();

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
    let path_to_ducat: HashMap<String, u32> = items.iter()
        .filter_map(|i| i.ducats.map(|d| (i.unique_name.clone(), d)))
        .collect();
    let path_to_vaulted: HashMap<String, bool> = items.iter()
        .filter_map(|i| i.vaulted.map(|v| (i.unique_name.clone(), v)))
        .collect();
    // Owned maps for debug capture — cloned once, no borrow from `items`.
    let path_to_item_type: HashMap<String, String> = items.iter()
        .map(|i| (i.unique_name.clone(), i.item_type.clone())).collect();
    let path_to_product_category: HashMap<String, String> = items.iter()
        .map(|i| (i.unique_name.clone(), i.product_category.clone())).collect();
    let path_to_wfcd_cat: HashMap<String, String> = items.iter()
        .map(|i| (i.unique_name.clone(), i.category.clone())).collect();
    let mut path_to_category: HashMap<String, String> = items.iter()
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
    let ignored_paths: std::collections::HashSet<String> = state.corrections.iter()
        .filter(|(_, c)| c.category.as_deref() == Some("Ignored"))
        .map(|(path, _)| path.clone())
        .collect();
    for p in &ignored_paths {
        path_to_name.remove(p);
        path_to_category.remove(p);
    }
    for (path, c) in &state.corrections {
        if ignored_paths.contains(path) { continue; }
        if let Some(ref name) = c.name {
            if !name.is_empty() { path_to_name.insert(path.clone(), name.clone()); }
        }
        if let Some(ref cat) = c.category {
            path_to_category.insert(path.clone(), cat.clone());
        }
    }
    // Ignored paths are suppressed from the inventory cache just like alias secondaries.
    alias_excluded.extend(ignored_paths.iter().cloned());

    let relic_drops_snapshot: HashMap<String, Vec<String>> =
        state.relic_drops.lock().unwrap_or_else(|e| e.into_inner()).clone();

    let flag = state.monitor_active.clone();
    let db_path = state.db_path.clone();
    let inventory_state_cache_path = state.inventory_state_cache_path.clone();
    let shared_quantities    = state.current_quantities.clone();
    let shared_unique        = state.unique_quantities.clone();
    let shared_mods          = state.current_mods.clone();
    let shared_crafting      = state.current_crafting.clone();
    let blob_log_enabled     = state.blob_log_enabled.clone();
    let blob_log_dir         = state.blob_log_dir.clone();
    let blob_sync_pending    = state.blob_sync_pending.clone();
    let debug_cat_enabled    = state.debug_cat_enabled.clone();
    let auto_capture_dir     = state.auto_capture_dir.clone();
    let unmatched_paths_dir  = state.unmatched_paths_dir.clone();
    let force_pid_check      = state.force_pid_check.clone();
    let reward_app = app.clone();  // clone before app is moved into the inventory thread

    // Channel for the blob capture thread to deliver a parsed BlobInventory to the monitor loop.
    let (blob_tx, blob_rx) = std::sync::mpsc::channel::<memory_scanner::BlobInventory>();

    std::thread::spawn(move || {
        let conn = match rusqlite::Connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => { error!(error = %e, "monitor DB open failed"); return; }
        };
        let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");

        // Start from whatever quantities were last known (survives restarts).
        let mut known: HashMap<String, i64> =
            shared_quantities.lock().unwrap_or_else(|e| e.into_inner()).clone();

        // Load the full inventory state from the last session so the UI shows data
        // immediately on restart without waiting for the first full scan pass.
        let startup_cache = load_inventory_state_cache(&inventory_state_cache_path);

        // Pre-populate known with cached resource quantities so that per-cycle hint
        // emits never replace the frontend display with a partial inventory.
        // is_stackable overrides is_unique_path: Kubrow Eggs, Kavat Genetic Codes,
        // cosmetics, and Railjack weapons share path prefixes with actual unique items
        // but have counts > 1 from MiscItems/FlavourItems — they must go into known.
        for (path, item) in &startup_cache.items {
            if item.amount > 0 && item.mod_ranks.is_none()
                && (item.is_stackable || !is_unique_path(path))
            {
                known.entry(path.clone()).or_insert(item.amount as i64);
            }
        }
        // Keep shared_quantities in sync so the cache-clear detector doesn't misfire.
        {
            let mut q = shared_quantities.lock().unwrap_or_else(|e| e.into_inner());
            if q.is_empty() && !known.is_empty() { *q = known.clone(); }
        }

        // Stability buffer for unique scanner items (weapons/warframes).
        // Pre-seed confirmed items at count=4 so they show immediately on restart.
        // Exclude is_stackable items — they are seeded into known above, not here.
        let mut unique_stable: HashMap<String, u8> = startup_cache.items.iter()
            .filter(|(k, v)| v.mod_ranks.is_none() && v.amount > 0 && !v.subsumed
                          && !v.is_stackable && is_unique_path(k))
            .map(|(k, _)| (k.clone(), 4u8))
            .collect();
        let mut confirmed_unique: std::collections::HashSet<String> =
            unique_stable.keys().cloned().collect();

        // Mods: commit hint results directly on every partial pass.
        // The hint is the live inventory-root region and is always authoritative.
        // No stability buffer needed — wrong counts on a bad scan self-correct next pass.
        // Pre-seed from startup cache so mods/arcanes show immediately on restart instead
        // of going blank until the hint scan rediscovers the RawUpgrades region.
        let mut known_mods: HashMap<String, memory_scanner::ModCount> = {
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
        // Track the last date we recorded daily snapshots (YYYY-MM-DD).
        // Initialise to yesterday so the first scan of a new day always fires.
        let mut last_snapshot_date = String::new();

        // Emit an immediate status before the first scan so the UI shows cached
        // inventory data without waiting for the scan to finish.
        {
            let game_found = memory_scanner::find_warframe_pid_pub().is_some();
            let now_pre = chrono::Utc::now().timestamp();
            let mut initial_qty = known.clone();
            for k in unique_stable.keys() { initial_qty.entry(k.clone()).or_insert(1); }
            for (path, mc) in &known_mods { initial_qty.entry(path.clone()).or_insert(mc.total); }
            let _ = app.emit("inventory-update", InventoryUpdate {
                quantities: initial_qty,
                crafting: vec![],
                mastery_rank: startup_cache.mastery_rank,
                mastery_data: startup_cache.items.iter()
                    .filter(|(_, v)| v.mastery_rank > 0)
                    .map(|(k, v)| (k.clone(), v.mastery_rank))
                    .collect(),
                changes: vec![],
                consumed_suits: startup_cache.consumed_suits(),
                mods: known_mods.clone(),
                socketed_shards: startup_cache.items.iter()
                    .filter(|(_, v)| !v.archon_shards.is_empty())
                    .map(|(k, v)| (k.clone(), v.archon_shards.clone()))
                    .collect(),
                forma_counts: startup_cache.items.iter()
                    .filter_map(|(k, v)| v.forma_count.map(|n| (k.clone(), n)))
                    .collect(),
                warframe_running: game_found,
                scanned_at: now_pre,
                is_full_pass: true,
                player_name: app.state::<AppState>().local_player_name
                    .lock().ok().and_then(|g| g.clone()),
            });
        }

        let mut current_mastery_rank: Option<u32> = startup_cache.mastery_rank;
        let mut current_mastery_data: HashMap<String, u32> = startup_cache.items.iter()
            .filter(|(_, v)| v.mastery_rank > 0)
            .map(|(k, v)| (k.clone(), v.mastery_rank))
            .collect();
        let mut current_recipes: Vec<memory_scanner::PendingRecipe> = Vec::new();
        let mut current_consumed_suits: Vec<String> = startup_cache.consumed_suits();
        let mut current_socketed_shards: HashMap<String, Vec<memory_scanner::ArchonShard>> = startup_cache.items.iter()
            .filter(|(_, v)| !v.archon_shards.is_empty())
            .map(|(k, v)| (k.clone(), v.archon_shards.clone()))
            .collect();
        let mut current_forma_counts: HashMap<String, u32> = startup_cache.items.iter()
            .filter_map(|(k, v)| v.forma_count.map(|n| (k.clone(), n)))
            .collect();
        let mut last_walk_time: Option<std::time::Instant> = None;
        let mut last_probe_time: Option<std::time::Instant> = None;
        let mut last_blob_probe: Option<std::time::Instant> = None;
        // Guard against overlapping captures: a full memory walk can take >10 s on large
        // game processes, so without this flag we'd stack up concurrent scan threads.
        let blob_scan_active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Cache the game-running state so we only re-enumerate processes once every 5 s
        // instead of on every 2-second loop tick (CreateToolhelp32Snapshot is not free).
        let mut last_pid_check: Option<std::time::Instant> = None;
        let mut last_pid: Option<u32> = None;
        let mut cached_game_running = false;
        // When game is not running, suppress redundant inventory-update emits.
        // Only emit on the status-change tick and then at most once every 30 s as a heartbeat.
        let mut prev_game_running = false;
        let mut last_not_running_emit: Option<std::time::Instant> = None;

        while flag.load(Ordering::SeqCst) {
            // If shared_quantities was cleared externally (clear_cache command), wipe local
            // state so the next blob logs everything as fresh.
            {
                let sq = shared_quantities.lock().unwrap_or_else(|e| e.into_inner());
                let local_has_data = !known.is_empty() || !unique_stable.is_empty() || !known_mods.is_empty();
                if sq.is_empty() && local_has_data {
                    known.clear();
                    unique_stable.clear();
                    confirmed_unique.clear();
                    known_mods.clear();
                }
            }

            let now = chrono::Utc::now().timestamp();

            // Process any incoming blob (non-blocking)
            while let Ok(blob) = blob_rx.try_recv() {
                let existing_wfm: HashMap<String, u32> =
                    load_inventory_state_cache(&inventory_state_cache_path)
                        .items.into_iter()
                        .filter_map(|(k, v)| v.wfm_price.map(|p| (k, p)))
                        .collect();
                let sc = build_inventory_from_blob(
                    &blob,
                    &path_to_name, &path_to_category, &path_to_ducat, &path_to_vaulted,
                    &relic_drops_snapshot, &existing_wfm, &alias_excluded,
                );
                if let Ok(json) = serde_json::to_string(&sc) {
                    let _ = atomic_write(&inventory_state_cache_path, json.as_bytes());
                }

                // Snapshot previous full inventory (known + uniques + mods) for change detection.
                let prev_all: HashMap<String, i64> = {
                    let mut m = known.clone();
                    for k in &confirmed_unique { m.entry(k.clone()).or_insert(1); }
                    for (p, mc) in &known_mods { m.entry(p.clone()).or_insert(mc.total); }
                    m
                };

                // Completeness guard: parse_full_account_blob already rejects blobs missing
                // required sections (MiscItems, RegularCredits, etc.) — see memory_scanner.rs.
                // Keep this secondary guard for the unique-items case as a belt-and-suspenders
                // defence against incomplete blobs that slipped through parsing.
                let prev_unique_count = confirmed_unique.len();
                if blob.unique_items.is_empty() && prev_unique_count > 0 {
                    warn!("blob rejected at commit: 0 unique items vs {} previously — incomplete blob", prev_unique_count);
                    continue;
                }

                // Blob is authoritative — full replacement, not a merge.
                // Clear known so items that disappeared from the blob drop to 0.
                known.clear();

                // Currency
                known.insert("/_currency/Credits".to_string(),      blob.credits);
                known.insert("/_currency/Endo".to_string(),         blob.endo);
                known.insert("/_currency/Platinum".to_string(),     blob.platinum - blob.free_platinum);
                known.insert("/_currency/PlatinumGift".to_string(), blob.free_platinum);

                // Stackable items
                for entry in &blob.stackable_items {
                    known.insert(entry.item_type.clone(), entry.item_count);
                }

                // Unique items — full replacement (blob is authoritative)
                unique_stable.clear();
                confirmed_unique.clear();
                current_socketed_shards.clear();
                current_forma_counts.clear();
                for entry in &blob.unique_items {
                    let canonical = path_aliases.get(entry.item_type.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| entry.item_type.clone());
                    if blob.consumed_suits.contains(&canonical) { continue; }
                    unique_stable.insert(canonical.clone(), 4);
                    confirmed_unique.insert(canonical.clone());
                    if !entry.archon_shards.is_empty() {
                        current_socketed_shards.insert(canonical.clone(), entry.archon_shards.clone());
                    }
                    if entry.polarized > 0 {
                        current_forma_counts.insert(canonical, entry.polarized);
                    }
                }

                // Mods — full replacement
                known_mods.clear();
                for (path, mc) in &blob.mods {
                    known_mods.insert(path.clone(), mc.clone());
                }
                // Rivens — group by item_type so they appear in inventory like regular mods
                for riven in &blob.rivens {
                    let mc = known_mods.entry(riven.item_type.clone()).or_default();
                    mc.total += riven.count as i64;
                    *mc.by_rank.entry(riven.mod_rank).or_insert(0) += riven.count as i64;
                }

                // Cosmetics (FlavourItems + WeaponSkins) — occurrence-counted, go into known
                for (path, &count) in blob.flavour_items.iter().chain(blob.weapon_skins.iter()) {
                    known.insert(path.clone(), count);
                }

                // Debug: write paths with no WFCD entry or Misc fallback to the Unmatched Paths folder.
                if debug_cat_enabled.load(Ordering::Relaxed) {
                    // ── Reference file (written once per session) ─────────────────────
                    // Lists every distinct item_type / product_category / wfcd_category value
                    // present in the catalog, together with the display category fix_category()
                    // assigns to each.  Useful for adding new tiers to fix_category.
                    let ref_path = unmatched_paths_dir.join("_reference.json");
                    if !ref_path.exists() {
                        // Collect distinct values; BTreeMap keeps them alphabetically sorted.
                        // Iterate over path_to_name (covers ALL catalog entries, including
                        // blueprints that have item_type = "" but wfcd_category = "Blueprints").
                        let mut item_types: std::collections::BTreeMap<String, String> = Default::default();
                        let mut prod_cats:  std::collections::BTreeMap<String, String> = Default::default();
                        let mut wfcd_cats:  std::collections::BTreeMap<String, String> = Default::default();
                        for (path, nm) in &path_to_name {
                            let it  = path_to_item_type.get(path).map(|s| s.as_str()).unwrap_or("");
                            let pc  = path_to_product_category.get(path).map(|s| s.as_str()).unwrap_or("");
                            let wc  = path_to_wfcd_cat.get(path).map(|s| s.as_str()).unwrap_or("");
                            let cat = fix_category(nm, it, pc, wc, path);
                            if !it.is_empty() { item_types.entry(it.to_string()).or_insert(cat.clone()); }
                            if !pc.is_empty() { prod_cats.entry(pc.to_string()).or_insert(cat.clone()); }
                            if !wc.is_empty() { wfcd_cats.entry(wc.to_string()).or_insert(cat); }
                        }
                        let ref_json = serde_json::json!({
                            "note": "Distinct field values from the loaded WFCD catalog. 'maps_to' shows the display category fix_category() assigns when that field is the deciding factor.",
                            "item_type": item_types.iter().map(|(v, c)| serde_json::json!({ "value": v, "maps_to": c })).collect::<Vec<_>>(),
                            "product_category": prod_cats.iter().map(|(v, c)| serde_json::json!({ "value": v, "maps_to": c })).collect::<Vec<_>>(),
                            "wfcd_category": wfcd_cats.iter().map(|(v, c)| serde_json::json!({ "value": v, "maps_to": c })).collect::<Vec<_>>(),
                        });
                        if let Ok(s) = serde_json::to_string_pretty(&ref_json) {
                            let _ = std::fs::write(&ref_path, s);
                        }
                    }

                    // ── Per-scan unmatched file ───────────────────────────────────────
                    // Build per-path blob field lookups.
                    let stackable_count: std::collections::HashMap<&str, i64> = blob.stackable_items.iter()
                        .map(|e| (e.item_type.as_str(), e.item_count)).collect();
                    let unique_section: std::collections::HashMap<&str, &str> = blob.unique_items.iter()
                        .map(|e| (e.item_type.as_str(), e.section.as_str())).collect();
                    let unique_polarized: std::collections::HashMap<&str, u32> = blob.unique_items.iter()
                        .map(|e| (e.item_type.as_str(), e.polarized)).collect();

                    let all_paths: Vec<&str> = blob.stackable_items.iter().map(|e| e.item_type.as_str())
                        .chain(blob.unique_items.iter().map(|e| e.item_type.as_str()))
                        .chain(blob.mods.keys().map(|k| k.as_str()))
                        .collect();
                    let mut new_entries: Vec<DebugUnmatched> = Vec::new();
                    for p in all_paths {
                        if p.starts_with("/_currency/") { continue; }
                        if ignored_paths.contains(p) { continue; }
                        let name = path_to_name.get(p).cloned().unwrap_or_default();
                        let (reason, final_cat) = if name.is_empty() {
                            // Check path-prefix rules first (Tier 8 in fix_category).
                            let inferred_cat = fix_category("", "", "", "", p);
                            if inferred_cat != "Miscellaneous" && inferred_cat != "Excluded" {
                                ("path_rule".to_string(), inferred_cat)
                            } else {
                                let last = p.rsplit('/').next().unwrap_or("");
                                if last.ends_with("Blueprint") && p.contains("/Recipes/") {
                                    ("path_inferred".to_string(), "Blueprints".to_string())
                                } else {
                                    ("no_wfcd_match".to_string(), "Unknown".to_string())
                                }
                            }
                        } else {
                            let cat = path_to_category.get(p).map(|s| s.as_str()).unwrap_or("Miscellaneous");
                            if cat != "Miscellaneous" { continue; }
                            ("misc_fallback".to_string(), "Misc".to_string())
                        };
                        // Last 4 non-trivial segments for quick identification.
                        let path_hint: Vec<String> = p.split('/')
                            .filter(|s| !s.is_empty() && *s != "Lotus")
                            .rev().take(4).collect::<Vec<_>>()
                            .into_iter().rev().map(|s| s.to_string()).collect();
                        new_entries.push(DebugUnmatched {
                            path: p.to_string(),
                            name,
                            item_type:        path_to_item_type.get(p).cloned().unwrap_or_default(),
                            product_category: path_to_product_category.get(p).cloned().unwrap_or_default(),
                            wfcd_category:    path_to_wfcd_cat.get(p).cloned().unwrap_or_default(),
                            final_category:   final_cat,
                            reason,
                            item_count:  stackable_count.get(p).copied(),
                            section:     unique_section.get(p).map(|s| s.to_string()),
                            polarized:   unique_polarized.get(p).copied(),
                            mod_total:   blob.mods.get(p).map(|m| m.total),
                            path_hint,
                        });
                    }
                    if !new_entries.is_empty() {
                        let ts = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string();
                        let out = unmatched_paths_dir.join(format!("{}.json", ts));
                        if let Ok(json) = serde_json::to_string_pretty(&new_entries) {
                            let _ = std::fs::write(&out, json);
                        }
                    }
                }

                // Meta
                current_mastery_rank = Some(blob.mastery_level);
                for (path, &rank) in &blob.mastery_data {
                    current_mastery_data.insert(path.clone(), rank);
                }
                current_consumed_suits = blob.consumed_suits.clone();
                for suit in &current_consumed_suits {
                    confirmed_unique.remove(suit);
                    unique_stable.remove(suit);
                }
                current_recipes = blob.pending_recipes.iter().map(|r| memory_scanner::PendingRecipe {
                    unique_name:   r.item_type.clone(),
                    completion_ms: r.completion_ms,
                }).collect();

                // Sync shared state
                if let Ok(mut q)  = shared_quantities.lock() { *q = known.clone(); }
                if let Ok(mut sm) = shared_mods.lock()       { *sm = known_mods.clone(); }
                if let Ok(mut uq) = shared_unique.lock() {
                    uq.clear();
                    for name in &confirmed_unique { uq.insert(name.clone(), 1); }
                }

                // Emit inventory update
                let mut emit_qty = known.clone();
                for k in &confirmed_unique { emit_qty.entry(k.clone()).or_insert(1); }
                for (p, mc) in &known_mods { emit_qty.entry(p.clone()).or_insert(mc.total); }

                // Detect and record every quantity change (up, down, new, gone-to-0).
                // Skip on the very first blob of the session (prev_all empty = no prior baseline).
                let mut changes: Vec<QuantityChange> = vec![];
                if !prev_all.is_empty() {
                    let ts = chrono::Utc::now().timestamp();
                    let all_keys: std::collections::HashSet<&String> =
                        prev_all.keys().chain(emit_qty.keys()).collect();
                    for key in all_keys {
                        // Ignored paths are absent from the startup cache but present
                        // in every blob, so without this each start logs them as new.
                        if ignored_paths.contains(key.as_str()) { continue; }
                        let old_qty = *prev_all.get(key).unwrap_or(&0);
                        let new_qty = *emit_qty.get(key).unwrap_or(&0);
                        if old_qty == new_qty { continue; }
                        let item_name = path_to_name.get(key.as_str())
                            .cloned()
                            .unwrap_or_else(|| key.split('/').last().unwrap_or("?").to_string());
                        let _ = db::add_quantity_change(&conn, key, &item_name, old_qty, new_qty);
                        changes.push(QuantityChange {
                            id: 0,
                            unique_name: key.clone(),
                            item_name,
                            old_qty,
                            new_qty,
                            delta: new_qty - old_qty,
                            timestamp: ts,
                        });
                    }
                }

                let crafting: Vec<CraftingJob> = blob.pending_recipes.iter().map(|r| {
                    let name = display_names.iter().zip(unique_names.iter())
                        .find(|(_, u)| **u == r.item_type)
                        .map(|(d, _)| d.clone())
                        .unwrap_or_else(|| r.item_type.split('/').last().unwrap_or("?").to_string());
                    CraftingJob { unique_name: r.item_type.clone(), item_name: name, completion_ms: r.completion_ms }
                }).collect();
                *shared_crafting.lock().unwrap_or_else(|e| e.into_inner()) = crafting.clone();
                let _ = app.emit("inventory-update", InventoryUpdate {
                    quantities: emit_qty,
                    crafting,
                    mastery_rank: current_mastery_rank,
                    mastery_data: current_mastery_data.clone(),
                    changes,
                    warframe_running: true,
                    scanned_at:   now,
                    consumed_suits:   current_consumed_suits.clone(),
                    mods:             known_mods.clone(),
                    socketed_shards:  current_socketed_shards.clone(),
                    forma_counts:     current_forma_counts.clone(),
                    is_full_pass:     true,
                    player_name: app.state::<AppState>().local_player_name
                        .lock().ok().and_then(|g| g.clone()),
                });

                let detail = format!(
                    "{} unique · {} resources · {} mods · {} flavour",
                    blob.unique_items.len(), blob.stackable_items.len(),
                    blob.mods.len(), blob.flavour_items.len()
                );
                info!(detail = %detail, "blob applied");
                let _ = app.emit("blob-status", BlobStatusPayload {
                    stage: "done".into(),
                    detail,
                });

                // Daily snapshots
                let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
                if today != last_snapshot_date {
                    last_snapshot_date = today.clone();
                    if let Ok(tracked) = db::get_tracked_items(&conn) {
                        for item in &tracked {
                            let qty = *known.get(&item.unique_name).unwrap_or(&0);
                            let _ = db::record_snapshot(&conn, &item.unique_name, &today, qty);
                        }
                    }
                }
            }

            // Re-enumerate processes at most every 5 s (CreateToolhelp32Snapshot overhead).
            // force_pid_check bypasses the cooldown (set by the poke_scan command).
            let forced = force_pid_check.swap(false, Ordering::SeqCst);
            let needs_pid_check = forced || last_pid_check
                .map_or(true, |t: std::time::Instant| t.elapsed().as_secs() >= 5);
            if needs_pid_check {
                let current_pid = memory_scanner::find_warframe_pid_pub();
                cached_game_running = current_pid.is_some();
                if current_pid != last_pid {
                    if current_pid.is_some() {
                        info!(?last_pid, ?current_pid, "Warframe PID changed, clearing blob region cache");
                        memory_scanner::reset_last_blob_region();
                        memory_scanner::reset_log_region();
                    }
                    last_pid = current_pid;
                }
                last_pid_check = Some(std::time::Instant::now());
            }
            let game_running = cached_game_running;
            if game_running {
                // ── Blob capture: cheap probe, rate-limited walk ──────────────
                // The probe runs at PROBE_INTERVAL; re-reading the blob itself is
                // gated additionally by BLOB_PROBE_FALLBACK or the sync marker.
                const PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

                let walk_in_flight = blob_scan_active.load(Ordering::SeqCst);
                let probe_due = last_probe_time
                    .map_or(true, |t: std::time::Instant| t.elapsed() >= PROBE_INTERVAL);

                let mut should_capture = false;
                if probe_due && !walk_in_flight {
                    last_probe_time = Some(std::time::Instant::now());
                    let stitch_due = last_blob_probe
                        .map_or(true, |t: std::time::Instant| t.elapsed() >= BLOB_PROBE_FALLBACK)
                        || blob_sync_pending.load(Ordering::SeqCst)
                        || !memory_scanner::has_cached_blob();
                    let (outcome, sync_marker) = match last_pid {
                        Some(pid) => memory_scanner::probe_tick(pid, blob_tx.clone(), stitch_due),
                        None => (None, false),
                    };
                    if outcome.is_some() {
                        last_blob_probe = Some(std::time::Instant::now());
                    }
                    if sync_marker {
                        blob_sync_pending.store(true, Ordering::SeqCst);
                    }
                    let sync_seen  = blob_sync_pending.load(Ordering::SeqCst);
                    let blob_known = memory_scanner::has_cached_blob();
                    let since_walk = last_walk_time
                        .map_or(std::time::Duration::MAX, |t: std::time::Instant| t.elapsed());
                    should_capture = outcome
                        .as_ref()
                        .is_some_and(|o| walk_is_due(o, sync_seen, blob_known, since_walk));
                    if should_capture || outcome == Some(memory_scanner::ScanOutcome::Updated) {
                        blob_sync_pending.store(false, Ordering::SeqCst);
                    }
                    if should_capture {
                        let since = match last_walk_time {
                            Some(t) => format!("{:.1}s", t.elapsed().as_secs_f64()),
                            None => "never".into(),
                        };
                        info!(outcome = ?outcome, sync_seen, blob_known, since_last_walk = %since, "escalating to full walk");
                    }
                }

                if should_capture {
                    blob_scan_active.store(true, Ordering::SeqCst);
                    last_walk_time = Some(std::time::Instant::now());
                    let ts     = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();
                    let dir    = blob_log_dir.clone();
                    let tx     = blob_tx.clone();
                    let save   = blob_log_enabled.load(Ordering::SeqCst);
                    let active = blob_scan_active.clone();
                    let _ = app.emit("blob-status", BlobStatusPayload {
                        stage:  "scanning".into(),
                        detail: "Reading Warframe memory\u{2026}".into(),
                    });
                    debug!(save, "blob capture starting");
                    std::thread::spawn(move || {
                        struct ClearOnDrop(std::sync::Arc<std::sync::atomic::AtomicBool>);
                        impl Drop for ClearOnDrop {
                            fn drop(&mut self) { self.0.store(false, Ordering::SeqCst); }
                        }
                        let _guard = ClearOnDrop(active);
                        let count = memory_scanner::capture_all_blobs(&dir, &ts, tx, save);
                        debug!(files_saved = count, save_flag = save, ts = %ts, "blob capture finished");
                    });
                }
                prev_game_running = true;
            } else {
                // Game not running — throttle emits: only on status-change and every 30 s heartbeat.
                // Without this guard the loop emits every 2 s with identical data, triggering a
                // full React render cascade (17 k-item useMemo rebuild) 30 times per minute.
                let status_changed = prev_game_running;
                let heartbeat_due  = last_not_running_emit
                    .map_or(true, |t: std::time::Instant| t.elapsed() >= std::time::Duration::from_secs(30));
                if status_changed || heartbeat_due {
                    let mut emit_qty = known.clone();
                    for k in &confirmed_unique { emit_qty.entry(k.clone()).or_insert(1); }
                    for (p, mc) in &known_mods { emit_qty.entry(p.clone()).or_insert(mc.total); }
                    let crafting: Vec<CraftingJob> = current_recipes.iter().map(|r| {
                        let name = display_names.iter().zip(unique_names.iter())
                            .find(|(_, u)| *u == &r.unique_name)
                            .map(|(d, _)| d.clone())
                            .unwrap_or_else(|| r.unique_name.split('/').last().unwrap_or("?").to_string());
                        CraftingJob { unique_name: r.unique_name.clone(), item_name: name, completion_ms: r.completion_ms }
                    }).collect();
                    // Skip mastery_data on heartbeats — it hasn't changed and spreading 17k
                    // entries into React state on every tick is expensive.
                    let send_mastery = status_changed;
                    let _ = app.emit("inventory-update", InventoryUpdate {
                        quantities: emit_qty, crafting,
                        mastery_rank: current_mastery_rank,
                        mastery_data: if send_mastery { current_mastery_data.clone() } else { HashMap::new() },
                        changes: vec![], warframe_running: false, scanned_at: now,
                        consumed_suits: current_consumed_suits.clone(),
                        mods: known_mods.clone(),
                        socketed_shards: current_socketed_shards.clone(),
                        forma_counts: current_forma_counts.clone(),
                        is_full_pass: false,
                        player_name: app.state::<AppState>().local_player_name
                            .lock().ok().and_then(|g| g.clone()),
                    });
                    last_not_running_emit = Some(std::time::Instant::now());
                }
                prev_game_running = false;
            }

            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });

    // ── Dedicated relic reward thread — OCR poll every 500 ms ───────────────
    // Takes a screenshot of the Warframe window, runs Windows OCR on the
    // reward area, matches names against the catalog. Emits "relic-rewards"
    // only when the result changes (screen opens/closes or items change).
    let reward_flag   = state.monitor_active.clone();
    let reward_items  = state.wfcd_items.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let relic_rewards_map = state.relic_rewards.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let wiki_names    = state.wiki_reward_names.lock().unwrap_or_else(|e| e.into_inner()).clone();

    // ── Catalog: build from Relics.json reward names ─────────────────────────
    //
    // relic_rewards_map is already populated from Relics.json — every name in it
    // IS a confirmed relic reward. Start from those names and look up unique_names
    // in the WFCD item catalog. This guarantees only actual relic rewards appear
    // as OCR candidates, preventing false matches against items like
    // "Titan Extractor Prime Blueprint" or "Kavasa Prime Kubrow Collar Kavasa Prime Band"
    // that contain "prime" but are not relic rewards.

    // Collect all reward display names (lowercase) from Relics.json + wiki corrections.
    let reward_display_names: std::collections::HashSet<String> = relic_rewards_map
        .values()
        .flat_map(|rewards| rewards.iter().map(|r| r.name.to_lowercase()))
        .chain(wiki_names.iter().cloned())
        .collect();

    // Build a lowercase-name → (unique_name, original_display_name) lookup over the WFCD
    // item catalog. Excludes assembled warframes/weapons and relics (never relic rewards).
    let wfcd_by_name: std::collections::HashMap<String, (String, String)> = reward_items.iter()
        .filter(|i| {
            let lower = i.name.to_lowercase();
            let is_relic = lower.ends_with("intact") || lower.ends_with("exceptional")
                || lower.ends_with("flawless") || lower.ends_with("radiant");
            let is_built = matches!(i.category.as_str(),
                "Warframes" | "Primary" | "Secondary" | "Melee" | "Companion" |
                "Sentinels" | "Archwing" | "Arch-Gun" | "Arch-Melee" | "Pets" | "Robotic");
            !is_relic && !is_built
        })
        .map(|i| (i.name.to_lowercase(), (i.unique_name.clone(), i.name.clone())))
        .collect();

    // For each known reward name find the WFCD unique_name.
    // Handles the WFCD "Blueprint" suffix inconsistency in both directions:
    //   Relics.json "Lavos Prime Chassis Blueprint" ↔ WFCD item "Lavos Prime Chassis"
    let mut catalog_pairs: Vec<(String, String)> = reward_display_names.iter()
        .filter_map(|reward_lower| {
            // Exact match
            if let Some((unique, display)) = wfcd_by_name.get(reward_lower.as_str()) {
                return Some((unique.clone(), display.clone()));
            }
            // Reward has " blueprint" suffix but WFCD item doesn't
            if let Some(stem) = reward_lower.strip_suffix(" blueprint") {
                if let Some((unique, display)) = wfcd_by_name.get(stem) {
                    return Some((unique.clone(), format!("{} Blueprint", display)));
                }
            }
            // Reward lacks " blueprint" but WFCD item has it
            let with_bp = format!("{} blueprint", reward_lower);
            if let Some((unique, display)) = wfcd_by_name.get(&with_bp) {
                return Some((unique.clone(), display.clone()));
            }
            // Not in WFCD item catalog — skip (no unique_name means no price/inventory data)
            None
        })
        .collect();

    // Deduplicate by unique_name
    catalog_pairs.sort_by(|a, b| a.0.cmp(&b.0));
    catalog_pairs.dedup_by(|a, b| a.0 == b.0);

    // Wrap catalog in Arc so it can be cheaply shared with spawn_blocking closures
    let catalog_pairs = std::sync::Arc::new(catalog_pairs);

    // Build a name-lookup map from catalog_pairs for the debug file.
    let _catalog_name_map: std::collections::HashMap<String, String> = catalog_pairs
        .iter()
        .map(|(u, n)| (u.clone(), n.clone()))
        .collect();

    let debug_path      = paths::state_dir().join("frameforge_reward_debug.txt");
    let last_found_path = paths::state_dir().join("frameforge_last_reward.txt");

    // ── EE.log watcher ────────────────────────────────────────────────────────
    // Warframe writes "Script [Info]: Got rewards" to EE.log the moment the
    // Void Fissure reward selection screen becomes active.  All open-source
    // tools (WFInfo, warframeocr, Sentinel) use this string as their trigger.
    // We tail the log file instead of relying on fragile OCR gate heuristics.
    let ee_log_path = log_parser::watched_log_path();

    // Shared flag: true while the reward screen is active according to EE.log
    let reward_screen_active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reward_screen_active2 = reward_screen_active.clone();

    // Unix-ms timestamp of the last relic-rewards emit with real items. Zero = never.
    // Written by the OCR task when it locks and emits; read by the dismiss handler to
    // enforce a minimum overlay display time so fast EE.log flushes don't hide the
    // overlay before the user has time to read it.
    let rewards_emitted_ms: std::sync::Arc<std::sync::atomic::AtomicU64> =
        std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let rewards_emitted_ms_ocr  = rewards_emitted_ms.clone();
    let rewards_emitted_ms_ee   = rewards_emitted_ms.clone();

    // Shared squad size: updated by EE.log watcher when VoidProjections sequence
    // completes, read by OCR loop for each attempt. This lets late-arriving squad
    // data (VoidProjections often arrives 1-2 s after the screen opens) inform
    // subsequent OCR retries so the card count is always correct.
    let shared_squad_size: std::sync::Arc<std::sync::Mutex<Option<usize>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let shared_squad_size2 = std::sync::Arc::clone(&shared_squad_size);

    // Squad member names collected from EE.log "AddSquadMember:" lines.
    // Passed to OCR so it can reject any text that fuzzy-matches a player name.
    let shared_squad_names: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let shared_squad_names2 = std::sync::Arc::clone(&shared_squad_names);

    // ── EE.log watcher → AlecaFrame-style OCR trigger ────────────────────────
    //
    // When Warframe writes "Got rewards" to EE.log, the reward screen is active.
    // We immediately schedule an OCR capture (same path as the working Capture
    // button) and emit the result as a "relic-rewards" event.
    // No polling needed — this is exactly how AlecaFrame works.

    let ee_ocr_app   = reward_app.clone();
    let ee_catalog   = std::sync::Arc::clone(&catalog_pairs);
    let ee_last_path = last_found_path.clone();
    let session_log_path = paths::state_dir().join("frameforge_overlay_session.txt");
    let ee_auto_capture_dir = auto_capture_dir.clone();

    // The gate only makes blob capture faster; with no log to tail the monitor
    // still escalates on its own interval. A missing gate therefore changes no
    // behaviour, so it is reported once at startup.
    let blob_sync_pending = state.blob_sync_pending.clone();
    info!(
        target: "frameforge::monitor",
        gate = if ee_log_path.is_some() { "armed" } else { "disarmed (no EE.log)" },
        "inventory-sync marker gate"
    );

    if let Some(log_path) = ee_log_path {
        let flag = reward_flag.clone();
        std::thread::spawn(move || {
            let mut file_pos: u64 = std::fs::metadata(&log_path)
                .map(|m| m.len()).unwrap_or(0);
            let mut active_since: Option<std::time::Instant> = None;
            use std::io::{Read, Seek, SeekFrom};

            // ── Startup scan: seed player names from the existing log ─────────
            // The tail starts at file-end so lines written before FrameForge launched
            // are invisible to it. Two bounded reads cover both cases:
            //  • First 64 KB  → "Logged in NAME" is always within the first ~100 lines.
            //  • Last 1 MB    → AddSquadMember fires during mission load-in (recent).
            // Bounded reads avoid stalling on a log file that has grown to hundreds of MB.
            {
                use std::io::{Read, Seek, SeekFrom};

                // Read the last 1 MB of EE.log. This covers both cases:
                //   • EE.log resets on game launch → whole file fits in 1 MB.
                //   • EE.log accumulates → current session's "Logged in" is near the end.
                // Searching only the first 64 KB misses the current session when the log
                // has grown large from previous runs.
                if let Ok(mut f) = std::fs::File::open(&log_path) {
                    let file_len = f.seek(SeekFrom::End(0)).unwrap_or(0);
                    let read_from = file_len.saturating_sub(1_048_576); // last 1 MB
                    let _ = f.seek(SeekFrom::Start(read_from));
                    let mut buf = Vec::with_capacity(1_048_576);
                    let _ = f.read_to_end(&mut buf);
                    // Skip first (potentially partial) line when starting mid-file.
                    let start = if read_from > 0 { buf.iter().position(|&b| b == b'\n').map_or(0, |i| i + 1) } else { 0 };
                    if let Ok(text) = std::str::from_utf8(&buf[start..]) {
                        // ── Local player name (most recent "Logged in NAME") ──────────
                        parse_logged_in_name(text, &shared_squad_names2, &ee_ocr_app);

                        // ── Squad mate names ──────────────────────────────────────────
                        for line in text.lines() {
                            if line.contains("AddSquadMember: ") {
                                if let Some(after) = line.find("AddSquadMember: ").map(|i| &line[i + 16..]) {
                                    if let Some(name) = after.split(',').next().map(str::trim) {
                                        if !name.is_empty() {
                                            if let Ok(mut g) = shared_squad_names2.lock() {
                                                if !g.iter().any(|n: &String| n == name) {
                                                    g.push(name.to_string());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            // "NAME - new avatar: CombatOperatorAvatar…" fires when any player
                            // switches to Operator or Necramech — catches squadmates who joined
                            // before FrameForge started (and thus missed AddSquadMember lines).
                            if line.contains(" - new avatar: ") {
                                if let Some(after_bracket) = line.find("]: ").map(|i| &line[i + 3..]) {
                                    if let Some(name) = after_bracket.split(" - new avatar:").next() {
                                        let name = name.trim();
                                        if name.len() >= 3 && !name.contains(' ') {
                                            if let Ok(mut g) = shared_squad_names2.lock() {
                                                if !g.iter().any(|n: &String| n == name) {
                                                    g.push(name.to_string());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ── VoidProjections reward sequence state ─────────────────────────
            // The game logs squad reward info BEFORE the screen trigger fires.
            // We accumulate it across poll iterations so it's ready when OCR starts.
            let mut vp_in_seq        = false;
            let mut vp_seq_completed = false; // set when sequence finishes; used as fallback trigger
            let mut vp_other_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut vp_own_item = String::new(); // local player's reward path from EE.log
            // Cooldown: after any dismiss, block new triggers for 5 s to filter
            // stale EE.log lines that can arrive shortly after a dismiss.
            let mut last_dismiss_at: Option<std::time::Instant> = None;
            // ── Relic prefilter ───────────────────────────────────────────────────
            // Projection paths collected from "Resource load completed" EE.log lines
            // while squad loadouts download. Used at trigger time to narrow the OCR
            // candidate list from ~700 items to the ~6-24 rewards of the active relics.
            // To revert: delete this Vec, the collection block below, the clear in the
            // dismiss handler, and the filtered_cat block at trigger time.
            let mut session_relics: Vec<String> = Vec::new();
            // One diagnostics folder per trigger→dismiss cycle.
            // Created at trigger, BMP written after overlay confirmed, session log at dismiss.
            let diag_arc: Arc<Mutex<Option<std::path::PathBuf>>> = Arc::new(Mutex::new(None));

            // Wake the instant EE.log is written to disk instead of sleeping
            // 200 ms between checks.
            let notifier = open_log_notifier(&log_path);

            loop {
                if !flag.load(Ordering::SeqCst) { break; }
                wait_for_log_change(notifier, std::time::Duration::from_millis(200));
                let Ok(mut f) = std::fs::File::open(&log_path) else { continue };
                let len = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
                if len < file_pos { file_pos = 0; }
                if f.seek(SeekFrom::Start(file_pos)).is_err() { continue; }
                let mut buf = String::new();
                if f.read_to_string(&mut buf).is_err() { continue; }
                file_pos = len;
                if buf.is_empty() { continue; }

                let lower = buf.to_lowercase();

                // ── VoidProjections squad parsing ─────────────────────────────
                // Parse the reward-handshake sequence that fires before the screen opens:
                //   "VoidProjections: GetVoidProjectionReward[s]"  → sequence start
                //   "[id] gets reward /Lotus/..."                  → local player's item
                //   "Still waiting on response from [id]"          → one other player
                //   "Client has reward info for all players now"   → sequence complete
                //
                // squad_size = 1 (local) + count("Still waiting") lines.
                // Logging only for now; item path matching is a future improvement.
                for line in buf.lines() {
                    // Separates "the game fetched inventory" from "some
                    // unrelated allocation moved". The monitor uses it to decide
                    // whether a cache miss is worth a full memory walk.
                    if log_parser::is_inventory_sync_line(line) {
                        blob_sync_pending.store(true, Ordering::SeqCst);
                    }
                    let ll = line.to_lowercase();
                    if ll.contains("voidprojections: getvoidprojectionreward") {
                        vp_in_seq  = true;
                        vp_other_ids.clear();
                        vp_own_item.clear();
                        // Reset the shared mutex so any OCR loop that's still
                        // retrying from a previous fissure doesn't carry a stale
                        // squad count into the next one.
                        if let Ok(mut g) = shared_squad_size2.lock() { *g = None; }
                    }
                    // Capture "gets reward" whenever it appears — inside or outside
                    // the VP sequence. The line fires when the server confirms the local
                    // player's reward assignment, which can happen just after the screen
                    // opens (same EE.log flush, after vp_in_seq has already closed).
                    if ll.contains("gets reward /lotus/") {
                        if let Some(i) = line.find("/Lotus/") {
                            vp_own_item = line[i..].trim().to_string();
                        }
                    }
                    if vp_in_seq {
                        if ll.contains("gets reward /lotus/") {
                            // Already captured above — handled outside the block.
                        } else if ll.contains("still waiting on response from") {
                            // Extract the player ID (last whitespace-separated token)
                            if let Some(id) = ll.split_whitespace().last() {
                                vp_other_ids.insert(id.to_string());
                            }
                        } else if ll.contains("has reward info for all players now") {
                            // squad = local player (1) + unique other IDs seen
                            let squad = (1 + vp_other_ids.len()).clamp(1, 4);
                            // Update the shared mutex so any pending OCR retry reads the correct count.
                            if let Ok(mut g) = shared_squad_size2.lock() { *g = Some(squad); }
                            vp_in_seq = false;
                            vp_seq_completed = true; // fallback trigger signal
                            let _ = append_to_file(&session_log_path, &format!(
                                "[EE.log] VoidProjections squad\n\
                                 ├─ Local item : {}\n\
                                 ├─ Other players (unique IDs) : {}\n\
                                 └─ Squad size : {} total\n\n",
                                if vp_own_item.is_empty() { "(not found)" } else { &vp_own_item },
                                vp_other_ids.len(),
                                squad,
                            ));
                        }
                    }
                }

                // ── Relic prefilter: collect squad projection paths ───────────────
                // "Resource load completed 0x... (/Lotus/Types/Game/Projections/T?VoidProjection...)"
                // fires once per squad member's relic as their loadout is downloaded.
                // All 4 relics appear seconds before the mission starts, well ahead of
                // the reward screen trigger (~200+ seconds later).
                for line in buf.lines() {
                    if line.contains("Resource load completed")
                        && line.contains("/Lotus/Types/Game/Projections/")
                    {
                        if let Some(paren) = line.find("(/Lotus/Types/Game/Projections/") {
                            let rest = &line[paren + 1..]; // skip the '('
                            let path = rest.split(')').next().unwrap_or("").trim().to_string();
                            if !path.is_empty() && !session_relics.contains(&path) {
                                session_relics.push(path.clone());
                            }
                        }
                    }
                }

                // ── Squad member name collection ─────────────────────────────────
                // "AddSquadMember: NAME, mm=..." fires when each squadmate loads in.
                // "Logged in NAME" fires when the local player signs in — their name
                // never appears in AddSquadMember (that's only for squad mates).
                // Both sets feed the OCR filter so usernames don't fuzzy-match items.
                for line in buf.lines() {
                    if line.contains("AddSquadMember: ") {
                        if let Some(after) = line.find("AddSquadMember: ").map(|i| &line[i + 16..]) {
                            if let Some(name) = after.split(',').next().map(str::trim) {
                                if !name.is_empty() {
                                    if let Ok(mut g) = shared_squad_names2.lock() {
                                        if !g.iter().any(|n: &String| n == name) {
                                            g.push(name.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if line.contains(" - new avatar: ") {
                        if let Some(after_bracket) = line.find("]: ").map(|i| &line[i + 3..]) {
                            if let Some(name) = after_bracket.split(" - new avatar:").next() {
                                let name = name.trim();
                                if name.len() >= 3 && !name.contains(' ') {
                                    if let Ok(mut g) = shared_squad_names2.lock() {
                                        if !g.iter().any(|n: &String| n == name) {
                                            g.push(name.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if line.contains("Logged in ") {
                        parse_logged_in_name(line, &shared_squad_names2, &ee_ocr_app);
                    }
                }

                // ── WFM trade whisper detection ──────────────────────────────────
                if lower.contains("(warframe.market)") {
                    // EE.log whisper format: "@From Username : Hi! I want to buy Item for N platinum. (warframe.market)"
                    let raw = buf.as_str();
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
                            rest.find(suffix).map(|j| rest[..j].to_string())
                        })
                    };
                    let price: Option<u64> = raw.find(" for ").and_then(|i| {
                        let rest = &raw[i+5..];
                        rest.find(" platinum").and_then(|j| rest[..j].trim().parse().ok())
                    });
                    let _ = ee_ocr_app.emit("wfm-whisper", serde_json::json!({
                        "from": from,
                        "message": raw.trim(),
                        "item": item,
                        "price": price,
                        "timestamp": chrono::Local::now().format("%H:%M:%S").to_string(),
                    }));
                }

                // Riven trigger and close events are handled exclusively by start_log_watcher
                // (always-on) — do not duplicate them here.

                // Unveil: riven challenge completion
                if lower.contains("modreveal") || (lower.contains("riven") && lower.contains("unveiled")) {
                    let _ = ee_ocr_app.emit("riven-unveiled", ());
                }

                // Trigger: "VoidProjections: GetVoidProjectionReward[s]" fires when the
                // server actually delivers the reward choices to the client — later than
                // the old "initialized" / "openvoidprojectionrewardscreen" lines, which
                // fired before the cards were visible in endless missions.
                // Matching the singular prefix catches both "Reward" and "Rewards" variants.
                let has_trigger = lower.contains("voidprojections: getvoidprojectionreward")
                    || vp_seq_completed;
                vp_seq_completed = false; // consume the flag

                // Dismiss: "Relic reward screen shut down" fires when the player selects
                // a reward (or the countdown expires). DO NOT use "relic timer closed" —
                // that fires at 874.265 when the screen OPENS, not when it closes, causing
                // triggers and dismisses to appear in the same 200ms EE.log flush.
                // "CloseVoidProjectionRewardScreen" fires at the same moment as shut down.
                // "EndSession" is the final fallback for abrupt disconnects/exits.
                // Host migration is NOT a dismiss — the mission continues with a new host.
                let has_dismiss = lower.contains("relic reward screen shut down")
                    || lower.contains("closevoidprojectionrewardscreen")
                    || lower.contains("matchingservice::endsession");

                // ── Dismiss — always processed first (even if same batch as trigger) ──
                if has_dismiss {
                    let dismiss_line = buf.lines()
                        .find(|l| {
                            let ll = l.to_lowercase();
                            ll.contains("relic reward screen shut down")
                                || ll.contains("closevoidprojectionrewardscreen")
                                || ll.contains("matchingservice::endsession")
                        })
                        .unwrap_or("<unknown dismiss line>")
                        .trim()
                        .to_string();
                    let ts_d = chrono::Local::now().format("%H:%M:%S%.3f");
                    let elapsed_s = active_since.map(|t| t.elapsed().as_secs_f64());
                    let dismiss_block = format!(
                        "[STEP 4] DISMISS\n\
                         ├─ Time     : {}\n\
                         ├─ Line     : \"{}\"\n\
                         └─ Open for : {}\n\n",
                        ts_d, dismiss_line,
                        elapsed_s.map(|s| format!("{:.1}s", s)).unwrap_or_else(|| "(unknown)".to_string())
                    );
                    append_to_diag(&session_log_path, &dismiss_block);
                    // Copy the completed session log to the diagnostics folder for this run.
                    if let Ok(mut g) = diag_arc.lock() {
                        if let Some(folder) = g.take() {
                            let _ = std::fs::copy(&session_log_path, folder.join("ocr_session_log.txt"));
                        }
                    }
                    reward_screen_active2.store(false, Ordering::SeqCst);
                    active_since = None;
                    last_dismiss_at = Some(std::time::Instant::now());
                    // Only clear relics on actual mission end. In survival fissures the reward
                    // screen fires "shut down" after every round, but the next round's relic is
                    // selected between that event and the relic selection screen closing. Clearing
                    // here would leave session_relics empty for every round after the first.
                    if lower.contains("matchingservice::endsession") {
                        session_relics.clear();
                    }

                    // ── Immediate inventory update from EE.log reward line ────────
                    // "gets reward /Lotus/StoreItems/..." fires when the player
                    // confirms their reward. Convert to the inventory path and
                    // increment shared_quantities so the UI updates instantly
                    // without waiting for the next memory-scan cycle (~10 s).
                    if !vp_own_item.is_empty() {
                        let store_path = std::mem::take(&mut vp_own_item);
                        let inv_path = store_to_unique(&store_path);
                        let state: tauri::State<AppState> = ee_ocr_app.state();
                        let (old_qty, new_qty) = {
                            let mut qty = state.current_quantities
                                .lock().unwrap_or_else(|e| e.into_inner());
                            let old = *qty.get(&inv_path).unwrap_or(&0);
                            let new = old + 1;
                            qty.insert(inv_path.clone(), new);
                            (old, new)
                        };
                        let item_name = inv_path.split('/').last().unwrap_or("?").to_string();
                        let ts_log = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
                        if let Ok(mut f) = std::fs::OpenOptions::new()
                            .create(true).append(true)
                            .open(&state.changes_log_path)
                        {
                            use std::io::Write;
                            let _ = writeln!(f,
                                "[{}] EE.log Reward | {} | {} → {} (gets reward)",
                                ts_log, item_name, old_qty, new_qty);
                        }
                        let _ = ee_ocr_app.emit("inventory-reward",
                            serde_json::json!({ "path": inv_path, "qty": new_qty }));
                        append_to_diag(&session_log_path, &format!(
                            "[REWARD] Inventory updated from EE.log\n\
                             ├─ Store path : {}\n\
                             ├─ Inv path   : {}\n\
                             └─ Qty        : {} → {}\n\n",
                            store_path, inv_path, old_qty, new_qty
                        ));
                    }

                    // Enforce a minimum 5-second overlay display time. EE.log is
                    // sometimes buffered: the "reward screen shut down" line arrives
                    // in a log flush only 1-2 s after the trigger, even though the
                    // in-game screen was open much longer. Without this guard the
                    // overlay appears and disappears before the user can read it.
                    const MIN_DISPLAY_MS: u64 = 5_000;
                    let emitted_at = rewards_emitted_ms_ee.load(Ordering::SeqCst);
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let elapsed_ms = now_ms.saturating_sub(emitted_at);
                    let delay_ms = if emitted_at > 0 {
                        MIN_DISPLAY_MS.saturating_sub(elapsed_ms)
                    } else {
                        0 // no rewards were emitted this session — dismiss immediately
                    };
                    // Clear the stamp so the next session starts clean.
                    rewards_emitted_ms_ee.store(0, Ordering::SeqCst);

                    let dismiss_app = ee_ocr_app.clone();
                    std::thread::spawn(move || {
                        if delay_ms > 0 {
                            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                        }
                        park_overlay_offscreen(&dismiss_app, "relic-overlay");
                        if let Ok(mut g) = dismiss_app.state::<AppState>().pending_relic_rewards.lock() {
                            *g = None;
                        }
                        let _ = dismiss_app.emit("relic-rewards", serde_json::Value::Null);
                    });
                }

                // ── Trigger: skip if dismiss in same batch, screen already active, or
                //    within 60 s of last dismiss ───────────────────────────────────────
                // active_since.is_some() guards against duplicate triggers: EE.log is
                // polled every 200 ms, and multiple matching lines (e.g. "Client has
                // reward info" + "relic rewards initialized" 250 ms later) can fire in
                // consecutive polls while the same reward screen is still open.  Without
                // this guard, a second OCR task would spawn, emit different card
                // positions, and make the overlay stutter.
                let trigger_allowed = !has_dismiss
                    && active_since.is_none()
                    && last_dismiss_at.map_or(true, |t| t.elapsed().as_secs() >= 5);
                if has_trigger && trigger_allowed {
                    reward_screen_active2.store(true, Ordering::SeqCst);
                    active_since = Some(std::time::Instant::now());

                    // Always ensure the local player's name is in the OCR filter — it may be
                    // absent if FrameForge started after the "Logged in" line was written.
                    if let Ok(local_name) = ee_ocr_app.state::<AppState>().local_player_name.lock() {
                        if let Some(ref name) = *local_name {
                            if let Ok(mut g) = shared_squad_names.lock() {
                                if !g.iter().any(|n: &String| n == name) {
                                    g.push(name.clone());
                                }
                            }
                        }
                    }

                    // Find the exact EE.log line that matched so we can log it
                    let trigger_line = buf.lines()
                        .find(|l| {
                            let ll = l.to_lowercase();
                            ll.contains("voidprojections: getvoidprojectionreward")
                        })
                        .unwrap_or("<unknown trigger line>")
                        .trim()
                        .to_string();

                    let ts0 = chrono::Local::now().format("%H:%M:%S%.3f");

                    // Start a fresh session log for this reward screen
                    let known_names_str = {
                        let names = shared_squad_names.lock()
                            .map(|g| g.clone()).unwrap_or_default();
                        if names.is_empty() {
                            "  (none — names not yet seen in EE.log)".to_string()
                        } else {
                            names.iter().map(|n| format!("  • {}", n)).collect::<Vec<_>>().join("\n")
                        }
                    };
                    // ── Relic prefilter: build narrowed catalog from session relics ──
                    // Union the rewards of all collected relics; fall back to the full
                    // catalog if none were seen (FrameForge started mid-mission, solo, etc.)
                    // To revert: delete this block and restore the two lines below it.
                    let (filtered_cat, prefilter_log) = if !session_relics.is_empty() {
                        // Collect reward display names from Relics.json for all active relics.
                        // Relics.json keys match EE.log paths exactly (full path incl. refinement).
                        // Filter ee_catalog by name — avoids unique_name format mismatches.
                        let allowed_names: std::collections::HashSet<String> = {
                            let state = ee_ocr_app.state::<AppState>();
                            let rw = state.relic_rewards.lock().unwrap_or_else(|e| e.into_inner());
                            session_relics.iter()
                                .filter_map(|p| rw.get(p.as_str()))
                                .flat_map(|rewards| rewards.iter().map(|r| r.name.to_lowercase()))
                                .collect()
                        };
                        if allowed_names.is_empty() {
                            let msg = format!(
                                "  {} relic path(s) found but none matched relic_rewards — using full catalog\n  Paths: {:?}",
                                session_relics.len(), session_relics
                            );
                            (Arc::clone(&ee_catalog), msg)
                        } else {
                            let filtered: Vec<(String, String)> = ee_catalog.iter()
                                .filter(|(_, display_name)| {
                                    let dn = display_name.to_lowercase();
                                    // Relics.json omits " Blueprint" from component names
                                    // (e.g. "Nautilus Prime Carapace") while the item catalog
                                    // stores them as "Nautilus Prime Carapace Blueprint".
                                    // Strip the suffix before comparing so both forms match.
                                    let dn_no_bp = dn.strip_suffix(" blueprint").unwrap_or(&dn);
                                    allowed_names.contains(dn_no_bp) || allowed_names.contains(dn.as_str())
                                })
                                .cloned()
                                .collect();
                            if filtered.is_empty() {
                                let mut sample: Vec<&String> = allowed_names.iter().take(8).collect();
                                sample.sort();
                                let msg = format!(
                                    "  {} relic(s) → 0 catalog matches (allowed_names={}) — using full catalog\n  Relics: {:?}\n  Names sample: {:?}",
                                    session_relics.len(), allowed_names.len(), session_relics, sample
                                );
                                (Arc::clone(&ee_catalog), msg)
                            } else {
                                let msg = format!(
                                    "  {} relic(s) → {} candidates (full catalog: {})\n  Relics: {:?}",
                                    session_relics.len(), filtered.len(), ee_catalog.len(), session_relics
                                );
                                (std::sync::Arc::new(filtered), msg)
                            }
                        }
                    } else {
                        (Arc::clone(&ee_catalog), "  No relics collected — using full catalog (FrameForge started mid-mission?)".to_string())
                    };
                    // ── END relic prefilter ───────────────────────────────────────

                    let write_err = std::fs::write(&session_log_path, format!(
                        "══════════════════════════════════════════════\n\
                         RELIC OVERLAY SESSION — {}\n\
                         ══════════════════════════════════════════════\n\
                         Log path  : {}\n\n\
                         [KNOWN PLAYERS — OCR username filter]\n\
                         {}\n\n\
                         [STEP 1] EE.log TRIGGER\n\
                         ├─ Time     : {}\n\
                         ├─ Line     : \"{}\"\n\
                         ├─ Prefilter: {}\n\
                         └─ Catalog  : {} items\n\n",
                        ts0, session_log_path.display(), known_names_str,
                        ts0, trigger_line, prefilter_log, filtered_cat.len()
                    ));
                    if let Err(e) = write_err {
                        warn!(error = %e, "session log write failed");
                    }
                    // Create one diagnostics folder for this entire run.
                    let run_diag_dir = ee_auto_capture_dir.join(
                        chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string()
                    );
                    let _ = std::fs::create_dir_all(&run_diag_dir);
                    if let Ok(mut g) = diag_arc.lock() { *g = Some(run_diag_dir); }
                    let _ = std::fs::write(&ee_last_path, format!(
                        "=== {} ===\nEE.log trigger fired\n{}\n", ts0, trigger_line
                    ));

                    let _ = ee_ocr_app.emit("ff-status", "🔍 Relic reward screen detected");
                    // Tell App.tsx to pre-create the overlay window NOW, before OCR finishes.
                    // Window creation takes 1-2 s; pre-creating shaves that off the visible delay.
                    let _ = ee_ocr_app.emit("relic-trigger", ());

                    let app          = ee_ocr_app.clone();
                    let cat          = filtered_cat; // relic prefilter (was: Arc::clone(&ee_catalog))
                    let _cat_len     = cat.len();
                    let fallback_cat = Arc::clone(&ee_catalog); // full catalog — used after 3 no-match attempts
                    let lpath        = ee_last_path.clone();
                    let slog         = session_log_path.clone();
                    let active       = reward_screen_active2.clone();
                    let emitted_ms   = rewards_emitted_ms_ocr.clone();
                    let squad_arc    = std::sync::Arc::clone(&shared_squad_size);
                    let names_arc    = std::sync::Arc::clone(&shared_squad_names);
                    let diag_arc2    = Arc::clone(&diag_arc);
                    // Do NOT write ee_squad_size here. The mutex is already reset to None
                    // when GetVoidProjectionRewards fires (above), and is updated to the
                    // correct squad count when the sequence completes (line ~3395).
                    // Writing ee_squad_size here would corrupt the mutex if the sequence
                    // completed in this same poll (the per-line loop runs before this code).

                    tauri::async_runtime::spawn(async move {
                        let deadline = std::time::Instant::now()
                            + std::time::Duration::from_secs(45);
                        // Wait for the VoidProjections EE.log sequence (squad size hint) to
                        // arrive, or proceed after 1500ms if it never comes (solo / missing).
                        // The sequence fires after the server responds to GetVoidProjectionRewards
                        // which can take 800–1500ms after the screen opens. Poll in 100ms ticks.
                        {
                            let hint_deadline = std::time::Instant::now()
                                + std::time::Duration::from_millis(1500);
                            while std::time::Instant::now() < hint_deadline {
                                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                                let has_hint = squad_arc.lock().ok()
                                    .map(|g| g.is_some()).unwrap_or(false);
                                if has_hint { break; }
                            }
                        }

                        // Allow the catalog to be rebuilt inside the loop — it may be empty
                        // when start_monitor fired before WFCD data finished loading.
                        let mut cat = cat;
                        let mut no_match_streak = 0u32;
                        let mut attempt = 0u32;
                        let mut best_item_count = 0usize;
                        let mut best_payload: Option<serde_json::Value> = None; // locked when complete
                        // When no EE squad hint is available, the first "complete" result may
                        // undercount cards (e.g. dark text hides a 2-line item name).
                        // soft_complete_at tracks the first attempt that returned complete-without-hint
                        // so we do one extra retry before locking.
                        let mut soft_complete_at: Option<usize> = None;
                        // Item count at the time soft_complete_at was set.
                        // If the follow-up attempt finds no more items, emit best_payload even if
                        // a newly-arrived EE hint raised estimated_cards above the count we saw.
                        // (Warframe can show fewer unique cards than squad size when players share
                        // the same relic reward — one player lacking reactant is another example.)
                        let mut soft_complete_count: usize = 0;
                        loop {
                            attempt += 1;
                            // Rebuild catalog if WFCD hadn't loaded when this OCR session started.
                            // Runs only while cat is empty — once populated it stays populated.
                            if cat.is_empty() {
                                let s = app.state::<AppState>();
                                let items_lock = s.wfcd_items.lock().unwrap_or_else(|e| e.into_inner());
                                if !items_lock.is_empty() {
                                    let bp_lock = s.blueprint_to_result.lock().unwrap_or_else(|e| e.into_inner());
                                    let bad = ["Warframes","Primary","Secondary","Melee","Companion",
                                               "Sentinels","Archwing","Arch-Gun","Arch-Melee","Pets","Robotic"];
                                    let mut fresh: Vec<(String,String)> = items_lock.iter()
                                        .filter(|i| {
                                            let lo = i.name.to_lowercase();
                                            !bad.contains(&i.category.as_str())
                                            && !lo.ends_with("intact") && !lo.ends_with("exceptional")
                                            && !lo.ends_with("flawless") && !lo.ends_with("radiant")
                                            && (lo.contains("prime") || lo.starts_with("forma"))
                                        })
                                        .map(|i| (i.unique_name.clone(), i.name.clone()))
                                        .collect();
                                    for (u, (n, _)) in bp_lock.iter() {
                                        let lo = n.to_lowercase();
                                        if lo.contains("prime") || lo.starts_with("forma") {
                                            fresh.push((u.clone(), n.clone()));
                                        }
                                    }
                                    fresh.sort_by(|a, b| a.0.cmp(&b.0));
                                    fresh.dedup_by(|a, b| a.0 == b.0);
                                    if !fresh.is_empty() {
                                        cat = std::sync::Arc::new(fresh);
                                    }
                                }
                            }
                            let _ = app.emit("ff-status", "📷 OCR scanning...");
                            let cat2 = std::sync::Arc::clone(&cat);
                            // Clone the Arc so the hint can be read inside spawn_blocking.
                            // Reading AFTER capture (~100-400 ms) rather than before gives the
                            // EE.log VoidProjections sequence time to complete and write the
                            // correct squad count before we decide how many columns to use.
                            let squad_arc2    = std::sync::Arc::clone(&squad_arc);
                            let names_arc2    = std::sync::Arc::clone(&names_arc);
                            let ocr_frame_arc = Arc::clone(&app.state::<AppState>().last_ocr_frame);
                            let result = tauri::async_runtime::spawn_blocking(move || {
                                let (pixels, w, cap_h, full_h, cap_info) =
                                    ocr::capture_warframe_reward_area()?;
                                // Cache the raw frame so auto-capture can write it to disk
                                // without a second GPU readback (no extra GetDIBits stall).
                                if let Ok(mut g) = ocr_frame_arc.lock() {
                                    *g = Some((pixels.clone(), w, cap_h));
                                }
                                // Read hint AFTER capture — the sequence may have completed
                                // during the PrintWindow/DXGI call.
                                let hint_squad = squad_arc2.lock().ok().and_then(|g| *g);
                                let player_names = names_arc2.lock()
                                    .map(|g| g.clone()).unwrap_or_default();
                                Some(ocr::extract_reward_items_twophase(
                                    &pixels, w, cap_h, full_h, &cat2, &cap_info,
                                    hint_squad, &player_names,
                                ))
                            }).await.ok().flatten();
                            // Re-read hint for confirm_ready logic below (same mutex, post-capture value).
                            let hint_squad = squad_arc.lock().ok().and_then(|g| *g);

                            let ts = chrono::Local::now().format("%H:%M:%S%.3f");
                            let sleep_ms = match &result {
                                // ✅ 1+ items found (solo=1, duo=2, trio=3, full squad=4)
                                Some((complete, _, ref items, ref positions, ref dbg)) if !items.is_empty() => {
                                    no_match_streak = 0;
                                    let payload = Some(serde_json::json!({
                                        "items": items, "positions": positions
                                    }));

                                    // Determine whether this complete result should be locked now.
                                    // If we have an EE squad hint the count is authoritative.
                                    // If we don't, wait 3 retries (≈1.2 s) before confirming —
                                    // the VoidProjections EE.log sequence typically arrives 1–2 s
                                    // after the trigger, and we need it before we can validate the
                                    // card count. Waiting 3 extra attempts gives it time to arrive.
                                    let soft_retries_done = soft_complete_at
                                        .map_or(false, |sa| (attempt as usize).saturating_sub(sa) >= 3);
                                    // If the EE hint just arrived saying the squad is LARGER than
                                    // what we matched, suppress confirmation and keep retrying.
                                    // The next pass will use word_card_count = hint_squad, split
                                    // the columns correctly, and find the missing card.
                                    let hint_wants_more = hint_squad
                                        .map_or(false, |h| h > items.len());
                                    let confirm_ready = !hint_wants_more
                                        && (hint_squad.is_some() || soft_retries_done);

                                    // Save best result; only emit to overlay when confirmed (LOCK).
                                    // Partial updates are intentionally suppressed — emitting
                                    // partial data while the user is still hovering cards causes
                                    // the overlay to flicker with wrong items between attempts.
                                    let is_new_best = items.len() > best_item_count;
                                    if is_new_best {
                                        best_item_count = items.len();
                                        best_payload = payload.clone();
                                        let label = if *complete && confirm_ready { "✅" } else { "⚡" };
                                        let status_label = if *complete && confirm_ready { "locked" }
                                            else if *complete { "soft-complete, waiting for EE hint" }
                                            else { "waiting" };
                                        let _ = app.emit("ff-status",
                                            format!("{} {} items ({})", label, items.len(), status_label));
                                        let result_label = if *complete && confirm_ready { "LOCKED & emitting" }
                                            else if *complete { "soft-complete, retrying (waiting for EE hint)" }
                                            else { "saved, retrying" };
                                        let session_entry = format!(
                                            "[STEP 2] OCR ATTEMPT #{}\n\
                                             ├─ Time     : {}\n\
                                             {}\n\
                                             └─ RESULT   : {} items found → {}\n\
                                             └─ Items    : {:?}\n\n",
                                            attempt, ts, dbg, items.len(),
                                            result_label,
                                            items,
                                        );
                                        let _ = append_to_file(&slog, &session_entry);
                                        let _ = std::fs::write(&lpath, format!(
                                            "=== {} ===\nItems: {:?}\n{}\n", ts, items, dbg));
                                    }

                                    // Stop retrying and emit ONLY when all expected cards found AND confirmed.
                                    if *complete {
                                        if confirm_ready {
                                            // Hard cutoff: if dismiss arrived while OCR was running, drop the result.
                                            if !active.load(Ordering::SeqCst) { break; }
                                            // Log the confirming attempt only when the improvement block above
                                            // didn't already log this attempt (is_new_best = false means item
                                            // count didn't change, so the block above was skipped).
                                            if !is_new_best {
                                                let _ = append_to_file(&slog, &format!(
                                                    "[STEP 2] OCR ATTEMPT #{} (confirm)\n\
                                                     ├─ Time     : {}\n\
                                                     └─ {} items — same as before, confirmed\n\n",
                                                    attempt, ts, items.len()
                                                ));
                                            }
                                            let _ = append_to_file(&slog, "[STEP 3] OVERLAY OPENED\n\n");
                                            // Always emit the BEST result captured so far, not the
                                            // current attempt — later attempts may have worse OCR
                                            // quality (player-name pollution, brightness change).
                                            let emit_val = if best_payload.is_some() { &best_payload } else { &payload };
                                            // Store so Overlay.tsx can pull it on mount (race-condition fix).
                                            if let Some(v) = emit_val.as_ref() {
                                                if let Ok(mut g) = app.state::<AppState>().pending_relic_rewards.lock() {
                                                    *g = Some(v.clone());
                                                }
                                            }
                                            let _ = app.emit("relic-rewards", emit_val);
                                            // Record when rewards were emitted so the dismiss handler can
                                            // enforce a minimum display time (see below).
                                            emitted_ms.store(
                                                std::time::SystemTime::now()
                                                    .duration_since(std::time::UNIX_EPOCH)
                                                    .map(|d| d.as_millis() as u64)
                                                    .unwrap_or(0),
                                                Ordering::SeqCst,
                                            );
                                            // After 1.5 s the overlay has finished animating in —
                                            // capture the full desktop (DXGI) so the BMP shows the overlay.
                                            {
                                                let diag_snap = diag_arc2.lock().ok().and_then(|g| g.clone());
                                                if let Some(folder) = diag_snap {
                                                    tauri::async_runtime::spawn(async move {
                                                        tokio::time::sleep(std::time::Duration::from_millis(4000)).await;
                                                        tauri::async_runtime::spawn_blocking(move || {
                                                            if let Some((px, w, h)) = ocr::capture_desktop_for_diag() {
                                                                let _ = write_bmp(&folder.join("screenshot.bmp"), &px, w, h);
                                                            }
                                                        }).await.ok();
                                                    });
                                                }
                                            }
                                            let app2 = app.clone();
                                            let slog2 = slog.clone();
                                            let diag_arc_fb = Arc::clone(&diag_arc2);
                                            let slog_fb = slog.clone();
                                            tauri::async_runtime::spawn(async move {
                                                // 20s safety fallback — normally the overlay closes
                                                // when EE.log fires "relic timer closed" (player picks).
                                                tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                                                if let Ok(mut g) = app2.state::<AppState>().pending_relic_rewards.lock() { *g = None; }
                                                let _ = app2.emit("relic-rewards", serde_json::Value::Null);
                                                park_overlay_offscreen(&app2, "relic-overlay");
                                                append_to_diag(&slog2,
                                                    "[STEP 4] AUTO-DISMISS (20s safety fallback)\n\n");
                                                if let Ok(mut g) = diag_arc_fb.lock() {
                                                    if let Some(folder) = g.take() {
                                                        let _ = std::fs::copy(&slog_fb, folder.join("ocr_session_log.txt"));
                                                    }
                                                }
                                            });
                                            break;
                                        } else {
                                            // Complete result but no EE hint yet — set once and keep
                                            // retrying.  Must NOT overwrite on subsequent iterations
                                            // or the retry counter resets to 1 every loop.
                                            if soft_complete_at.is_none() {
                                                soft_complete_at = Some(attempt as usize);
                                                soft_complete_count = best_item_count;
                                            }
                                        }
                                    } else if soft_complete_at.is_some() && items.len() <= soft_complete_count {
                                        // Soft-complete confirmation retry found no more items.
                                        // A late EE hint may have raised estimated_cards above what
                                        // the screen actually shows (e.g. squad=4 but only 3 unique
                                        // cards because one player lacked reactant or shared a reward).
                                        // Emit best_payload now rather than retrying until timeout.
                                        if !active.load(Ordering::SeqCst) { break; }
                                        let emit_val = best_payload.clone().unwrap_or(serde_json::Value::Null);
                                        if !emit_val.is_null() {
                                            if let Ok(mut g) = app.state::<AppState>().pending_relic_rewards.lock() {
                                                *g = Some(emit_val.clone());
                                            }
                                        }
                                        let _ = app.emit("relic-rewards", &emit_val);
                                        let _ = append_to_file(&slog,
                                            "[STEP 3] OVERLAY OPENED (soft-complete confirmed — no improvement)\n\n");
                                        {
                                            let diag_snap = diag_arc2.lock().ok().and_then(|g| g.clone());
                                            if let Some(folder) = diag_snap {
                                                tauri::async_runtime::spawn(async move {
                                                    tokio::time::sleep(std::time::Duration::from_millis(4000)).await;
                                                    tauri::async_runtime::spawn_blocking(move || {
                                                        if let Some((px, w, h)) = ocr::capture_desktop_for_diag() {
                                                            let _ = write_bmp(&folder.join("screenshot.bmp"), &px, w, h);
                                                        }
                                                    }).await.ok();
                                                });
                                            }
                                        }
                                        let app2 = app.clone();
                                        let slog2 = slog.clone();
                                        let diag_arc_fb = Arc::clone(&diag_arc2);
                                        let slog_fb = slog.clone();
                                        tauri::async_runtime::spawn(async move {
                                            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                                            if let Ok(mut g) = app2.state::<AppState>().pending_relic_rewards.lock() { *g = None; }
                                            let _ = app2.emit("relic-rewards", serde_json::Value::Null);
                                            park_overlay_offscreen(&app2, "relic-overlay");
                                            let _ = append_to_file(&slog2,
                                                "[STEP 4] AUTO-DISMISS (20s safety fallback)\n\n");
                                            if let Ok(mut g) = diag_arc_fb.lock() {
                                                if let Some(folder) = g.take() {
                                                    let _ = std::fs::copy(&slog_fb, folder.join("ocr_session_log.txt"));
                                                }
                                            }
                                        });
                                        break;
                                    }
                                    // Partial result (or soft-complete pending confirmation) — retry
                                    400u64
                                }
                                // ⬛ Dark/blank frame — PrintWindow returned nearly-black
                                Some((_, _, _, _, ref dbg)) if dbg.starts_with("dark-frame") => {
                                    let entry = format!(
                                        "[STEP 2] OCR ATTEMPT #{}\n\
                                         ├─ Time     : {}\n\
                                         └─ RESULT   : {} → PrintWindow returned dark image\n\
                                            Check %TEMP%\\frameforge_capture_debug.bmp\n\
                                            Fix: switch Warframe to Borderless Windowed mode\n\
                                            Retrying in 100ms…\n\n",
                                        attempt, ts, dbg);
                                    let _ = append_to_file(&slog, &entry);
                                    let _ = std::fs::write(&lpath,
                                        format!("=== {} ===\n{} — retrying\n", ts, dbg));
                                    let _ = app.emit("ff-status", format!("⬛ {}", dbg));
                                    100u64
                                }
                                // ⬜ OCR ran but returned no text
                                Some((_, _, _, _, ref dbg)) if dbg.starts_with("ocr-empty") => {
                                    let entry = format!(
                                        "[STEP 2] OCR ATTEMPT #{}\n\
                                         ├─ Time     : {}\n\
                                         └─ RESULT   : {} → image has content but OCR found no text\n\
                                            Check %TEMP%\\frameforge_capture_debug.bmp\n\
                                            Retrying in 300ms…\n\n",
                                        attempt, ts, dbg);
                                    let _ = append_to_file(&slog, &entry);
                                    let _ = std::fs::write(&lpath,
                                        format!("=== {} ===\n{} — retrying\n", ts, dbg));
                                    let _ = app.emit("ff-status", format!("⬜ {}", dbg));
                                    300u64
                                }
                                // ❌ Text found but no catalog match
                                Some((_, _, ref items, _, ref dbg)) => {
                                    no_match_streak += 1;
                                    // After 3 consecutive no-matches on the prefiltered catalog,
                                    // expand to the full item catalog. The prefilter can miss
                                    // items when the collected relic paths don't correspond to
                                    // the relics the squad actually ran (e.g. browsed-but-unused
                                    // relics loaded by the game UI, or the local player's relic
                                    // was consumed and is no longer in inventory).
                                    let expanded = if no_match_streak == 3 && cat.len() < fallback_cat.len() {
                                        cat = Arc::clone(&fallback_cat);
                                        true
                                    } else { false };
                                    let cur_cat_len = cat.len();
                                    let expand_note = if expanded {
                                        format!(" [expanded to full catalog: {}]", cur_cat_len)
                                    } else { String::new() };
                                    let entry = format!(
                                        "[STEP 2] OCR ATTEMPT #{}\n\
                                         ├─ Time     : {}\n\
                                         {}\n\
                                         └─ RESULT   : no catalog match (catalog={}){}→ retrying in 700ms\n\n",
                                        attempt, ts, dbg, cur_cat_len, expand_note);
                                    let _ = append_to_file(&slog, &entry);
                                    let _ = std::fs::write(&lpath, format!(
                                        "=== {} ===\nno match (catalog={}): {:?}\n{}\n",
                                        ts, cur_cat_len, items, dbg));
                                    let _ = app.emit("ff-status", "❌ No catalog match, retrying...");
                                    // On attempt 1, save the captured frame to the diagnostic folder
                                    // so we have a screenshot even when OCR never finds a match.
                                    if attempt == 1 {
                                        let frame = app.state::<AppState>().last_ocr_frame.lock()
                                            .ok().and_then(|g| g.clone());
                                        let diag_snap = diag_arc2.lock().ok().and_then(|g| g.clone());
                                        if let (Some((px, w, h)), Some(folder)) = (frame, diag_snap) {
                                            let _ = write_bmp(&folder.join("screenshot.bmp"), &px, w, h);
                                        }
                                    }
                                    700u64
                                }
                                // ⚠️ Warframe window not found
                                None => {
                                    let entry = format!(
                                        "[STEP 2] OCR ATTEMPT #{}\n\
                                         ├─ Time     : {}\n\
                                         └─ RESULT   : capture failed — Warframe window not found\n\
                                            Retrying in 500ms…\n\n",
                                        attempt, ts);
                                    let _ = append_to_file(&slog, &entry);
                                    let _ = std::fs::write(&lpath,
                                        format!("=== {} ===\nCapture failed (window not found?)\n", ts));
                                    let _ = app.emit("ff-status", "⚠️ Capture failed");
                                    500u64
                                }
                            };

                            if std::time::Instant::now() >= deadline {
                                // Emit best partial result if we found anything, otherwise null.
                                // This means even a timeout shows something rather than nothing
                                // when OCR found cards but couldn't reach the expected count.
                                let emit_val = if active.load(Ordering::SeqCst) {
                                    best_payload.unwrap_or(serde_json::Value::Null)
                                } else {
                                    serde_json::Value::Null
                                };
                                if !emit_val.is_null() {
                                    if let Ok(mut g) = app.state::<AppState>().pending_relic_rewards.lock() {
                                        *g = Some(emit_val.clone());
                                    }
                                }
                                let _ = app.emit("relic-rewards", &emit_val);
                                let _ = append_to_file(&slog,
                                    "[STEP 2] OCR TIMEOUT — 45 seconds elapsed, emitting best result\n\n");
                                park_overlay_offscreen(&app, "relic-overlay");
                                active.store(false, Ordering::SeqCst);
                                if let Ok(mut g) = diag_arc2.lock() {
                                    if let Some(folder) = g.take() {
                                        let _ = std::fs::copy(&slog, folder.join("ocr_session_log.txt"));
                                    }
                                }
                                break;
                            }
                            if !active.load(Ordering::SeqCst) {
                                let _ = append_to_file(&slog,
                                    "[STEP 2] OCR STOPPED — dismiss signal received\n\n");
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
                        }
                    });

                } // end trigger block

                // Auto-dismiss after 20 s — safety net only.
                // Normal close path is EE.log "relic timer closed" above.
                if let Some(since) = active_since {
                    if since.elapsed().as_secs() >= 20 {
                        let ts_a = chrono::Local::now().format("%H:%M:%S%.3f");
                        append_to_diag(&session_log_path, &format!(
                            "[STEP 4] AUTO-DISMISS (20s timeout)\n\
                             ├─ Time     : {}\n\
                             └─ Open for : {:.1}s\n\n",
                            ts_a, since.elapsed().as_secs_f64()
                        ));
                        if let Ok(mut g) = diag_arc.lock() {
                            if let Some(folder) = g.take() {
                                let _ = std::fs::copy(&session_log_path, folder.join("ocr_session_log.txt"));
                            }
                        }
                        reward_screen_active2.store(false, Ordering::SeqCst);
                        active_since = None;
                        last_dismiss_at = Some(std::time::Instant::now());
                        park_overlay_offscreen(&ee_ocr_app, "relic-overlay");
                        if let Ok(mut g) = ee_ocr_app.state::<AppState>().pending_relic_rewards.lock() { *g = None; }
                        let _ = ee_ocr_app.emit("relic-rewards", serde_json::Value::Null);
                    }
                }
            }
        });
    }

    // OCR polling fallback removed — it ran every second with no EE.log context
    // guard, causing false overlays on Mission Complete, orbiter, Last Mission
    // Results, and any screen with Prime item names visible.
    // The EE.log watcher already retries OCR for 45 seconds after the trigger,
    // so the fallback is both redundant and harmful.

    std::thread::spawn(move || {
        // Initialize COM (required for Windows OCR / WinRT APIs).
        // std::thread::spawn creates a raw OS thread with no COM apartment;
        // WinRT calls silently fail without this, returning empty strings.
        #[cfg(target_os = "windows")]
        unsafe {
            windows_sys::Win32::System::Com::CoInitializeEx(
                std::ptr::null(),
                windows_sys::Win32::System::Com::COINIT_MULTITHREADED.try_into().unwrap(),
            );
        }

        while reward_flag.load(Ordering::SeqCst) {
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

    Ok(())
}

/// Extract the local player name from EE.log lines containing "Logged in NAME".
/// Adds the name to shared_squad_names (for OCR filtering) and AppState.local_player_name
/// (for UI display). Safe to call with a single line or the full log contents.
fn parse_logged_in_name(
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

pub(crate) fn append_to_file(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(text.as_bytes())
}

/// Append text to both the global overlay session log and the per-session diagnostic file.
/// The diagnostic target is found by picking the most recently modified folder under
/// the state directory's `diagnostics/` that contains an ocr_session_log.txt.
fn append_to_diag(global_log: &std::path::Path, text: &str) {
    let _ = append_to_file(global_log, text);
    let diag_base = paths::state_dir().join("diagnostics");
    if let Ok(entries) = std::fs::read_dir(&diag_base) {
        let mut folders: Vec<std::path::PathBuf> = entries
            .filter_map(|e| e.ok().map(|d| d.path()))
            .filter(|p| p.is_dir())
            .collect();
        folders.sort();
        if let Some(latest) = folders.last() {
            let diag_log = latest.join("ocr_session_log.txt");
            if diag_log.exists() {
                let _ = append_to_file(&diag_log, text);
            }
        }
    }
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
pub(crate) fn get_monitor_status(state: State<AppState>) -> bool {
    state.monitor_active.load(Ordering::SeqCst)
}

#[cfg(test)]
mod walk_policy_tests {
    use super::{walk_is_due, WALK_COLD_INTERVAL, WALK_MAX_INTERVAL, WALK_MIN_INTERVAL};
    use crate::memory_scanner::ScanOutcome;
    use std::time::Duration;

    /// At the login screen no blob has ever been found, and re-checking that
    /// every WALK_MIN_INTERVAL reads gigabytes and costs the player frames for
    /// seconds at a time.
    #[test]
    fn a_client_with_no_blob_yet_waits_for_the_backstop() {
        let just_walked = WALK_MIN_INTERVAL + Duration::from_secs(1);
        assert!(!walk_is_due(&ScanOutcome::CacheMiss, false, false, just_walked));
        assert!(walk_is_due(&ScanOutcome::CacheMiss, false, false, WALK_COLD_INTERVAL));
    }

    /// A settled inventory answers "unchanged" every couple of seconds for as
    /// long as the player stays docked.
    #[test]
    fn a_settled_inventory_does_not_walk_on_the_minute() {
        assert!(!walk_is_due(&ScanOutcome::Unchanged, false, true, Duration::from_secs(60)));
        assert!(!walk_is_due(&ScanOutcome::Unchanged, false, true, Duration::from_secs(300)));
        assert!(walk_is_due(&ScanOutcome::Unchanged, false, true, WALK_MAX_INTERVAL));
    }

    /// The client announced a fetch and our copy did not move. Usually a sync
    /// with no delta, but it is also what a stale address holding the old bytes
    /// looks like.
    #[test]
    fn an_unchanged_probe_with_a_marker_still_walks() {
        assert!(walk_is_due(&ScanOutcome::Unchanged, true, true, WALK_MIN_INTERVAL));
        assert!(!walk_is_due(&ScanOutcome::Unchanged, true, true, Duration::from_secs(3)));
    }

    /// The probe already delivered the new inventory.
    #[test]
    fn a_fresh_parse_never_escalates() {
        assert!(!walk_is_due(&ScanOutcome::Updated, true, true, Duration::MAX));
    }

    /// Once a blob is known, a miss plausibly means the game reallocated it.
    #[test]
    fn a_miss_on_a_known_blob_keeps_the_old_cadence() {
        assert!(walk_is_due(&ScanOutcome::CacheMiss, false, true, WALK_MIN_INTERVAL));
        assert!(!walk_is_due(&ScanOutcome::CacheMiss, false, true, Duration::from_secs(4)));
    }

    /// The marker resolves the ambiguity, including at the login screen.
    #[test]
    fn a_sync_marker_escalates_immediately() {
        assert!(walk_is_due(&ScanOutcome::CacheMiss, true, false, Duration::ZERO));
        assert!(walk_is_due(&ScanOutcome::CacheMiss, true, true, Duration::ZERO));
    }

    /// The first tick after the game appears has no previous walk to rate-limit
    /// against, so the app still gets one immediately at startup.
    #[test]
    fn the_first_walk_is_never_delayed() {
        assert!(walk_is_due(&ScanOutcome::CacheMiss, false, false, Duration::MAX));
    }
}
