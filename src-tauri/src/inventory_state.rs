use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::warn;
use crate::app_state::AppState;
use crate::cache::atomic_write;
use crate::db::QuantityChange;
use crate::memory_scanner;

pub struct BlobBuildParams<'a> {
    pub(crate) blob: &'a memory_scanner::BlobInventory,
    pub(crate) path_to_name: &'a HashMap<String, String>,
    pub(crate) path_to_category: &'a HashMap<String, String>,
    pub(crate) path_to_ducat: &'a HashMap<String, u32>,
    pub(crate) path_to_vaulted: &'a HashMap<String, bool>,
    pub(crate) path_to_tradable: &'a HashMap<String, bool>,
    pub(crate) path_to_masterable: &'a HashMap<String, bool>,
    pub(crate) relic_drops: &'a HashMap<String, Vec<String>>,
    pub(crate) existing_wfm_prices: &'a HashMap<String, u32>,
    pub(crate) excluded_paths: &'a std::collections::HashSet<String>,
}

/// Subsumed warframes from the persisted cache, so the Foundry shows them
/// before the first scan pass completes.
#[tauri::command]
pub(crate) fn get_saved_consumed_suits(state: tauri::State<'_, AppState>) -> Vec<String> {
    load_inventory_state_cache(&state.inventory_state_cache_path).consumed_suits()
}

/// One resolved modular component (Amp Prism/Scaffold/Brace, Kitgun barrel, etc.)
/// stored inside the parent item's cache entry.
#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
pub(crate) struct ModularPart {
    path: String,
    name: String,
}

/// One item's complete persisted state — all data for a single inventory entry in one place.
#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
pub(crate) struct CachedItem {
    /// Lotus path — stable cross-session identifier.
    pub(crate) unique_name: String,
    /// Human-readable display name (populated from WFCD catalog when available).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) name: String,
    /// Total owned copies (or quantity for stackable resources).
    #[serde(default)]
    pub(crate) amount: i64,
    /// Mastery rank 0-30 (0 = not mastered or not applicable).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub(crate) mastery_rank: u32,
    /// Socketed Archon Shards (warframes only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) archon_shards: Vec<memory_scanner::ArchonShard>,
    /// Resolved modular components (Amp Prism/Scaffold/Brace, Kitgun parts, etc.).
    /// Populated from the blob's ModularParts array with names looked up from WFCD + corrections.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) modular_parts: Vec<ModularPart>,
    /// Maximum rank this mod/arcane can reach (from WFCD fusionLimit). Absent for non-mod items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mod_max_rank: Option<u32>,
    /// Maximum level cap override (from WFCD maxLevelCap). Only set for items that exceed rank 30
    /// (e.g. Paracesis, Ironbride, Necramechs). Absent when the standard 30-cap applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_level_cap: Option<u32>,
    /// Mod/arcane rank breakdown: rank (as string) → copy count at that rank.
    /// Present only for mods and arcanes. Sum of values equals `amount`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mod_ranks: Option<HashMap<String, i64>>,
    /// Number of Forma applied (placeholder — not yet scanned, reserved for future use).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) forma_count: Option<u32>,
    /// True when this warframe has been fed to the Helminth (subsumed).
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) subsumed: bool,
    /// Ducat trade-in value from the WFCD catalog (prime parts/blueprints only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ducat_price: Option<u32>,
    /// Last-fetched warframe.market 48-hour median sell price (platinum).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) wfm_price: Option<u32>,
    /// Whether this item is currently vaulted (None = not applicable / unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) vaulted: Option<bool>,
    /// Normalised item category (Warframes, Weapons, Mods, Parts, Blueprints, Resources, …).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) category: String,
    /// True when this item can drop from void relics.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) relic_reward: bool,
    /// Whether this item can be traded between players (from WFCD).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tradable: Option<bool>,
    /// Whether levelling this item grants mastery XP (from WFCD).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) masterable: Option<bool>,
    /// True when this item is listed and tradeable on warframe.market.
    /// Set to false if a WFM price fetch confirmed the item is not listed.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) tradeable_wfm: bool,
    /// True when this item was detected via the FlavourItems array (glyphs, skins,
    /// colour palettes, animation sets, etc.).
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) is_flavour: bool,
    /// True when this item came from MiscItems (stackable resources/relics) or
    /// FlavourItems/WeaponSkins (occurrence-counted cosmetics). Prevents items
    /// whose Lotus path matches is_unique_path() (e.g. Kubrow Eggs, Kavat Genetic
    /// Codes, helmets under /Lotus/Powersuits/) from being treated as binary-owned
    /// on startup, which would cause spurious 1→N change log entries every session.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) is_stackable: bool,
}

