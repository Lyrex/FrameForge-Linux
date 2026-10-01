use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::collections::HashMap;

use tauri::{Emitter, Manager};
use tracing::{debug, error, info, warn};

use crate::app_state::AppState;
use crate::catalogue::{fix_category, DebugUnmatched};
use crate::db::{self, QuantityChange};
use crate::events;
use crate::inventory_state::{build_inventory_from_blob, compare_inventory_quantities, load_inventory_state_cache, persist_complete_inventory, BlobBuildParams};
use crate::mastery::MasteryProvenance;
use crate::mastery_progress::MasteryProgress;
use crate::monitor::{self, BlobStatusPayload, InventoryUpdate, MonitorCatalog, MonitorStartupState};
use crate::platform::{Platform, ProcessAccess};
use crate::{memory_scanner, memory_scanner_linux};

/// Shared state needed by the blob capture thread, extracted from AppState
/// to keep the clone list at the spawn site manageable.
pub(crate) struct BlobCaptureDeps {
    pub app: tauri::AppHandle,
    pub flag: Arc<AtomicBool>,
    pub db_path: std::path::PathBuf,
    pub inventory_state_cache_path: std::path::PathBuf,
    pub mastery_progress: Arc<Mutex<MasteryProgress>>,
    pub shared_quantities: Arc<Mutex<HashMap<String, i64>>>,
    pub shared_unique: Arc<Mutex<HashMap<String, i64>>>,
    pub shared_mods: Arc<Mutex<HashMap<String, memory_scanner::ModCount>>>,
    pub shared_crafting: Arc<Mutex<Vec<monitor::CraftingJob>>>,
    pub blob_log_enabled: Arc<AtomicBool>,
    pub blob_log_dir: std::path::PathBuf,
    pub blob_sync_pending: Arc<AtomicBool>,
    pub debug_cat_enabled: Arc<AtomicBool>,
    pub unmatched_paths_dir: std::path::PathBuf,
    pub force_pid_check: Arc<AtomicBool>,
    pub blob_rx: Receiver<memory_scanner::BlobInventory>,
    pub blob_tx: Sender<memory_scanner::BlobInventory>,
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

/// Spawn the blob-capture thread.
///
/// This thread is responsible for:
/// - Detecting whether Warframe is running (PID check every 5s)
/// - Probing the cached blob every 2 s and walking memory when the probe or
///   the inventory-sync marker says it is worth it
/// - Processing incoming blobs via the channel and updating shared state
/// - Emitting inventory-update events to the UI
pub(crate) fn spawn_blob_capture_thread(
    deps: BlobCaptureDeps,
    catalog: MonitorCatalog,
) {
    std::thread::spawn(move || {
        let BlobCaptureDeps {
            app, flag, db_path, inventory_state_cache_path, mastery_progress,
            shared_quantities, blob_log_enabled, blob_log_dir, blob_sync_pending,
            force_pid_check, blob_rx, blob_tx, ..
        } = &deps;
        let conn = match rusqlite::Connection::open(db_path) {
            Ok(c) => c,
            Err(e) => { error!(error = %e, "monitor DB open failed"); return; }
        };
        let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");

        // Sections seen in the last accepted blob; persisted so the first capture
        // after a restart is already checked for truncation.
        let section_baseline_path = inventory_state_cache_path.with_file_name("section_baseline.json");
        let mut section_baseline = memory_scanner::SectionBaseline::from_keys(
            std::fs::read(&section_baseline_path).ok()
                .and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok())
                .unwrap_or_default(),
        );

        // Content hash of the last blob actually applied; identical re-captures are skipped.
        let mut last_applied_hash: Option<u64> = None;

        let mut inv = monitor::init_monitor_startup_state(
            &deps.shared_quantities, &deps.shared_mods, inventory_state_cache_path,
        );

        // Emit an immediate status before the first scan so the UI shows cached
        // inventory data without waiting for the scan to finish.
        {
            let game_found = Platform::find_warframe_pid().is_some();
            let now_pre = chrono::Utc::now().timestamp();
            let mut initial_qty = inv.known.clone();
            for (k, &amount) in &inv.unique_quantities { initial_qty.entry(k.clone()).or_insert(amount); }
            for (path, mc) in &inv.known_mods { initial_qty.entry(path.clone()).or_insert(mc.total); }
            let _ = app.emit(events::INVENTORY_UPDATE, InventoryUpdate {
                quantities: initial_qty,
                crafting: vec![],
                mastery_rank: inv.current_mastery_rank,
                mastery_data: inv.current_mastery_data.clone(),
                owned_levels: inv.current_owned_levels.clone(),
                changes: vec![],
                consumed_suits: inv.current_consumed_suits.clone(),
                mods: inv.known_mods.clone(),
                socketed_shards: inv.current_socketed_shards.clone(),
                forma_counts: inv.current_forma_counts.clone(),
                warframe_running: game_found,
                scanned_at: now_pre,
                is_full_pass: true,
                player_name: app.state::<AppState>().local_player_name
                    .lock().ok().and_then(|g| g.clone()),
            });
        }

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
            {
                let sq = shared_quantities.lock().unwrap_or_else(|e| e.into_inner());
                let local_has_data = !inv.known.is_empty() || !inv.unique_quantities.is_empty() || !inv.known_mods.is_empty();
                if sq.is_empty() && local_has_data {
                    inv.known.clear();
                    inv.unique_quantities.clear();
                    inv.known_mods.clear();
                    last_applied_hash = None;
                    section_baseline = memory_scanner::SectionBaseline::default();
                }
            }

            let now = chrono::Utc::now().timestamp();

            // Process any incoming blob (non-blocking)
            while let Ok(blob) = blob_rx.try_recv() {
                // Truncation guard: reject a blob that lost a section the previous
                // accepted blob had. Must run before anything is written, since an
                // accepted blob fully replaces the inventory.
                match section_baseline.evaluate(&blob.sections) {
                    Err(missing) => {
                        warn!(?missing, "blob rejected: sections present in last good blob are missing — truncated capture");
                        mastery_progress.lock().unwrap_or_else(|e| e.into_inner()).discard_blob();
                        last_applied_hash = None;
                        // The scanner would otherwise report these bytes as unchanged and
                        // never resend them, so the missing-section streak could not advance.
                        memory_scanner::forget_blob_digest();
                        continue;
                    }
                    Ok(true) => {
                        if let Ok(json) = serde_json::to_string(&section_baseline.keys()) {
                            let _ = crate::cache::atomic_write(&section_baseline_path, json.as_bytes());
                        }
                    }
                    Ok(false) => {}
                }

                // Identical to what is already applied — nothing to do. Still report
                // "done" so the UI doesn't sit on "scanning". The same data seen
                // again is a re-observation of the last confirmed progress.
                if blob.content_hash != 0 && Some(blob.content_hash) == last_applied_hash {
                    debug!("blob unchanged since last apply — skipping");
                    let provenance = {
                        let mut progress = mastery_progress.lock().unwrap_or_else(|e| e.into_inner());
                        progress.reobserve(now).then(|| MasteryProvenance::from(progress.record()))
                    };
                    if let Some(provenance) = provenance {
                        let _ = app.emit(events::MASTERY_OBSERVED, provenance);
                    }
                    let _ = app.emit(events::BLOB_STATUS, BlobStatusPayload {
                        stage: "done".into(),
                        detail: "No changes".into(),
                    });
                    continue;
                }

                last_applied_hash = process_blob(&blob, &deps, &conn, &catalog, &mut inv, now)
                    .then_some(blob.content_hash);
            }

            // A /proc sweep every tick costs more than a 5 s stale PID does.
            // force_pid_check bypasses the cooldown (set by the poke_scan command).
            let forced = force_pid_check.swap(false, Ordering::SeqCst);
            let needs_pid_check = forced || last_pid_check
                .is_none_or(|t: std::time::Instant| t.elapsed().as_secs() >= 5);
            if needs_pid_check {
                let current_pid = Platform::find_warframe_pid();
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

            // The status the UI shows comes from the PID. A game that has
            // started but has not been scanned yet used to keep reading as
            // "not running" until the first blob parse succeeded.
            // While it is not running the payload repeats at most every 30 s.
            // Without that throttle the loop emits identical data every 2 s
            // and triggers a full React render cascade (17 k-item useMemo
            // rebuild).
            let status_changed = game_running != prev_game_running;
            let heartbeat_due = !game_running
                && last_not_running_emit
                    .is_none_or(|t: std::time::Instant| t.elapsed() >= std::time::Duration::from_secs(30));
            if status_changed || heartbeat_due {
                let mut emit_qty = inv.known.clone();
                for (k, &amount) in &inv.unique_quantities { emit_qty.entry(k.clone()).or_insert(amount); }
                for (p, mc) in &inv.known_mods { emit_qty.entry(p.clone()).or_insert(mc.total); }
                let crafting = monitor::build_crafting_jobs(
                    &inv.current_recipes.iter()
                        .map(|r| (r.unique_name.clone(), r.completion_ms))
                        .collect::<Vec<_>>(),
                    &catalog.display_names, &catalog.unique_names,
                );
                // Skip mastery_data on heartbeats — it hasn't changed and spreading 17k
                // entries into React state on every tick is expensive.
                let send_mastery = status_changed;
                let _ = app.emit(events::INVENTORY_UPDATE, InventoryUpdate {
                    quantities: emit_qty, crafting,
                    mastery_rank: inv.current_mastery_rank,
                    mastery_data: if send_mastery { inv.current_mastery_data.clone() } else { HashMap::new() },
                    owned_levels: if send_mastery { inv.current_owned_levels.clone() } else { HashMap::new() },
                    changes: vec![], warframe_running: game_running, scanned_at: now,
                    consumed_suits: inv.current_consumed_suits.clone(),
                    mods: inv.known_mods.clone(),
                    socketed_shards: inv.current_socketed_shards.clone(),
                    forma_counts: inv.current_forma_counts.clone(),
                    is_full_pass: false,
                    player_name: app.state::<AppState>().local_player_name
                        .lock().ok().and_then(|g| g.clone()),
                });
                if !game_running { last_not_running_emit = Some(std::time::Instant::now()); }
            }
            prev_game_running = game_running;

            if game_running {
                // ── Blob capture: cheap probe, rate-limited walk ──────────────
                // The probe runs at PROBE_INTERVAL; re-reading the blob itself is
                // gated additionally by BLOB_PROBE_FALLBACK or the sync marker.
                const PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

                let walk_in_flight = blob_scan_active.load(Ordering::SeqCst);
                let probe_due = last_probe_time
                    .is_none_or(|t: std::time::Instant| t.elapsed() >= PROBE_INTERVAL);

                let mut should_capture = false;
                if probe_due && !walk_in_flight {
                    last_probe_time = Some(std::time::Instant::now());
                    let stitch_due = last_blob_probe
                        .is_none_or(|t: std::time::Instant| t.elapsed() >= BLOB_PROBE_FALLBACK)
                        || blob_sync_pending.load(Ordering::SeqCst)
                        || !memory_scanner::has_cached_blob();
                    let (outcome, sync_marker) = match last_pid {
                        Some(pid) => memory_scanner_linux::probe_tick(pid, blob_tx.clone(), stitch_due),
                        None => (None, false),
                    };
                    if outcome.is_some() {
                        last_blob_probe = Some(std::time::Instant::now());
                    }
                    // A full overview refetch would rerun the planning pass
                    // to move one pill, so the stamp goes out on its own.
                    if outcome == Some(memory_scanner::ScanOutcome::Unchanged) {
                        let provenance = {
                            let mut progress = mastery_progress.lock().unwrap_or_else(|e| e.into_inner());
                            progress.reobserve(now).then(|| MasteryProvenance::from(progress.record()))
                        };
                        if let Some(provenance) = provenance {
                            let _ = app.emit(events::MASTERY_OBSERVED, provenance);
                        }
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
                    let ts     = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string();
                    let dir    = blob_log_dir.clone();
                    let tx     = blob_tx.clone();
                    let save   = blob_log_enabled.load(Ordering::SeqCst);
                    let active = blob_scan_active.clone();
                    let _ = app.emit(events::BLOB_STATUS, BlobStatusPayload {
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
                        let count = memory_scanner_linux::capture_all_blobs(&dir, &ts, tx, save);
                        debug!(files_saved = count, save_flag = save, ts = %ts, "blob capture finished");
                    });
                }
            }

            std::thread::sleep(std::time::Duration::from_secs(2));

            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
}

/// Returns false when the blob is rejected as incomplete and nothing was applied.
fn process_blob(
    blob: &memory_scanner::BlobInventory,
    deps: &BlobCaptureDeps,
    conn: &rusqlite::Connection,
    catalog: &MonitorCatalog,
    inv: &mut MonitorStartupState,
    now: i64,
) -> bool {
    let BlobCaptureDeps {
        app, inventory_state_cache_path, mastery_progress, shared_quantities,
        shared_unique, shared_mods, shared_crafting, debug_cat_enabled,
        unmatched_paths_dir, ..
    } = deps;
    let MonitorCatalog {
        path_to_name, path_to_ducat, path_to_vaulted, path_to_tradable,
        path_to_masterable, path_to_max_level_cap, path_to_category,
        path_to_item_type, path_to_product_category, path_to_wfcd_cat,
        alias_excluded, ignored_paths, unique_names, display_names,
        relic_drops_snapshot,
    } = catalog;

    let existing_wfm: HashMap<String, u32> =
        load_inventory_state_cache(inventory_state_cache_path)
            .items.into_iter()
            .filter_map(|(k, v)| v.wfm_price.map(|p| (k, p)))
            .collect();
    let sc = build_inventory_from_blob(BlobBuildParams {
        blob,
        path_to_name, path_to_category,
        path_to_ducat, path_to_vaulted,
        path_to_tradable, path_to_masterable,
        path_to_max_level_cap,
        relic_drops: relic_drops_snapshot, existing_wfm_prices: &existing_wfm,
        excluded_paths: alias_excluded,
    });
    if !persist_complete_inventory(blob, &inv.unique_quantities, &sc, inventory_state_cache_path) {
        mastery_progress.lock().unwrap_or_else(|e| e.into_inner()).discard_blob();
        return false;
    }
    if blob.mastery_xp.is_none() {
        warn!("XPInfo is not an array; inventory applied, equipment progress left as it was");
    }
    if blob.player_skills.is_none() {
        warn!("PlayerSkills is not an object; inventory applied, Intrinsics progress left as it was");
    }
    if blob.missions.is_none() {
        warn!("Missions is not an array; inventory applied, node progress left as it was");
    }
    if blob.affiliations.is_none() {
        warn!("Affiliations is not an array; inventory applied, standing left as it was");
    }
    if mastery_progress.lock().unwrap_or_else(|e| e.into_inner())
        .apply_blob(blob.mastery_xp.as_ref(), blob.player_skills.as_ref(), blob.missions.as_ref(), blob.affiliations.as_ref(), now)
    {
        let _ = app.emit(events::MASTERY_UPDATE, ());
    }

    // Snapshot previous full inventory (known + uniques + mods) for change detection.
    let prev_all: HashMap<String, i64> = {
        let mut m = inv.known.clone();
        for (k, &amount) in &inv.unique_quantities { m.entry(k.clone()).or_insert(amount); }
        for (p, mc) in &inv.known_mods { m.entry(p.clone()).or_insert(mc.total); }
        m
    };

    inv.known = sc.stackable_quantities();
    inv.unique_quantities = sc.unique_quantities();
    inv.current_socketed_shards = sc.items.iter()
        .filter(|(_, item)| !item.archon_shards.is_empty())
        .map(|(path, item)| (path.clone(), item.archon_shards.clone()))
        .collect();
    inv.current_forma_counts = sc.items.iter()
        .filter_map(|(path, item)| item.forma_count.map(|count| (path.clone(), count)))
        .collect();

    // Mods — full replacement
    inv.known_mods.clear();
    for (path, mc) in &blob.mods {
        inv.known_mods.insert(path.clone(), mc.clone());
    }
    // Rivens — group by item_type so they appear in inventory like regular mods
    for riven in &blob.rivens {
        let mc = inv.known_mods.entry(riven.item_type.clone()).or_default();
        mc.total += riven.count as i64;
        *mc.by_rank.entry(riven.mod_rank).or_insert(0) += riven.count as i64;
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
            for (path, nm) in path_to_name {
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
    inv.current_mastery_rank = Some(blob.mastery_level);
    inv.current_mastery_data = sc.mastery_data();
    inv.current_owned_levels = sc.owned_levels();
    inv.current_consumed_suits = blob.consumed_suits.clone();
    inv.current_recipes = blob.pending_recipes.iter().map(|r| memory_scanner::PendingRecipe {
        unique_name:   r.item_type.clone(),
        completion_ms: r.completion_ms,
    }).collect();

    // Sync shared state
    if let Ok(mut q)  = shared_quantities.lock() { *q = inv.known.clone(); }
    if let Ok(mut sm) = shared_mods.lock()       { *sm = inv.known_mods.clone(); }
    if let Ok(mut uq) = shared_unique.lock() {
        *uq = inv.unique_quantities.clone();
    }

    // Emit inventory update
    let mut emit_qty = inv.known.clone();
    for (k, &amount) in &inv.unique_quantities { emit_qty.entry(k.clone()).or_insert(amount); }
    for (p, mc) in &inv.known_mods { emit_qty.entry(p.clone()).or_insert(mc.total); }

    let mut changes = compare_inventory_quantities(
        &prev_all, &emit_qty, path_to_name, ignored_paths, now,
    );
    for change in &changes {
        let _ = db::add_quantity_change(
            conn, &change.unique_name, &change.item_name, change.old_qty, change.new_qty, None,
        );
    }

    // Rank-specific change detection for mods/arcanes.
    // Compare current by_rank with previous to find which specific rank changed.
    if !inv.prev_mods.is_empty() {
        let ts = chrono::Utc::now().timestamp();
        let all_paths: std::collections::HashSet<&String> =
            inv.prev_mods.keys().chain(inv.known_mods.keys()).collect();
        for path in all_paths {
            if ignored_paths.contains(path.as_str()) { continue; }
            let prev = inv.prev_mods.get(path);
            let current = inv.known_mods.get(path);
            let all_ranks: std::collections::HashSet<u8> = prev.into_iter()
                .flat_map(|mods| mods.by_rank.keys())
                .chain(current.into_iter().flat_map(|mods| mods.by_rank.keys()))
                .cloned()
                .collect();
            for rank in all_ranks {
                let old_count = prev.map(|p| *p.by_rank.get(&rank).unwrap_or(&0)).unwrap_or(0);
                let new_count = current.map(|mods| *mods.by_rank.get(&rank).unwrap_or(&0)).unwrap_or(0);
                if old_count == new_count { continue; }
                let item_name = path_to_name.get(path.as_str())
                    .cloned()
                    .unwrap_or_else(|| path.split('/').next_back().unwrap_or("?").to_string());
                let _ = db::add_quantity_change(conn, path, &item_name, old_count, new_count, Some(rank));
                changes.push(QuantityChange {
                    id: 0,
                    unique_name: path.clone(),
                    item_name,
                    old_qty: old_count,
                    new_qty: new_count,
                    delta: new_count - old_count,
                    timestamp: ts,
                    rank: Some(rank),
                });
            }
        }
    }
    // Update prev_mods for next iteration
    inv.prev_mods = inv.known_mods.clone();

    let crafting = monitor::build_crafting_jobs(
        &blob.pending_recipes.iter()
            .map(|r| (r.item_type.clone(), r.completion_ms))
            .collect::<Vec<_>>(),
        display_names, unique_names,
    );
    *shared_crafting.lock().unwrap_or_else(|e| e.into_inner()) = crafting.clone();
    let _ = app.emit(events::INVENTORY_UPDATE, InventoryUpdate {
        quantities: emit_qty,
        crafting,
        mastery_rank: inv.current_mastery_rank,
        mastery_data: inv.current_mastery_data.clone(),
        owned_levels: inv.current_owned_levels.clone(),
        changes,
        warframe_running: true,
        scanned_at:   now,
        consumed_suits:   inv.current_consumed_suits.clone(),
        mods:             inv.known_mods.clone(),
        socketed_shards:  inv.current_socketed_shards.clone(),
        forma_counts:     inv.current_forma_counts.clone(),
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
    let _ = app.emit(events::BLOB_STATUS, BlobStatusPayload {
        stage: "done".into(),
        detail,
    });

    // Daily snapshots
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    if today != inv.last_snapshot_date {
        inv.last_snapshot_date = today.clone();
        if let Ok(tracked) = db::get_tracked_items(conn) {
            for item in &tracked {
                let qty = *inv.known.get(&item.unique_name).unwrap_or(&0);
                let _ = db::record_snapshot(conn, &item.unique_name, &today, qty);
            }
        }
    }
    true
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
