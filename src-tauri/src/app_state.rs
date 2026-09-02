use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use crate::inventory_state::ApiModCopy;
use crate::monitor::CraftingJob;
use crate::wfcd::{RecipeComponent, SyndicateOffer, WfcdItem};
use crate::wfm::Wfm;
use crate::{memory_scanner, wfcd};

/// Bundled corrections file embedded at compile time. Never absent at runtime.
const BUNDLED_CORRECTIONS: &str = include_str!("../resources/corrections.json");

/// Load and merge corrections: bundled entries first, then user file overrides on a per-path basis.
pub(crate) fn load_corrections(user_path: &std::path::Path) -> HashMap<String, CorrectionEntry> {
    let mut map: HashMap<String, CorrectionEntry> = serde_json::from_str::<Vec<CorrectionEntry>>(BUNDLED_CORRECTIONS)
        .unwrap_or_default()
        .into_iter()
        .map(|e| (e.path.clone(), e))
        .collect();
    if let Ok(content) = std::fs::read_to_string(user_path) {
        if let Ok(entries) = serde_json::from_str::<Vec<CorrectionEntry>>(&content) {
            for e in entries { map.insert(e.path.clone(), e); }
        }
    }
    map
}

/// One entry in corrections.json — a hand-curated override for a specific Lotus path.
/// Fields are all optional so a minimal entry can omit unused columns.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CorrectionEntry {
    pub path:          String,
    /// Display name override. Required unless category is "Ignored".
    pub name:          Option<String>,
    /// Display category override, or "Ignored" to suppress the path everywhere.
    pub category:      Option<String>,
    /// Explicit WFM tradeability flag. `false` means skip all WFM price lookups.
    /// When absent the app auto-detects from ducat_price / category.
    pub tradeable_wfm: Option<bool>,
    /// True when this item is stackable (quantity shown rather than binary owned).
    pub is_stackable:  Option<bool>,
}

