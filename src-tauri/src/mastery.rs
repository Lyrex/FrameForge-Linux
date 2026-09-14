use std::collections::{HashMap, HashSet};
use std::path::Path;
use tauri::State;
use crate::app_state::{AppState, CorrectionEntry};
use crate::catalogue::fix_category;
use crate::inventory_state::{inventory_path_aliases, load_inventory_state_cache};
use crate::mastery_progress::{PlayerProgress, Provenance, ProvenanceState};
use crate::mastery_rules::{self, Unobtainable};
use crate::monitor::CraftingJob;
use crate::settings::read_settings_map;
use crate::wfcd::{RecipeComponent, SyndicateOffer, WfcdItem};

const COLLECTION_CATEGORIES: [&str; 9] = [
    "Warframes", "Primary", "Secondary", "Melee", "Operator Weapons",
    "Archwing", "Companions", "Companion Weapons", "Vehicles",
];

#[derive(serde::Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MasteryState { Mastered, Partial, Missing, Unknown }

#[derive(serde::Serialize, Clone, Debug)]
pub(crate) struct MasterySource {
    pub(crate) unique_name: String,
    pub(crate) name: String,
    pub(crate) category: String,
    pub(crate) image_name: Option<String>,
    pub(crate) mastery_req: Option<u32>,
    pub(crate) cap: u32,
    pub(crate) earned_rank: Option<u32>,
    pub(crate) state: MasteryState,
    /// The table's class, regardless of settings.
    pub(crate) unobtainable: Option<Unobtainable>,
    /// Settings exclude the class: the source sits in the Unobtainable
    /// bucket, outside `total`, with its progress still shown.
    pub(crate) excluded: bool,
}

/// `unobtainable` counts excluded sources and is not part of `total`.
#[derive(serde::Serialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) struct MasteryCounts {
    pub(crate) total: u32,
    pub(crate) mastered: u32,
    pub(crate) partial: u32,
    pub(crate) missing: u32,
    pub(crate) unknown: u32,
    pub(crate) unobtainable: u32,
}

impl MasteryCounts {
    fn add(&mut self, source: &MasterySource) {
        if source.excluded {
            self.unobtainable += 1;
            return;
        }
        self.total += 1;
        match source.state {
            MasteryState::Mastered => self.mastered += 1,
            MasteryState::Partial => self.partial += 1,
            MasteryState::Missing => self.missing += 1,
            MasteryState::Unknown => self.unknown += 1,
        }
    }
}

#[derive(serde::Serialize, Clone, Debug)]
pub(crate) struct MasteryCategory {
    pub(crate) category: String,
    pub(crate) counts: MasteryCounts,
    pub(crate) sources: Vec<MasterySource>,
}

/// Intrinsics, nodes and junctions have no extraction yet and stay Unknown.
#[derive(serde::Serialize, Clone, Copy, Default, Debug)]
pub(crate) struct MasteryProvenance {
    pub(crate) equipment: Provenance,
    pub(crate) intrinsics: Provenance,
    pub(crate) nodes: Provenance,
    pub(crate) junctions: Provenance,
}

// TODO: add a craft stage, and relic routes under Acquire.
#[derive(serde::Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage { LevelClaim, Acquire }

#[derive(serde::Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Action { Level, Claim, Buy }

#[derive(serde::Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Access { Available, Blocked, Unknown }

#[derive(serde::Serialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct VendorOffer {
    pub(crate) syndicate: String,
    pub(crate) tier: String,
    pub(crate) blueprint: bool,
}

#[derive(serde::Serialize, Clone, Debug)]
pub(crate) struct Opportunity {
    #[serde(flatten)]
    pub(crate) source: MasterySource,
    pub(crate) stage: Stage,
    pub(crate) action: Action,
    pub(crate) remaining_mastery: Option<u32>,
    pub(crate) owned: bool,
    /// None on caches from before levels were stored.
    pub(crate) owned_level: Option<u32>,
    pub(crate) build_completion_ms: Option<i64>,
    pub(crate) vendors: Vec<VendorOffer>,
    pub(crate) access: Access,
    pub(crate) blockers: Vec<String>,
}

#[derive(serde::Serialize, Clone, Debug)]
pub(crate) struct MasteryOverview {
    pub(crate) counts: MasteryCounts,
    pub(crate) categories: Vec<MasteryCategory>,
    pub(crate) provenance: MasteryProvenance,
    pub(crate) opportunities: Vec<Opportunity>,
}

pub(crate) struct Observed<'a> {
    pub(crate) owned: &'a HashMap<String, i64>,
    pub(crate) owned_levels: &'a HashMap<String, Vec<u32>>,
    pub(crate) mastery_rank: Option<u32>,
    pub(crate) crafting: &'a [CraftingJob],
    pub(crate) recipes: &'a HashMap<String, Vec<RecipeComponent>>,
    pub(crate) offers: &'a HashMap<String, Vec<SyndicateOffer>>,
    pub(crate) now_ms: i64,
}

#[tauri::command]
pub(crate) fn get_mastery_overview(state: State<AppState>) -> MasteryOverview {
    let player = state.local_player_name.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let excluded = excluded_classes(&state.settings_path);
    let mut overview = {
        let progress = state.mastery_progress.lock().unwrap_or_else(|e| e.into_inner());
        let items = state.wfcd_items.lock().unwrap_or_else(|e| e.into_inner());
        build_mastery_overview(&items, &state.corrections, progress.current(player.as_deref()), &excluded)
    };
    let inventory = load_inventory_state_cache(&state.inventory_state_cache_path);
    // Without an inventory scan we have no idea what the player owns, so skip suggestions.
    if inventory.items.is_empty() { return overview; }
    let crafting = state.current_crafting.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let recipes = state.recipes.lock().unwrap_or_else(|e| e.into_inner());
    let offers = state.syndicate_catalog.lock().unwrap_or_else(|e| e.into_inner());
    overview.opportunities = suggest(&overview, &Observed {
        owned: &inventory.unique_quantities(),
        owned_levels: &inventory.owned_levels(),
        mastery_rank: inventory.mastery_rank,
        crafting: &crafting,
        recipes: &recipes,
        offers: &offers,
        now_ms: chrono::Utc::now().timestamp_millis(),
    });
    overview
}