fn is_false(v: &bool) -> bool { !v }

fn is_zero_u32(v: &u32) -> bool { *v == 0 }

/// Full inventory snapshot persisted to disk. Survives app restarts.
#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
pub(crate) struct InventoryStateCache {
    /// All owned items: unique_name → item entry.
    #[serde(default)]
    pub(crate) items: HashMap<String, CachedItem>,
    /// Player-level mastery rank (separate from per-item ranks).
    #[serde(default)]
    pub(crate) mastery_rank: Option<u32>,
    /// All owned riven mods (veiled and revealed), populated from blob scans.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) rivens: Vec<memory_scanner::BlobRivenEntry>,
}

impl InventoryStateCache {
    pub(crate) fn unique_quantities(&self) -> HashMap<String, i64> {
        self.items.iter()
            .filter(|(path, item)| item.amount > 0 && item.mod_ranks.is_none()
                && !item.subsumed && !item.is_stackable && !item.is_flavour
                && !matches!(item.category.as_str(), "Blueprints" | "Parts")
                && is_unique_path(path))
            .map(|(path, item)| (path.clone(), item.amount))
            .collect()
    }

    pub(crate) fn stackable_quantities(&self) -> HashMap<String, i64> {
        self.items.iter()
            // Currency zeroes distinguish an accepted empty inventory from a cache reset.
            .filter(|(path, item)| (item.amount > 0 || path.starts_with("/_currency/")) && (item.is_flavour
                || (item.mod_ranks.is_none() && (item.is_stackable
                    || !is_unique_path(path)
                    || matches!(item.category.as_str(), "Blueprints" | "Parts")))))
            .map(|(path, item)| (path.clone(), item.amount))
            .collect()
    }

    /// Derive consumed_suits from items so callers don't need to know the internal layout.
    pub(crate) fn consumed_suits(&self) -> Vec<String> {
        self.items.iter()
            .filter(|(_, v)| v.subsumed)
            .map(|(k, _)| k.clone())
            .collect()
    }
}

/// True for unique items tracked by the unique scanner (warframes, weapons, companions,
/// archwings, sentinels). These are seeded into unique_quantities on startup.
/// Glyphs and sigils are intentionally excluded — they are detected via FlavourItems
/// and seeded through initial_quantities like stackable resources.
pub(crate) fn is_unique_path(p: &str) -> bool {
    p.starts_with("/Lotus/Powersuits/")
        || p.starts_with("/Lotus/Weapons/")
        || p.starts_with("/Lotus/Archwing/")
        || p.starts_with("/Lotus/Types/Sentinels/SentinelPowersuits/")
        || p.starts_with("/Lotus/Types/Sentinels/SentinelWeapons/")
        || p.starts_with("/Lotus/Types/Friendly/")
        || (p.starts_with("/Lotus/Types/Game/CatbrowPet/") && !p.contains("/Colors/"))
        || (p.starts_with("/Lotus/Types/Game/KubrowPet/") && !p.contains("/Colors/"))
        || p.starts_with("/Lotus/Types/Game/CrewShip/")
        || p.starts_with("/Lotus/Types/Enemies/")
}


pub(crate) fn inventory_path_aliases() -> HashMap<&'static str, &'static str> {
    [
        // Sirius & Orion: two WFCD entries for one warframe.
        ("/Lotus/Powersuits/SiriusOrion/OrionSuit",
         "/Lotus/Powersuits/SiriusOrion/SiriusSuit"),
        ("/Lotus/Powersuits/SiriusOrion/OrionSuitBlueprint",
         "/Lotus/Types/Recipes/WarframeRecipes/SiriusOrionBlueprint"),
    ].into_iter().collect()
}