pub struct AppState {
    pub db_path: PathBuf,
    pub quantities_cache_path: PathBuf,
    pub inventory_state_cache_path: PathBuf,
    pub settings_path: PathBuf,
    pub log_path: PathBuf,
    pub changes_log_path: PathBuf,
    pub conn: Mutex<rusqlite::Connection>,
    pub wfcd_items: Mutex<Vec<WfcdItem>>,
    /// parent unique_name → recipe component tree
    pub recipes: Mutex<HashMap<String, Vec<RecipeComponent>>>,
    /// component unique_name → relic unique_names that drop it
    pub relic_drops: Mutex<HashMap<String, Vec<String>>>,
    /// relic unique_name → sorted reward list (Bronze×3, Silver×2, Gold×1)
    pub relic_rewards: Mutex<HashMap<String, Vec<wfcd::RelicReward>>>,
    /// blueprint_unique → (display_name, ducats). Used to enrich virtual catalog entries.
    pub blueprint_to_result: Mutex<HashMap<String, (String, Option<u32>)>>,
    /// Canonical relic reward display names from the Warframe Wiki (lower-cased).
    pub wiki_reward_names: Mutex<std::collections::HashSet<String>>,
    /// weapon unique_name → riven disposition (omegaAttenuation). Populated from All.json.
    pub weapon_dispositions: Mutex<HashMap<String, f32>>,
    /// Last-known quantities from memory scans. Shared with monitor thread.
    pub current_quantities: Arc<Mutex<HashMap<String, i64>>>,
    /// Stable unique items (weapons/warframes) seen in 2+ consecutive scans.
    /// Exposed so get_current_quantities can return them for overlay ownership checks.
    pub unique_quantities: Arc<Mutex<HashMap<String, i64>>>,
    /// Mod/arcane inventory: unique_name → {total, by_rank}. Shared with monitor thread.
    /// API data is merged in when available; falls back to scanner-only totals.
    pub current_mods: Arc<Mutex<HashMap<String, memory_scanner::ModCount>>>,
    /// Last-known crafting jobs from memory scans. Shared with monitor thread.
    pub current_crafting: Arc<Mutex<Vec<CraftingJob>>>,
    pub monitor_active: Arc<AtomicBool>,
    /// Controls the raw memory string-dump background thread.
    pub raw_scan_active: Arc<AtomicBool>,
    pub raw_scan_path: PathBuf,
    /// Set by the EE.log tail when Warframe reports finishing an inventory
    /// refresh; cleared by the monitor loop when it acts on it. A bool rather
    /// than a count because the game flushes its log in bursts, so several
    /// markers can land at once and all of them call for the same single walk.
    pub blob_sync_pending: Arc<AtomicBool>,
    /// When true, save a timestamped inventory blob to blobs/ on each full scan pass.
    pub blob_log_enabled: Arc<AtomicBool>,
    pub blob_log_dir: PathBuf,
    /// When true, save the raw DE API response to api_logs/ on each fetch.
    pub api_log_enabled: Arc<AtomicBool>,
    pub api_log_dir: PathBuf,
    /// The warframe.market client: session, rate limiters, and the slug → price
    /// cache all live behind this one seam, shared (Arc) with the prefetch thread.
    pub wfm: Arc<Wfm>,
    /// Slugs waiting for a price fetch (normal priority). Drained by the WFM queue thread.
    pub wfm_price_queue: Arc<Mutex<std::collections::VecDeque<String>>>,
    /// High-priority slugs (popup / on-demand). Drained before wfm_price_queue.
    pub wfm_priority_queue: Arc<Mutex<std::collections::VecDeque<String>>>,
    /// Set to true once the WFM queue drain thread has been started.
    pub wfm_queue_started: Arc<AtomicBool>,
    /// syndicate name → purchasable items (all known syndicates)
    pub syndicate_catalog: Mutex<HashMap<String, Vec<SyndicateOffer>>>,
    /// IDs of riven auctions created via FrameForge — persisted so hidden auctions survive restarts.
    pub auction_ids: Mutex<Vec<String>>,
    pub auction_ids_path: PathBuf,
    /// Companion API quantities held in memory so the scanner includes them in cache writes.
    pub api_quantities_cache: Arc<Mutex<HashMap<String, i64>>>,
    /// Companion API mod copies held in memory so the scanner includes them in cache writes.
    pub api_mod_copies_cache: Arc<Mutex<Vec<ApiModCopy>>>,
    /// Most recent OCR frame (top ~48% of Warframe window, BGRA, width, height).
    /// Stored by the OCR loop so auto-capture can write it without a second GPU readback.
    pub last_ocr_frame: Arc<Mutex<Option<(Vec<u8>, u32, u32)>>>,
    /// Local image cache directory — craftable item images downloaded here on first run.
    pub img_cache_dir: PathBuf,
    /// Port of the local HTTP image server (set in setup hook, 0 until started).
    pub img_server_port: Mutex<u16>,
    /// Local Warframe account name extracted from EE.log "Logged in NAME".
    /// Used to filter the player's own name from OCR captures and to display in the UI.
    pub local_player_name: Arc<Mutex<Option<String>>>,
    /// Last successfully locked relic reward payload { items, positions }.
    /// Written when the OCR loop emits relic-rewards; cleared on dismiss or when read.
    /// Overlay.tsx pulls this on mount so it never misses rewards that arrived before
    /// its relic-rewards listener was registered.
    pub pending_relic_rewards: Mutex<Option<serde_json::Value>>,
    /// relics.run daily bulk price cache: item display name (lowercase) → median sell price.
    pub relics_run_prices: Mutex<HashMap<String, u32>>,
    /// Raw worldstate + Steam news from the last upstream fetch, with the time it
    /// was taken. Every window polls worldstate on its own timer, so without this
    /// two open windows mean two fetch pairs a minute against DE and Steam.
    /// Only the network payload is cached — parsing still runs per call, so
    /// activation/expiry filtering stays anchored to the current time. Held
    /// behind `Arc` so serving a hit shares the ~1MB tree instead of cloning it.
    pub worldstate_cache: Mutex<Option<(std::time::Instant, Arc<serde_json::Value>, Arc<serde_json::Value>)>>,
    /// When true, unmatched inventory paths are written to the Unmatched Paths debug folder.
    pub debug_cat_enabled: Arc<AtomicBool>,
    /// Subfolders of `Debugging/` in the state directory.
    pub auto_capture_dir: PathBuf,
    pub manual_capture_dir: PathBuf,
    pub memory_probe_path: PathBuf,
    pub unmatched_paths_dir: PathBuf,
    /// Merged bundled + user corrections: path → entry.
    /// Bundled file is embedded at compile time; user file from data dir overrides on a per-path basis.
    pub corrections: HashMap<String, CorrectionEntry>,
    /// Set by `poke_scan` to bypass the 5-second PID-check cooldown immediately.
    pub force_pid_check: Arc<AtomicBool>,
    /// When false, the Relic Pick Overlay is suppressed even when EE.log triggers it.
    pub relic_pick_overlay_enabled: Arc<AtomicBool>,
}
