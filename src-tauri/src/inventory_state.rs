use std::collections::HashMap;
use std::path::PathBuf;
use crate::memory_scanner;

/// Companion API mod copy entry — camelCase so it round-trips through TypeScript without conversion.
#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ApiModCopy {
    pub(crate) unique_name: String,
    pub(crate) rank: Option<u32>,
    pub(crate) count: i64,
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


/// Build a fresh `InventoryStateCache` from a parsed FULL_ACCOUNT blob.
/// All sections are authoritative — this fully replaces scanner-derived data.
pub(crate) fn build_inventory_from_blob(
    blob: &memory_scanner::BlobInventory,
    path_to_name: &HashMap<String, String>,
    path_to_category: &HashMap<String, String>,
    path_to_ducat: &HashMap<String, u32>,
    path_to_vaulted: &HashMap<String, bool>,
    relic_drops: &HashMap<String, Vec<String>>,
    existing_wfm_prices: &HashMap<String, u32>,
    excluded_paths: &std::collections::HashSet<String>,
) -> InventoryStateCache {
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

    // Unique items — binary owned (amount = 1).
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

        if excluded_paths.contains(&entry.item_type) { continue; }

        let item = upsert!(&entry.item_type);
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

    // Mastery rank per item from XPInfo.
    for (path, &rank) in &blob.mastery_data {
        if rank > 0 { upsert!(path).mastery_rank = rank; }
    }

    // Catalog-derived fields + carry forward fetched WFM prices.
    for (path, item) in items.iter_mut() {
        item.ducat_price  = path_to_ducat.get(path).copied();
        item.vaulted      = path_to_vaulted.get(path).copied();
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