/// A Foundry job carries the blueprint path. Each recipe lists its own
/// blueprint as the one component with an empty component list; part
/// blueprints like a chassis or barrel list their ingredients.
fn blueprint_results(recipes: &HashMap<String, Vec<RecipeComponent>>) -> HashMap<&str, &str> {
    recipes.iter().flat_map(|(result, components)| {
        components.iter()
            .filter(|c| c.unique_name.ends_with("Blueprint") && c.components.is_empty())
            .map(move |c| (c.unique_name.as_str(), result.as_str()))
    }).collect()
}

/// Hok and Rude Zuud offers have an empty result_unique, so they match on
/// the part path with "Blueprint" appended.
fn vendor_offers(source: &str, offers: &HashMap<String, Vec<SyndicateOffer>>) -> Vec<VendorOffer> {
    let blueprint = format!("{source}Blueprint");
    let blueprint = blueprint.as_str();
    let mut vendors: Vec<VendorOffer> = offers.iter().flat_map(|(syndicate, offers)| {
        offers.iter().filter_map(move |o| {
            let is_blueprint = if o.unique_name == source { false }
                else if o.result_unique.as_deref() == Some(source) || o.unique_name == blueprint { true }
                else { return None };
            Some(VendorOffer { syndicate: syndicate.clone(), tier: o.tier.clone(), blueprint: is_blueprint })
        })
    }).collect();
    vendors.sort_by(|a, b| a.syndicate.cmp(&b.syndicate).then_with(|| a.tier.cmp(&b.tier)));
    vendors
}

pub(crate) fn suggest(overview: &MasteryOverview, observed: &Observed) -> Vec<Opportunity> {
    let blueprint_results = blueprint_results(observed.recipes);
    let building: HashMap<&str, i64> = observed.crafting.iter()
        .map(|job| (blueprint_results.get(job.unique_name.as_str()).copied().unwrap_or(&job.unique_name), job.completion_ms))
        .collect();
    let mut opportunities: Vec<Opportunity> = overview.categories.iter().flat_map(|c| &c.sources)
        .filter(|s| !s.excluded && s.state != MasteryState::Mastered)
        .filter_map(|source| {
            let owned_level = observed.owned_levels.get(&source.unique_name).and_then(|levels| levels.iter().max().copied());
            let owned = owned_level.is_some() || observed.owned.get(&source.unique_name).is_some_and(|&copies| copies > 0);
            let build_completion_ms = building.get(source.unique_name.as_str()).copied();
            let vendors = vendor_offers(&source.unique_name, observed.offers);
            let action = if owned { Action::Level }
                else if build_completion_ms.is_some() { Action::Claim }
                else if !vendors.is_empty() { Action::Buy }
                else { return None };
            let stage = if action == Action::Buy { Stage::Acquire } else { Stage::LevelClaim };
            let remaining_mastery = source.earned_rank
                .map(|rank| source.cap.saturating_sub(rank) * mastery_rules::mastery_per_rank(&source.unique_name));
            let (access, blockers) = match action {
                Action::Level => (Access::Available, vec![]),
                Action::Claim if build_completion_ms.is_some_and(|done| done > observed.now_ms) => (Access::Blocked, vec!["Still building".into()]),
                Action::Claim => (Access::Available, vec![]),
                Action::Buy => {
                    // TODO: read standing from the scan. Until then every Buy stays Unknown.
                    let mut access = Access::Unknown;
                    let mut blockers = vec![];
                    match (source.mastery_req, observed.mastery_rank) {
                        (Some(required), Some(rank)) if required > rank => {
                            access = Access::Blocked;
                            blockers.push(format!("Requires MR {required}"));
                        }
                        (Some(required), None) if required > 0 => blockers.push("Mastery Rank not observed".into()),
                        _ => {}
                    }
                    blockers.push("Standing not observed".into());
                    (access, blockers)
                }
            };
            Some(Opportunity {
                source: source.clone(), stage, action, remaining_mastery, owned, owned_level, build_completion_ms, vendors, access, blockers,
            })
        })
        .collect();
    opportunities.sort_by(|a, b| a.stage.cmp(&b.stage)
        .then_with(|| match (a.remaining_mastery, b.remaining_mastery) {
            (Some(x), Some(y)) => y.cmp(&x),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        })
        .then_with(|| a.source.name.cmp(&b.source.name))
        .then_with(|| a.source.unique_name.cmp(&b.source.unique_name)));
    opportunities
}

/// The `masteryExclude` map in settings.json, one boolean per class; only an
/// explicit `false` lifts a class's exclusion.
pub(crate) fn excluded_classes(settings_path: &Path) -> HashSet<Unobtainable> {
    let mut excluded: HashSet<Unobtainable> = Unobtainable::ALL.into();
    let settings = read_settings_map(settings_path).unwrap_or_default();
    let lifted = settings.get("masteryExclude").and_then(|v| v.as_object()).into_iter().flatten()
        .filter(|(_, on)| on.as_bool() == Some(false))
        .filter_map(|(key, _)| serde_json::from_value::<Unobtainable>(key.as_str().into()).ok());
    for class in lifted { excluded.remove(&class); }
    excluded
}