/// Build a fresh `InventoryStateCache` from a parsed FULL_ACCOUNT blob.
/// All sections are authoritative — this fully replaces scanner-derived data.
pub(crate) fn build_inventory_from_blob(
    params: BlobBuildParams<'_>,
) -> InventoryStateCache {
    let BlobBuildParams {
        blob, path_to_name, path_to_category, path_to_ducat, path_to_vaulted,
        path_to_tradable, path_to_masterable, relic_drops, existing_wfm_prices, excluded_paths,
    } = params;
    let mut items: HashMap<String, CachedItem> = HashMap::new();

    macro_rules! upsert {
        ($path:expr) => {{
            let p: &str = $path;
            items.entry(p.to_string()).or_insert_with(|| CachedItem {
                unique_name: p.to_string(),
                name: path_to_name.get(p).cloned().unwrap_or_default(),
                ..Default::default()
            })
        }};
    }

    // Currency (virtual paths not in WFCD catalog).
    upsert!("/_currency/Credits").amount     = blob.credits;
    upsert!("/_currency/Endo").amount        = blob.endo;
    upsert!("/_currency/Platinum").amount    = blob.platinum - blob.free_platinum;
    upsert!("/_currency/PlatinumGift").amount = blob.free_platinum;

    let path_aliases = inventory_path_aliases();

    // Ordinary weapons are binary-owned; modular components count each instance.
    for entry in &blob.unique_items {
        // Amps: key by Prism (Barrel) path instead of the generic OperatorAmpWeapon type.
        // Must come before the excluded_paths guard because OperatorAmpWeapon is Ignored
        // (suppressed from the catalog) but the Prism-specific path is not.
        if entry.section == "OperatorAmps" {
            let prism_path = entry.modular_parts.iter()
                .find(|p| p.contains("Barrel"))
                .cloned()
                .unwrap_or_else(|| entry.item_type.clone());
            if excluded_paths.contains(&prism_path) { continue; }
            let item = items.entry(prism_path.clone()).or_insert_with(|| CachedItem {
                unique_name: prism_path.clone(),
                name: path_to_name.get(&prism_path).cloned().unwrap_or_default(),
                ..Default::default()
            });
            item.amount += 1;
            if entry.item_name.is_some() {
                let rank = memory_scanner::xp_to_rank(entry.xp, &entry.item_type).min(30);
                if rank > item.mastery_rank { item.mastery_rank = rank; }
            }
            continue;
        }

        // Zaws: key by Strike (Tip) path instead of the generic LotusModularWeapon type.
        // Must come before the excluded_paths guard for the same reason as Amps above.
        if entry.section == "Melee" && entry.item_type.contains("LotusModularWeapon") {
            let strike_path = entry.modular_parts.iter()
                .find(|p| p.contains("/Tip"))
                .cloned()
                .unwrap_or_else(|| entry.item_type.clone());
            if excluded_paths.contains(&strike_path) { continue; }
            let item = items.entry(strike_path.clone()).or_insert_with(|| CachedItem {
                unique_name: strike_path.clone(),
                name: path_to_name.get(&strike_path).cloned().unwrap_or_default(),
                ..Default::default()
            });
            item.amount += 1;
            if entry.item_name.is_some() {
                let rank = memory_scanner::xp_to_rank(entry.xp, &entry.item_type).min(30);
                if rank > item.mastery_rank { item.mastery_rank = rank; }
            }
            continue;
        }

        let canonical = path_aliases.get(entry.item_type.as_str()).copied()
            .unwrap_or(&entry.item_type);
        if excluded_paths.contains(canonical) { continue; }

        let item = upsert!(canonical);
        item.amount        = 1;
        item.archon_shards = entry.archon_shards.clone();
        if entry.polarized > 0 { item.forma_count = Some(entry.polarized); }
        if !entry.modular_parts.is_empty() {
            item.modular_parts = entry.modular_parts.iter()
                .map(|p| ModularPart {
                    path: p.clone(),
                    name: path_to_name.get(p).cloned().unwrap_or_default(),
                })
                .collect();
        }
    }

    // Subsumed warframes (InfestedFoundry.ConsumedSuits).
    for path in &blob.consumed_suits {
        if excluded_paths.contains(path) { continue; }
        upsert!(path).subsumed = true;
    }

    // Stackable items — resources, relics, blueprints, ayatan, decorations.
    for entry in &blob.stackable_items {
        if excluded_paths.contains(&entry.item_type) { continue; }
        if entry.item_count <= 0 { continue; }
        // Don't overwrite modular entries already written by the Amp/Zaw branches above.
        if items.contains_key(&entry.item_type) { continue; }
        let item = upsert!(&entry.item_type);
        item.amount      = entry.item_count;
        item.is_stackable = true;
    }

    // Mods and arcanes (merged from RawUpgrades + Upgrades).
    for (path, mc) in &blob.mods {
        if excluded_paths.contains(path) { continue; }
        let item = upsert!(path);
        item.amount    = mc.total;
        item.mod_ranks = Some(mc.by_rank.iter().map(|(&r, &c)| (r.to_string(), c)).collect());
    }

    // Rivens — group by item_type so they land in `items` with mod_ranks.
    // This ensures the startup cache seeds known_mods with riven counts, preventing
    // spurious 0→N change log entries on every app restart.
    let mut riven_counts: HashMap<String, memory_scanner::ModCount> = HashMap::new();
    for riven in &blob.rivens {
        let mc = riven_counts.entry(riven.item_type.clone()).or_default();
        mc.total += riven.count as i64;
        *mc.by_rank.entry(riven.mod_rank).or_insert(0) += riven.count as i64;
    }
    for (path, mc) in &riven_counts {
        if excluded_paths.contains(path) { continue; }
        let item = upsert!(path);
        item.amount    = mc.total;
        item.mod_ranks = Some(mc.by_rank.iter().map(|(&r, &c)| (r.to_string(), c)).collect());
    }

    // FlavourItems (glyphs, palettes, emotes, titles, ship skins) and
    // WeaponSkins (sigils, cosmetic overlays): occurrence count = amount owned.
    for (path, &count) in blob.flavour_items.iter().chain(blob.weapon_skins.iter()) {
        if excluded_paths.contains(path) { continue; }
        let item = upsert!(path);
        item.amount      = count;
        item.is_flavour  = true;
        item.is_stackable = true; // cosmetics can have count > 1; never treat as binary-owned
    }

    // Mastery rank per item from XPInfo. Cap at 30 — raw XP can yield uncapped
    // values (excess affinity beyond rank 30), matching the .min(30) applied to
    // Amps and Zaws above.
    for (path, &rank) in &blob.mastery_data {
        if rank > 0 { upsert!(path).mastery_rank = rank.min(30); }
    }

    // Catalog-derived fields + carry forward fetched WFM prices.
    for (path, item) in items.iter_mut() {
        item.ducat_price  = path_to_ducat.get(path).copied();
        item.vaulted      = path_to_vaulted.get(path).copied();
        item.tradable     = path_to_tradable.get(path).copied();
        item.masterable   = path_to_masterable.get(path).copied();
        item.category     = path_to_category.get(path).cloned().unwrap_or_default();
        item.relic_reward = relic_drops.contains_key(path.as_str());
        let tradeable = item.ducat_price.is_some()
            || matches!(item.category.as_str(), "Mods" | "Arcanes");
        item.tradeable_wfm = tradeable;
        if tradeable {
            if let Some(&p) = existing_wfm_prices.get(path) { item.wfm_price = Some(p); }
        }
    }

    for path in excluded_paths { items.remove(path); }

    InventoryStateCache {
        items,
        mastery_rank: if blob.mastery_level > 0 { Some(blob.mastery_level) } else { None },
        rivens: blob.rivens.clone(),
    }
}