pub(crate) fn build_mastery_overview(
    items: &[WfcdItem],
    corrections: &HashMap<String, CorrectionEntry>,
    progress: Option<&PlayerProgress>,
    excluded: &HashSet<Unobtainable>,
) -> MasteryOverview {
    let aliases = inventory_path_aliases();
    let equipment = progress.map(|p| p.equipment).unwrap_or_default();
    // XPInfo credits some aliases directly; the overview lists canonical entries only.
    let mut affinity: HashMap<&str, i64> = HashMap::new();
    for (path, &earned) in progress.iter().flat_map(|p| &p.affinity) {
        let canonical = aliases.get(path.as_str()).copied().unwrap_or(path);
        let credit = affinity.entry(canonical).or_insert(earned);
        *credit = (*credit).max(earned);
    }
    let mut sources: HashMap<String, MasterySource> = HashMap::new();

    // The Plexus has no WFCD entry. A stand-in carrying only the path is
    // enough: the loop reads name, category and cap from the table row.
    let catalogued: HashSet<&str> = items.iter().map(|i| i.unique_name.as_str()).collect();
    let table_only: Vec<WfcdItem> = corrections.values()
        .filter(|c| !catalogued.contains(c.path.as_str())
            && c.name.as_deref().is_some_and(|n| !n.is_empty())
            && mastery_rules::masterable(Some(c), None, &c.path) == Some(true))
        .map(|c| WfcdItem { unique_name: c.path.clone(), ..Default::default() })
        .collect();

    for i in items.iter().chain(&table_only) {
        if i.unique_name.contains("PvPVariant") { continue; }
        // Alias secondaries carry no credit of their own; the canonical entry does.
        if aliases.contains_key(i.unique_name.as_str()) { continue; }
        let correction = corrections.get(&i.unique_name);
        if correction.and_then(|c| c.category.as_deref()) == Some("Ignored") { continue; }
        if mastery_rules::masterable(correction, i.masterable, &i.unique_name) != Some(true) { continue; }
        let display_category = correction.and_then(|c| c.category.clone())
            .unwrap_or_else(|| fix_category(&i.name, &i.item_type, &i.product_category, &i.category, &i.unique_name));
        let Some(category) = collection_category(&i.item_type, &display_category) else { continue };
        let name = correction.and_then(|c| c.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| i.name.clone());
        sources.entry(i.unique_name.clone()).or_insert_with(|| MasterySource {
            unique_name: i.unique_name.clone(),
            name,
            category: category.into(),
            image_name: i.image_name.clone(),
            mastery_req: i.mastery_req,
            cap: mastery_rules::rank_cap(correction, &i.unique_name, i.max_level_cap),
            earned_rank: None,
            state: MasteryState::Unknown,
            unobtainable: correction.and_then(|c| c.unobtainable),
            excluded: correction.and_then(|c| c.unobtainable).is_some_and(|class| excluded.contains(&class)),
        });
    }

    let mut counts = MasteryCounts::default();
    let mut categories: Vec<MasteryCategory> = COLLECTION_CATEGORIES.iter()
        .map(|c| MasteryCategory { category: (*c).into(), counts: MasteryCounts::default(), sources: vec![] })
        .collect();
    for mut source in sources.into_values() {
        let rank = affinity.get(source.unique_name.as_str())
            .map(|&earned| mastery_rules::xp_to_rank(earned, &source.unique_name).min(source.cap));
        // An Unconfirmed record came from a cache that dropped rank-0 rows, so
        // absence there says nothing.
        let rank = match (equipment.state, rank) {
            (ProvenanceState::Unknown, _) | (ProvenanceState::Unconfirmed, None) => None,
            (ProvenanceState::Confirmed, None) => Some(0),
            (_, Some(rank)) => Some(rank),
        };
        if let Some(rank) = rank {
            source.earned_rank = Some(rank);
            source.state = if rank >= source.cap { MasteryState::Mastered }
                else if rank > 0 { MasteryState::Partial }
                else { MasteryState::Missing };
        }
        let category = categories.iter_mut().find(|c| c.category == source.category).expect("category comes from COLLECTION_CATEGORIES");
        category.counts.add(&source);
        counts.add(&source);
        category.sources.push(source);
    }
    categories.retain(|c| !c.sources.is_empty());
    for category in &mut categories {
        category.sources.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.unique_name.cmp(&b.unique_name)));
    }
    MasteryOverview { counts, categories, provenance: MasteryProvenance { equipment, ..Default::default() }, opportunities: vec![] }
}

/// Inventory display categories file modular chambers, decks and mechs under
/// Parts or Warframes and the Plexus under Railjack; the Collection files
/// them where the game's profile does.
fn collection_category(item_type: &str, display_category: &str) -> Option<&'static str> {
    match item_type {
        "Companion Weapon" => Some("Companion Weapons"),
        "Necramech" | "K-Drive Component" => Some("Vehicles"),
        "Kitgun Component" => Some("Secondary"),
        "Zaw Component" => Some("Melee"),
        "Amp" => Some("Operator Weapons"),
        _ if display_category == "Railjack" => Some("Vehicles"),
        _ => COLLECTION_CATEGORIES.iter().copied().find(|c| *c == display_category),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mastery_progress::{PlayerProgress, Provenance, ProvenanceState};
    use crate::wfcd::{RecipeComponent, SyndicateOffer};

    const ORION: &str = "/Lotus/Powersuits/SiriusOrion/OrionSuit";
    const SIRIUS: &str = "/Lotus/Powersuits/SiriusOrion/SiriusSuit";
    const BRATON: &str = "/Lotus/Weapons/Tenno/Rifle/Braton";
    const KUVA: &str = "/Lotus/Weapons/Grineer/KuvaLich/LongGuns/Karak/KuvaKarak";
    const PRISM: &str = "/Lotus/Weapons/Sentients/OperatorAmplifiers/Set1/Barrel/SentAmpSet1BarrelPartA";
    const STRIKE: &str = "/Lotus/Weapons/Ostron/Melee/ModularMelee01/Tip/TipOne";
    const CHAMBER: &str = "/Lotus/Weapons/SolarisUnited/Secondary/SUModularSecondarySet1/Barrel/SUModularSecondaryBarrelAPart";
    const DECK: &str = "/Lotus/Types/Vehicles/Hoverboard/HoverboardParts/PartComponents/HoverboardSolarisA/HoverboardSolarisADeck";
    const MECH: &str = "/Lotus/Powersuits/EntratiMech/NechroTech";
    const HOUND_HEAD: &str = "/Lotus/Types/Friendly/Pets/ZanukaPets/ZanukaPetParts/ZanukaPetPartHeadB";
    const SWEEPER: &str = "/Lotus/Types/Sentinels/SentinelWeapons/SentShotgun";
    const IMPERATOR: &str = "/Lotus/Weapons/Tenno/Archwing/Primary/ArchwingRifle/ArchwingRifle";
    const SICKLE: &str = "/Lotus/Weapons/Lasria/LasGooSickle/LasGooSicklePlayerWeapon";
    const VINQUIBUS_MELEE: &str = "/Lotus/Weapons/Tenno/Bayonet/TnBayonetMeleeWeapon";
    const MOTE_PRISM: &str = "/Lotus/Weapons/Sentients/OperatorAmplifiers/SentTrainingAmplifier/SentAmpTrainingBarrel";
    const SPORELACER: &str = "/Lotus/Weapons/Infested/Pistols/InfKitGun/Barrels/InfBarrelEgg/InfModularBarrelEggPart";
    const GRIMOIRE: &str = "/Lotus/Weapons/Tenno/Grimoire/TnGrimoire";
    const GRIMOIRE_ALIAS: &str = "/Lotus/Weapons/Tenno/Grimoire/TnDoppelgangerGrimoire";
    const ZAW_WEAPON: &str = "/Lotus/Weapons/Ostron/Melee/LotusModularWeapon";
    const EXCALIBUR_PRIME: &str = "/Lotus/Powersuits/Excalibur/ExcaliburPrime";
    const EXCALIBUR_UMBRA: &str = "/Lotus/Powersuits/Excalibur/ExcaliburUmbra";
    const SNIPETRON: &str = "/Lotus/Weapons/Tenno/Rifle/SniperRifle";
    const VENARI: &str = "/Lotus/Powersuits/Khora/Kavat/KhoraKavatPowerSuit";
    const PLEXUS: &str = "/Lotus/Types/Game/CrewShip/RailJack/DefaultHarness";

    fn item(name: &str, path: &str, item_type: &str, product_category: &str, category: &str, masterable: Option<bool>) -> WfcdItem {
        WfcdItem {
            name: name.into(), unique_name: path.into(), category: category.into(),
            item_type: item_type.into(), product_category: product_category.into(),
            image_name: Some(format!("{}.png", name.to_lowercase())), vaulted: None, ducats: None,
            mastery_req: Some(2), omega_attenuation: None, fusion_limit: None,
            max_level_cap: None, tradable: None, masterable,
        }
    }

    fn correction(path: &str, name: Option<&str>, category: &str) -> (String, CorrectionEntry) {
        (path.into(), CorrectionEntry {
            path: path.into(), name: name.map(Into::into), category: Some(category.into()), ..Default::default()
        })
    }

    fn catalog() -> Vec<WfcdItem> {
        let mut kuva = item("Kuva Karak", KUVA, "Rifle", "LongGuns", "Primary", Some(true));
        kuva.max_level_cap = Some(40);
        vec![
            item("Sirius & Orion", ORION, "Warframe", "SpecialItems", "Warframes", Some(true)),
            item("Sirius & Orion", SIRIUS, "Warframe", "Suits", "Warframes", Some(true)),
            item("Braton", BRATON, "Rifle", "LongGuns", "Primary", Some(true)),
            item("Braton", "/Lotus/Weapons/Tenno/Rifle/BratonPvPVariant", "Rifle", "LongGuns", "Primary", Some(true)),
            kuva,
            item("Raplak Prism", PRISM, "Amp", "Pistols", "Resources", Some(false)),
            item("Mote Prism", MOTE_PRISM, "Amp", "Pistols", "Resources", Some(false)),
            item("Sporelacer", SPORELACER, "Pistol", "Pistols", "Secondary", Some(false)),
            item("Grimoire", GRIMOIRE, "Pistol", "Pistols", "Secondary", Some(true)),
            item("Grimoire", GRIMOIRE_ALIAS, "Pistol", "Pistols", "Secondary", Some(true)),
            item("Balla", STRIKE, "Zaw Component", "Pistols", "Melee", Some(true)),
            item("Zaw", ZAW_WEAPON, "Melee", "Melee", "Melee", Some(true)),
            item("Catchmoon", CHAMBER, "Kitgun Component", "Pistols", "Resources", Some(true)),
            item("Bad Baby", DECK, "K-Drive Component", "Pistols", "Resources", Some(true)),
            item("Voidrig", MECH, "Necramech", "MechSuits", "Warframes", Some(true)),
            item("Bhaira Hound", HOUND_HEAD, "Pets", "Pistols", "Companions", Some(true)),
            item("Venari", VENARI, "Warframe", "SpecialItems", "Companions", Some(false)),
            item("Sweeper", SWEEPER, "Companion Weapon", "SentinelWeapons", "Primary", Some(true)),
            item("Imperator", IMPERATOR, "Arch-Gun", "SpaceGuns", "Archwing", Some(true)),
            item("Dual Viciss", SICKLE, "Rifle", "Melee", "Melee", Some(true)),
            item("Serration", "/Lotus/Upgrades/Mods/Rifle/Serration", "Primary Mod", "", "Mods", Some(false)),
            item("Forma", "/Lotus/Types/Items/MiscItems/Forma", "Resource", "", "Resources", None),
        ]
    }

    fn corrections() -> HashMap<String, CorrectionEntry> {
        [
            correction(ZAW_WEAPON, None, "Ignored"),
            correction(STRIKE, Some("Balla"), "Melee"),
            correction(PRISM, Some("Raplak Prism"), "Operator Weapons"),
            correction(VINQUIBUS_MELEE, Some("Vinquibus"), "Melee"),
        ].into()
    }

    fn observed(state: ProvenanceState, observed_at: Option<i64>, affinity: &[(&str, i64)]) -> PlayerProgress {
        PlayerProgress {
            affinity: affinity.iter().map(|(path, earned)| ((*path).to_string(), *earned)).collect(),
            equipment: Provenance { state, observed_at },
        }
    }

    fn source<'a>(overview: &'a MasteryOverview, path: &str) -> &'a MasterySource {
        overview.categories.iter().flat_map(|c| &c.sources)
            .find(|s| s.unique_name == path)
            .unwrap_or_else(|| panic!("{path} listed"))
    }

    fn names(overview: &MasteryOverview, category: &str) -> Vec<String> {
        overview.categories.iter().find(|c| c.category == category)
            .map(|c| c.sources.iter().map(|s| s.name.clone()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn every_masterable_category_is_listed_once_by_canonical_identity() {
        let overview = build_mastery_overview(&catalog(), &corrections(), Some(&observed(ProvenanceState::Confirmed, Some(1_000), &[])), &HashSet::new());
        let categories: Vec<&str> = overview.categories.iter().map(|c| c.category.as_str()).collect();
        assert_eq!(categories, ["Warframes", "Primary", "Secondary", "Melee", "Operator Weapons",
            "Archwing", "Companions", "Companion Weapons", "Vehicles"]);
        assert_eq!(names(&overview, "Warframes"), ["Sirius & Orion"]);
        assert_eq!(source(&overview, SIRIUS).category, "Warframes");
        assert_eq!(names(&overview, "Primary"), ["Braton", "Kuva Karak"]);
        assert_eq!(names(&overview, "Secondary"), ["Catchmoon", "Grimoire", "Sporelacer"]);
        assert_eq!(names(&overview, "Melee"), ["Balla", "Dual Viciss"]);
        assert_eq!(names(&overview, "Operator Weapons"), ["Mote Prism", "Raplak Prism"]);
        assert_eq!(names(&overview, "Archwing"), ["Imperator"]);
        assert_eq!(names(&overview, "Companions"), ["Bhaira Hound", "Venari"]);
        assert_eq!(names(&overview, "Companion Weapons"), ["Sweeper"]);
        assert_eq!(names(&overview, "Vehicles"), ["Bad Baby", "Voidrig"]);
        assert_eq!(overview.counts.total, 16);
        let all: Vec<&str> = overview.categories.iter().flat_map(|c| &c.sources).map(|s| s.unique_name.as_str()).collect();
        assert_eq!(all.len(), all.iter().collect::<std::collections::HashSet<_>>().len());
        for absent in [ORION, GRIMOIRE_ALIAS, ZAW_WEAPON, VINQUIBUS_MELEE] {
            assert!(!all.contains(&absent), "{absent} listed");
        }
        assert_eq!(source(&overview, BRATON).image_name.as_deref(), Some("braton.png"));
    }

    #[test]
    fn buckets_follow_catalog_caps_and_alias_credit_lands_on_canonical() {
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[
            (ORION, 900_000), (BRATON, 450_000), (KUVA, 612_500), (STRIKE, 72_000),
        ]);
        let overview = build_mastery_overview(&catalog(), &corrections(), Some(&progress), &HashSet::new());
        assert_eq!(overview.provenance.equipment, Provenance { state: ProvenanceState::Confirmed, observed_at: Some(1_000) });
        assert_eq!(overview.provenance.intrinsics.state, ProvenanceState::Unknown);
        assert_eq!((source(&overview, SIRIUS).earned_rank, source(&overview, SIRIUS).state), (Some(30), MasteryState::Mastered));
        assert_eq!(source(&overview, BRATON).state, MasteryState::Mastered);
        assert_eq!((source(&overview, KUVA).cap, source(&overview, KUVA).state), (40, MasteryState::Partial));
        assert_eq!(source(&overview, STRIKE).state, MasteryState::Partial);
        assert_eq!((source(&overview, CHAMBER).earned_rank, source(&overview, CHAMBER).state), (Some(0), MasteryState::Missing));
        let primary = overview.categories.iter().find(|c| c.category == "Primary").expect("primary");
        assert_eq!(primary.counts, MasteryCounts { total: 2, mastered: 1, partial: 1, missing: 0, unknown: 0, unobtainable: 0 });
        assert_eq!(overview.counts, MasteryCounts { total: 16, mastered: 2, partial: 2, missing: 12, unknown: 0, unobtainable: 0 });
    }

    #[test]
    fn no_observation_means_unknown_not_missing() {
        let overview = build_mastery_overview(&catalog(), &corrections(), None, &HashSet::new());
        assert!(overview.categories.iter().flat_map(|c| &c.sources)
            .all(|s| s.state == MasteryState::Unknown && s.earned_rank.is_none()));
        assert_eq!(overview.counts, MasteryCounts { total: 16, mastered: 0, partial: 0, missing: 0, unknown: 16, unobtainable: 0 });
        for kind in [overview.provenance.equipment, overview.provenance.intrinsics, overview.provenance.nodes, overview.provenance.junctions] {
            assert_eq!(kind, Provenance::default());
        }
    }

    #[test]
    fn unconfirmed_progress_shows_saved_ranks_and_leaves_absent_entries_unknown() {
        let progress = observed(ProvenanceState::Unconfirmed, None, &[(BRATON, 450_000), (KUVA, 612_500)]);
        let overview = build_mastery_overview(&catalog(), &corrections(), Some(&progress), &HashSet::new());
        assert_eq!(overview.provenance.equipment, Provenance { state: ProvenanceState::Unconfirmed, observed_at: None });
        assert_eq!((source(&overview, BRATON).earned_rank, source(&overview, BRATON).state), (Some(30), MasteryState::Mastered));
        assert_eq!((source(&overview, KUVA).earned_rank, source(&overview, KUVA).state), (Some(35), MasteryState::Partial));
        assert_eq!((source(&overview, CHAMBER).earned_rank, source(&overview, CHAMBER).state), (None, MasteryState::Unknown));
        assert_eq!(overview.counts, MasteryCounts { total: 16, mastered: 1, partial: 1, missing: 0, unknown: 14, unobtainable: 0 });
    }

    #[test]
    fn settings_exclude_every_class_until_they_say_otherwise() {
        let dir = std::env::temp_dir().join("frameforge-mastery-tests");
        std::fs::create_dir_all(&dir).expect("temp dir is always writable");
        let path = dir.join(format!("settings-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert_eq!(excluded_classes(&path), Unobtainable::ALL.into());
        std::fs::write(&path, r#"{"tracked":[],"masteryExclude":{"founders":false,"removedNode":true}}"#).expect("scratch file is writable");
        assert_eq!(excluded_classes(&path), [Unobtainable::RetiredEvent, Unobtainable::RemovedNode].into());
        std::fs::write(&path, r#"{"masteryExclude":{"founders":false,"retiredEvent":false,"removedNode":false}}"#).expect("scratch file is writable");
        assert!(excluded_classes(&path).is_empty());
    }

    #[test]
    fn a_table_entry_decides_eligibility_and_cap_over_the_catalogue() {
        let mut items = catalog();
        items.push(item("Excalibur Umbra", EXCALIBUR_UMBRA, "Warframe", "Suits", "Warframes", Some(false)));
        let mut corrections = corrections();
        corrections.insert(EXCALIBUR_UMBRA.into(), CorrectionEntry { path: EXCALIBUR_UMBRA.into(), masterable: Some(true), ..Default::default() });
        corrections.insert(CHAMBER.into(), CorrectionEntry { path: CHAMBER.into(), masterable: Some(false), ..Default::default() });
        corrections.insert(SICKLE.into(), CorrectionEntry { path: SICKLE.into(), rank_cap: Some(40), ..Default::default() });
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[(SICKLE, 450_000)]);
        let overview = build_mastery_overview(&items, &corrections, Some(&progress), &HashSet::new());
        assert_eq!(names(&overview, "Warframes"), ["Excalibur Umbra", "Sirius & Orion"]);
        assert_eq!(names(&overview, "Secondary"), ["Grimoire", "Sporelacer"]);
        assert_eq!((source(&overview, SICKLE).cap, source(&overview, SICKLE).earned_rank, source(&overview, SICKLE).state), (40, Some(30), MasteryState::Partial));
    }

    /// WFCD ships no Plexus entry while XPInfo credits it.
    #[test]
    fn a_table_row_lists_a_source_the_catalogue_lacks() {
        let mut corrections = corrections();
        corrections.insert(PLEXUS.into(), CorrectionEntry {
            path: PLEXUS.into(), name: Some("Plexus".into()), category: Some("Railjack".into()),
            masterable: Some(true), ..Default::default()
        });
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[(PLEXUS, 640_341)]);
        let overview = build_mastery_overview(&catalog(), &corrections, Some(&progress), &HashSet::new());
        assert_eq!(names(&overview, "Vehicles"), ["Bad Baby", "Plexus", "Voidrig"]);
        let plexus = source(&overview, PLEXUS);
        assert_eq!((plexus.cap, plexus.earned_rank, plexus.state), (30, Some(25), MasteryState::Partial));
        assert_eq!((plexus.image_name.as_deref(), plexus.mastery_req), (None, None));
        assert_eq!(overview.counts.total, 17);

        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[]);
        let overview = build_mastery_overview(&catalog(), &corrections, Some(&progress), &HashSet::new());
        assert_eq!((source(&overview, PLEXUS).earned_rank, source(&overview, PLEXUS).state), (Some(0), MasteryState::Missing));
    }

    #[test]
    fn each_excluded_class_leaves_the_denominator_and_returns_when_its_toggle_is_off() {
        let mut items = catalog();
        items.push(item("Excalibur Prime", EXCALIBUR_PRIME, "Warframe", "Suits", "Warframes", Some(true)));
        items.push(item("Snipetron", SNIPETRON, "Rifle", "LongGuns", "Primary", Some(true)));
        let marked = [
            (EXCALIBUR_PRIME, Unobtainable::Founders),
            (SNIPETRON, Unobtainable::RetiredEvent),
            (IMPERATOR, Unobtainable::RemovedNode),
        ];
        let mut corrections = corrections();
        for (path, class) in marked {
            corrections.insert(path.into(), CorrectionEntry { path: path.into(), unobtainable: Some(class), ..Default::default() });
        }
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[(EXCALIBUR_PRIME, 900_000)]);
        let all: HashSet<Unobtainable> = Unobtainable::ALL.into();

        let overview = build_mastery_overview(&items, &corrections, Some(&progress), &all);
        assert_eq!(overview.counts, MasteryCounts { total: 15, mastered: 0, partial: 0, missing: 15, unknown: 0, unobtainable: 3 });
        let excalibur = source(&overview, EXCALIBUR_PRIME);
        assert_eq!((excalibur.excluded, excalibur.unobtainable), (true, Some(Unobtainable::Founders)));
        assert_eq!((excalibur.state, excalibur.earned_rank), (MasteryState::Mastered, Some(30)));
        let warframes = overview.categories.iter().find(|c| c.category == "Warframes").expect("warframes");
        assert_eq!(warframes.counts, MasteryCounts { total: 1, mastered: 0, partial: 0, missing: 1, unknown: 0, unobtainable: 1 });

        for (path, class) in marked {
            let mut excluded = all.clone();
            excluded.remove(&class);
            let overview = build_mastery_overview(&items, &corrections, Some(&progress), &excluded);
            assert_eq!((source(&overview, path).excluded, source(&overview, path).unobtainable), (false, Some(class)));
            assert_eq!((overview.counts.total, overview.counts.unobtainable), (16, 2), "{class:?}");
        }

        let overview = build_mastery_overview(&items, &corrections, Some(&progress), &HashSet::new());
        assert_eq!(overview.counts, MasteryCounts { total: 18, mastered: 1, partial: 0, missing: 17, unknown: 0, unobtainable: 0 });
    }

    fn with_suggestions(items: &[WfcdItem], corrections: &HashMap<String, CorrectionEntry>, progress: Option<&PlayerProgress>, excluded: &HashSet<Unobtainable>, observed: &Observed) -> MasteryOverview {
        let mut overview = build_mastery_overview(items, corrections, progress, excluded);
        overview.opportunities = suggest(&overview, observed);
        overview
    }

    fn observed_gear<'a>(owned: &'a HashMap<String, i64>, owned_levels: &'a HashMap<String, Vec<u32>>, crafting: &'a [CraftingJob], recipes: &'a HashMap<String, Vec<RecipeComponent>>, offers: &'a HashMap<String, Vec<SyndicateOffer>>, mastery_rank: Option<u32>) -> Observed<'a> {
        Observed { owned, owned_levels, mastery_rank, crafting, recipes, offers, now_ms: 1_000_000 }
    }

    fn recipe(source: &str, blueprint: &str) -> (String, Vec<RecipeComponent>) {
        let component = |unique_name: &str, name: &str, components: Vec<RecipeComponent>| RecipeComponent {
            unique_name: unique_name.into(), name: name.into(), count: 1, result_count: 1, components,
        };
        let part = component("/Lotus/Types/Recipes/Parts/ChassisBlueprint", "Chassis Blueprint", vec![component("/Lotus/Types/Items/MiscItems/Ferrite", "Ferrite", vec![])]);
        (source.into(), vec![component("/Lotus/Types/Items/MiscItems/Ferrite", "Ferrite", vec![]), part, component(blueprint, "Blueprint", vec![])])
    }

    fn offer(unique_name: &str, tier: &str, result_unique: Option<&str>) -> SyndicateOffer {
        SyndicateOffer {
            unique_name: unique_name.into(), name: String::new(), category: String::new(), image_name: None,
            tier: tier.into(), ducats: None, result_unique: result_unique.map(Into::into),
        }
    }

    fn job(blueprint: &str, completion_ms: i64) -> CraftingJob {
        CraftingJob { unique_name: blueprint.into(), item_name: String::new(), completion_ms }
    }

    fn summary(opportunities: &[Opportunity]) -> Vec<(&str, Action, Option<u32>, Access)> {
        opportunities.iter().map(|o| (o.source.name.as_str(), o.action, o.remaining_mastery, o.access)).collect()
    }

    #[test]
    fn stages_run_level_claim_acquire_with_known_remaining_first_then_name() {
        let mut items = catalog();
        items.push(item("Excalibur Prime", EXCALIBUR_PRIME, "Warframe", "Suits", "Warframes", Some(true)));
        let mut corrections = corrections();
        corrections.insert(EXCALIBUR_PRIME.into(), CorrectionEntry { path: EXCALIBUR_PRIME.into(), unobtainable: Some(Unobtainable::Founders), ..Default::default() });
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[
            (BRATON, 72_000), (GRIMOIRE, 450_000), (MOTE_PRISM, 450_000),
        ]);
        let levels: HashMap<String, Vec<u32>> = [
            (BRATON.to_string(), vec![5, 12]), (GRIMOIRE.to_string(), vec![30]), (EXCALIBUR_PRIME.to_string(), vec![30]),
        ].into();
        let owned: HashMap<String, i64> = [(SIRIUS.to_string(), 1), (BRATON.to_string(), 1)].into();
        let crafting = [job("/Lotus/Types/Recipes/Weapons/KuvaKarakBlueprint", 999_000), job("/Lotus/Types/Recipes/Mechs/VoidrigBlueprint", 1_001_000)];
        let recipes: HashMap<String, Vec<RecipeComponent>> = [
            recipe(KUVA, "/Lotus/Types/Recipes/Weapons/KuvaKarakBlueprint"),
            recipe(MECH, "/Lotus/Types/Recipes/Mechs/VoidrigBlueprint"),
        ].into();
        let offers: HashMap<String, Vec<SyndicateOffer>> = [
            ("Steel Meridian".to_string(), vec![offer(SWEEPER, "General", None), offer("syndicate/stub/thing", "Maxim", None)]),
            ("Solaris United".to_string(), vec![offer(&format!("{CHAMBER}Blueprint"), "(Rude Zuud), Neutral", None)]),
            ("Cephalon Simaris".to_string(), vec![offer("/Lotus/Types/Recipes/Weapons/SweeperBlueprint", "Neutral", Some(SWEEPER))]),
        ].into();
        let overview = with_suggestions(&items, &corrections, Some(&progress), &Unobtainable::ALL.into(),
            &observed_gear(&owned, &levels, &crafting, &recipes, &offers, Some(2)));

        assert_eq!(summary(&overview.opportunities), [
            ("Voidrig", Action::Claim, Some(8_000), Access::Blocked),
            ("Sirius & Orion", Action::Level, Some(6_000), Access::Available),
            ("Kuva Karak", Action::Claim, Some(4_000), Access::Available),
            ("Braton", Action::Level, Some(1_800), Access::Available),
            ("Catchmoon", Action::Buy, Some(3_000), Access::Unknown),
            ("Sweeper", Action::Buy, Some(3_000), Access::Unknown),
        ]);
        let stages: Vec<Stage> = overview.opportunities.iter().map(|o| o.stage).collect();
        assert_eq!(stages, [Stage::LevelClaim, Stage::LevelClaim, Stage::LevelClaim, Stage::LevelClaim, Stage::Acquire, Stage::Acquire]);
        let braton = &overview.opportunities[3];
        assert_eq!((braton.owned, braton.owned_level, braton.source.earned_rank, braton.build_completion_ms), (true, Some(12), Some(12), None));
        assert_eq!((overview.opportunities[1].owned, overview.opportunities[1].owned_level), (true, None));
        let voidrig = &overview.opportunities[0];
        assert_eq!(voidrig.build_completion_ms, Some(1_001_000));
        assert_eq!(voidrig.blockers, ["Still building"]);
        assert!(overview.opportunities[2].blockers.is_empty());
        let sweeper = &overview.opportunities[5];
        assert_eq!(sweeper.vendors, [
            VendorOffer { syndicate: "Cephalon Simaris".into(), tier: "Neutral".into(), blueprint: true },
            VendorOffer { syndicate: "Steel Meridian".into(), tier: "General".into(), blueprint: false },
        ]);
        assert_eq!(sweeper.blockers, ["Standing not observed"]);
        assert_eq!(overview.opportunities[4].vendors, [VendorOffer { syndicate: "Solaris United".into(), tier: "(Rude Zuud), Neutral".into(), blueprint: true }]);
    }

    #[test]
    fn a_mastery_lock_blocks_acquisition_and_an_unknown_mastery_rank_leaves_access_unknown() {
        let owned = HashMap::new();
        let levels = HashMap::new();
        let recipes = HashMap::new();
        let offers: HashMap<String, Vec<SyndicateOffer>> = [("Steel Meridian".to_string(), vec![offer(SWEEPER, "General", None)])].into();
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[]);
        let locked = with_suggestions(&catalog(), &corrections(), Some(&progress), &HashSet::new(),
            &observed_gear(&owned, &levels, &[], &recipes, &offers, Some(1)));
        assert_eq!(summary(&locked.opportunities), [("Sweeper", Action::Buy, Some(3_000), Access::Blocked)]);
        assert_eq!(locked.opportunities[0].blockers, ["Requires MR 2", "Standing not observed"]);
        let unranked = with_suggestions(&catalog(), &corrections(), Some(&progress), &HashSet::new(),
            &observed_gear(&owned, &levels, &[], &recipes, &offers, None));
        assert_eq!(summary(&unranked.opportunities), [("Sweeper", Action::Buy, Some(3_000), Access::Unknown)]);
        assert_eq!(unranked.opportunities[0].blockers, ["Mastery Rank not observed", "Standing not observed"]);
    }

    #[test]
    fn unknown_remaining_stays_unknown_and_sorts_after_known_within_its_stage() {
        let owned = HashMap::new();
        let levels: HashMap<String, Vec<u32>> = [(BRATON.to_string(), vec![12]), (SIRIUS.to_string(), vec![3]), (KUVA.to_string(), vec![35])].into();
        let recipes = HashMap::new();
        let offers = HashMap::new();
        let unobserved = with_suggestions(&catalog(), &corrections(), None, &HashSet::new(),
            &observed_gear(&owned, &levels, &[], &recipes, &offers, None));
        assert_eq!(summary(&unobserved.opportunities), [
            ("Braton", Action::Level, None, Access::Available),
            ("Kuva Karak", Action::Level, None, Access::Available),
            ("Sirius & Orion", Action::Level, None, Access::Available),
        ]);
        let progress = observed(ProvenanceState::Unconfirmed, None, &[(BRATON, 72_000), (KUVA, 612_500)]);
        let unconfirmed = with_suggestions(&catalog(), &corrections(), Some(&progress), &HashSet::new(),
            &observed_gear(&owned, &levels, &[], &recipes, &offers, None));
        assert_eq!(summary(&unconfirmed.opportunities), [
            ("Braton", Action::Level, Some(1_800), Access::Available),
            ("Kuva Karak", Action::Level, Some(500), Access::Available),
            ("Sirius & Orion", Action::Level, None, Access::Available),
        ]);
    }

    #[test]
    fn one_result_per_source_keeps_the_other_routes_as_details() {
        let owned = HashMap::new();
        let levels: HashMap<String, Vec<u32>> = [(KUVA.to_string(), vec![10])].into();
        let crafting = [job("/Lotus/Types/Recipes/Weapons/KuvaKarakBlueprint", 0)];
        let recipes: HashMap<String, Vec<RecipeComponent>> = [recipe(KUVA, "/Lotus/Types/Recipes/Weapons/KuvaKarakBlueprint")].into();
        let offers: HashMap<String, Vec<SyndicateOffer>> = [("Kahl's Garrison".to_string(), vec![offer(KUVA, "Champion", None)])].into();
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[(KUVA, 50_000)]);
        let overview = with_suggestions(&catalog(), &corrections(), Some(&progress), &HashSet::new(),
            &observed_gear(&owned, &levels, &crafting, &recipes, &offers, Some(30)));
        assert_eq!(summary(&overview.opportunities), [("Kuva Karak", Action::Level, Some(3_000), Access::Available)]);
        let kuva = &overview.opportunities[0];
        assert_eq!((kuva.owned_level, kuva.build_completion_ms, kuva.vendors.len()), (Some(10), Some(0), 1));
    }
}