pub(crate) fn load_inventory_state_cache(path: &PathBuf) -> InventoryStateCache {
    std::fs::read_to_string(path).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub(crate) fn persist_complete_inventory(
    blob: &memory_scanner::BlobInventory,
    previous_unique: &HashMap<String, i64>,
    cache: &InventoryStateCache,
    path: &Path,
) -> bool {
    // Parsing checks required sections; an empty unique section can still be incomplete.
    // Reject before writing so a partial scan cannot become the restart baseline.
    if blob.unique_items.is_empty() && !previous_unique.is_empty() {
        warn!("blob rejected at commit: 0 unique items vs {} previously — incomplete blob", previous_unique.len());
        return false;
    }
    if let Ok(json) = serde_json::to_string(cache) {
        let _ = atomic_write(path, json.as_bytes());
    }
    true
}

pub(crate) fn compare_inventory_quantities(
    previous: &HashMap<String, i64>,
    current: &HashMap<String, i64>,
    names: &HashMap<String, String>,
    ignored: &std::collections::HashSet<String>,
    timestamp: i64,
) -> Vec<QuantityChange> {
    if previous.is_empty() { return vec![]; }
    let keys: std::collections::HashSet<&String> = previous.keys().chain(current.keys()).collect();
    keys.into_iter().filter_map(|key| {
        if ignored.contains(key) { return None; }
        let old_qty = previous.get(key).copied().unwrap_or(0);
        let new_qty = current.get(key).copied().unwrap_or(0);
        if old_qty == new_qty { return None; }
        Some(QuantityChange {
            id: 0,
            unique_name: key.clone(),
            item_name: names.get(key).cloned()
                .unwrap_or_else(|| key.split('/').next_back().unwrap_or("?").to_string()),
            old_qty, new_qty, delta: new_qty - old_qty, timestamp, rank: None,
        })
    }).collect()
}

#[cfg(test)]
mod inventory_quantity_tests {
    use super::*;

    const ALIAS: &str = "/Lotus/Powersuits/SiriusOrion/OrionSuit";
    const CANONICAL: &str = "/Lotus/Powersuits/SiriusOrion/SiriusSuit";

    fn unique(path: &str, section: &str, parts: &[&str]) -> memory_scanner::BlobUniqueEntry {
        memory_scanner::BlobUniqueEntry {
            item_type: path.into(), section: section.into(), polarized: 0, xp: 0,
            item_name: None, pet_name: None, focus_lens: None,
            archon_shards: vec![], modular_parts: parts.iter().map(|p| p.to_string()).collect(),
        }
    }

    fn cache(blob: &memory_scanner::BlobInventory) -> InventoryStateCache {
        build_inventory_from_blob(BlobBuildParams {
            blob,
            path_to_name: &HashMap::new(), path_to_category: &HashMap::new(),
            path_to_ducat: &HashMap::new(), path_to_vaulted: &HashMap::new(),
            path_to_tradable: &HashMap::new(), path_to_masterable: &HashMap::new(),
            relic_drops: &HashMap::new(), existing_wfm_prices: &HashMap::new(),
            excluded_paths: &[ALIAS.to_string(), AMP.into(), ZAW.into()].into(),
        })
    }

    const AMP: &str = "/Lotus/Weapons/Sentients/OperatorAmplifiers/OperatorAmpWeapon";
    const PRISM: &str = "/Lotus/Weapons/Sentients/OperatorAmplifiers/Set1/Barrel/SentAmpSet1BarrelPartA";
    const ZAW: &str = "/Lotus/Weapons/Ostron/Melee/LotusModularWeapon";
    const STRIKE: &str = "/Lotus/Weapons/Ostron/Melee/ModularMelee01/Tip/TipOne";
    const COSMETIC: &str = "/Lotus/Powersuits/Operator/VahdCuirass";

    fn modular_blob() -> memory_scanner::BlobInventory {
        let mut blob = memory_scanner::BlobInventory::default();
        for _ in 0..2 {
            blob.unique_items.push(unique(AMP, "OperatorAmps", &[PRISM]));
            blob.unique_items.push(unique(ZAW, "Melee", &[STRIKE]));
        }
        blob.flavour_items.insert(COSMETIC.into(), 2);
        blob
    }

    fn quantities(cache: &InventoryStateCache) -> HashMap<String, i64> {
        let mut quantities = cache.stackable_quantities();
        quantities.extend(cache.unique_quantities());
        for (path, item) in &cache.items {
            if item.mod_ranks.is_some() {
                quantities.entry(path.clone()).or_insert(item.amount);
            }
        }
        quantities
    }

    fn changes(previous: &HashMap<String, i64>, current: &HashMap<String, i64>) -> Vec<QuantityChange> {
        compare_inventory_quantities(
            previous, current,
            &[(PRISM.into(), "Raplak Prism".into()), (STRIKE.into(), "Balla".into()),
                (COSMETIC.into(), "Vahd Cuirass".into())].into(),
            &[AMP.into(), ZAW.into()].into(), 123,
        )
    }

    #[test]
    fn restart_and_repeated_scans_preserve_modular_and_cosmetic_counts() {
        let blob = modular_blob();
        let saved = serde_json::to_string(&cache(&blob)).expect("cache serializes");
        let restarted: InventoryStateCache = serde_json::from_str(&saved).expect("valid cache");
        let mut previous = quantities(&restarted);
        assert_eq!(previous, [(PRISM.into(), 2), (STRIKE.into(), 2), (COSMETIC.into(), 2),
            ("/_currency/Credits".into(), 0), ("/_currency/Endo".into(), 0),
            ("/_currency/Platinum".into(), 0), ("/_currency/PlatinumGift".into(), 0)].into());
        for _ in 0..3 {
            let current = quantities(&cache(&blob));
            assert!(changes(&previous, &current).is_empty());
            previous = current;
        }
        assert!(!previous.contains_key(AMP));
        assert!(!previous.contains_key(ZAW));
    }

    #[test]
    fn gains_removals_and_zero_keep_component_names() {
        for (path, name) in [(PRISM, "Raplak Prism"), (STRIKE, "Balla"), (COSMETIC, "Vahd Cuirass")] {
            for (old, new) in [(2, 3), (2, 1), (2, 0), (0, 1)] {
                let mut before = modular_blob();
                let mut after = modular_blob();
                for (blob, count) in [(&mut before, old), (&mut after, new)] {
                    if path == COSMETIC {
                        blob.flavour_items.insert(path.into(), count);
                    } else {
                        blob.unique_items.retain(|entry| !entry.modular_parts.iter().any(|p| p == path));
                        for _ in 0..count {
                            blob.unique_items.push(if path == PRISM {
                                unique(AMP, "OperatorAmps", &[PRISM])
                            } else {
                                unique(ZAW, "Melee", &[STRIKE])
                            });
                        }
                    }
                }
                let delta = changes(&quantities(&cache(&before)), &quantities(&cache(&after)));
                assert_eq!(delta.len(), 1);
                assert_eq!((&*delta[0].unique_name, &*delta[0].item_name), (path, name));
                assert_eq!((delta[0].old_qty, delta[0].new_qty, delta[0].delta), (old, new, new - old));
            }
        }
    }

    #[test]
    fn ownership_classification_survives_cache_restart() {
        let mut blob = modular_blob();
        let weapon = "/Lotus/Weapons/Tenno/Rifle/Braton";
        let egg = "/Lotus/Types/Game/KubrowPet/Egg";
        let blueprint = "/Lotus/Weapons/ClanTech/BratonBlueprint";
        let part = "/Lotus/Weapons/Tenno/BratonBarrel";
        let suit = "/Lotus/Powersuits/Excalibur/Excalibur";
        let mod_path = "/Lotus/Upgrades/Mods/Rifle/Serration";
        let riven = "/Lotus/Upgrades/Mods/Randomized/ShotgunRiven";
        blob.unique_items.extend([unique(weapon, "LongGuns", &[]), unique(weapon, "LongGuns", &[]),
            unique(suit, "Suits", &[])]);
        blob.consumed_suits.push(suit.into());
        for path in [egg, blueprint, part] {
            blob.stackable_items.push(memory_scanner::BlobStackableEntry {
                item_type: path.into(), item_count: 2, sockets: None,
            });
        }
        blob.mods.insert(mod_path.into(), memory_scanner::ModCount {
            total: 3, by_rank: [(0, 2), (5, 1)].into(),
        });
        blob.rivens.push(serde_json::from_value(serde_json::json!({
            "item_id": "", "item_type": riven, "compat": null, "lvl_req": null,
            "polarity": null, "buffs": [], "curses": [], "mod_rank": 0, "count": 2
        })).expect("valid riven"));
        let mut cached = cache(&blob);
        // Legacy caches can predate is_stackable.
        cached.items.get_mut(COSMETIC).expect("cosmetic").is_stackable = false;
        for (path, category) in [(blueprint, "Blueprints"), (part, "Parts")] {
            let item = cached.items.get_mut(path).expect("stackable");
            item.is_stackable = false;
            item.category = category.into();
        }
        let uniques = cached.unique_quantities();
        assert_eq!(uniques, [(PRISM.into(), 2), (STRIKE.into(), 2), (weapon.into(), 1)].into());
        let stackables = cached.stackable_quantities();
        for path in [COSMETIC, egg, blueprint, part] { assert_eq!(stackables.get(path), Some(&2)); }
        assert_eq!(cached.items[mod_path].amount, 3);
        assert_eq!(cached.items[riven].amount, 2);
        assert!(cached.items[riven].mod_ranks.is_some());
        assert!(changes(&quantities(&cached), &quantities(&cache(&blob))).is_empty());
    }

    #[test]
    fn empty_or_reset_baseline_does_not_log_inventory_as_new() {
        let current = quantities(&cache(&modular_blob()));
        let mut baseline = quantities(&InventoryStateCache::default());
        assert!(changes(&baseline, &current).is_empty());
        baseline = current.clone();
        baseline.clear();
        assert!(changes(&baseline, &current).is_empty());
        let accepted_empty = quantities(&cache(&memory_scanner::BlobInventory::default()));
        assert_eq!(changes(&accepted_empty, &current).len(), 3);
        let previous = [("credits".into(), 1), (AMP.into(), 1)].into();
        let current = [("credits".into(), 1), (ZAW.into(), 1)].into();
        assert!(changes(&previous, &current).is_empty());
    }

    #[test]
    fn rejected_blob_preserves_persisted_cache_and_baseline() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tmp");
        std::fs::create_dir_all(&dir).expect("project tmp writable");
        let path = dir.join(format!("inventory-regression-{}.json", std::process::id()));
        let blob = modular_blob();
        let accepted = cache(&blob);
        assert!(persist_complete_inventory(&blob, &HashMap::new(), &accepted, &path));
        let before = std::fs::read(&path).expect("cache written");
        let previous = quantities(&load_inventory_state_cache(&path));
        let rejected = memory_scanner::BlobInventory::default();
        assert!(!persist_complete_inventory(&rejected, &accepted.unique_quantities(), &cache(&rejected), &path));
        assert_eq!(std::fs::read(&path).expect("cache retained"), before);
        assert!(changes(&previous, &quantities(&load_inventory_state_cache(&path))).is_empty());
        assert!(changes(&previous, &quantities(&cache(&blob))).is_empty());
        std::fs::remove_file(path).expect("test cache removable");
    }

    #[test]
    fn alias_only_retains_canonical_ownership() {
        let mut blob = memory_scanner::BlobInventory::default();
        blob.unique_items.push(unique(ALIAS, "Suits", &[]));
        assert_eq!(cache(&blob).items.get(CANONICAL).map(|v| v.amount), Some(1));
        blob.unique_items.push(unique(CANONICAL, "Suits", &[]));
        assert_eq!(cache(&blob).items.get(CANONICAL).map(|v| v.amount), Some(1));
        assert!(!cache(&blob).items.contains_key(ALIAS));
    }
}
