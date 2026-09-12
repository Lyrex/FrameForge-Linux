use std::collections::HashMap;
use tauri::State;
use crate::app_state::{AppState, CorrectionEntry};
use crate::catalogue::fix_category;
use crate::inventory_state::{inventory_path_aliases, load_inventory_state_cache};
use crate::mastery_rules;
use crate::wfcd::WfcdItem;

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
}

#[derive(serde::Serialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) struct MasteryCounts {
    pub(crate) total: u32,
    pub(crate) mastered: u32,
    pub(crate) partial: u32,
    pub(crate) missing: u32,
    pub(crate) unknown: u32,
}

impl MasteryCounts {
    fn add(&mut self, state: MasteryState) {
        self.total += 1;
        match state {
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

#[derive(serde::Serialize, Clone, Debug)]
pub(crate) struct MasteryOverview {
    pub(crate) counts: MasteryCounts,
    pub(crate) categories: Vec<MasteryCategory>,
}

#[tauri::command]
pub(crate) fn get_mastery_overview(state: State<AppState>) -> MasteryOverview {
    let cache = load_inventory_state_cache(&state.inventory_state_cache_path);
    let earned = cache.has_account_observation().then(|| cache.mastery_data());
    let items = state.wfcd_items.lock().unwrap_or_else(|e| e.into_inner());
    build_mastery_overview(&items, &state.corrections, earned.as_ref())
}

/// `earned` is None until a full account observation exists: every source is
/// then Unknown rather than Missing, since unknown is never zero.
pub(crate) fn build_mastery_overview(
    items: &[WfcdItem],
    corrections: &HashMap<String, CorrectionEntry>,
    earned: Option<&HashMap<String, u32>>,
) -> MasteryOverview {
    let aliases = inventory_path_aliases();
    let mut sources: HashMap<String, MasterySource> = HashMap::new();

    for i in items {
        if i.unique_name.contains("PvPVariant") { continue; }
        // Alias secondaries carry no credit of their own; the canonical entry does.
        if aliases.contains_key(i.unique_name.as_str()) { continue; }
        let correction = corrections.get(&i.unique_name);
        if correction.and_then(|c| c.category.as_deref()) == Some("Ignored") { continue; }
        if mastery_rules::masterable(i.masterable, &i.unique_name) != Some(true) { continue; }
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
            cap: mastery_rules::rank_cap(&i.unique_name, i.max_level_cap),
            earned_rank: None,
            state: MasteryState::Unknown,
        });
    }

    let mut counts = MasteryCounts::default();
    let mut categories: Vec<MasteryCategory> = COLLECTION_CATEGORIES.iter()
        .map(|c| MasteryCategory { category: (*c).into(), counts: MasteryCounts::default(), sources: vec![] })
        .collect();
    for mut source in sources.into_values() {
        if let Some(earned) = earned {
            let rank = earned.get(&source.unique_name).copied().unwrap_or(0);
            source.earned_rank = Some(rank);
            source.state = if rank >= source.cap { MasteryState::Mastered }
                else if rank > 0 { MasteryState::Partial }
                else { MasteryState::Missing };
        }
        let category = categories.iter_mut().find(|c| c.category == source.category).expect("category comes from COLLECTION_CATEGORIES");
        category.counts.add(source.state);
        counts.add(source.state);
        category.sources.push(source);
    }
    categories.retain(|c| !c.sources.is_empty());
    for category in &mut categories {
        category.sources.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.unique_name.cmp(&b.unique_name)));
    }
    MasteryOverview { counts, categories }
}

/// Inventory display categories file modular chambers, decks and mechs under
/// Parts or Warframes; the Collection files them where the game's profile does.
fn collection_category(item_type: &str, display_category: &str) -> Option<&'static str> {
    match item_type {
        "Companion Weapon" => Some("Companion Weapons"),
        "Necramech" | "K-Drive Component" => Some("Vehicles"),
        "Kitgun Component" => Some("Secondary"),
        "Zaw Component" => Some("Melee"),
        "Amp" => Some("Operator Weapons"),
        _ => COLLECTION_CATEGORIES.iter().copied().find(|c| *c == display_category),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            path: path.into(), name: name.map(Into::into), category: Some(category.into()),
            tradeable_wfm: None, is_stackable: None,
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
        let overview = build_mastery_overview(&catalog(), &corrections(), Some(&HashMap::new()));
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
        assert_eq!(names(&overview, "Companions"), ["Bhaira Hound"]);
        assert_eq!(names(&overview, "Companion Weapons"), ["Sweeper"]);
        assert_eq!(names(&overview, "Vehicles"), ["Bad Baby", "Voidrig"]);
        assert_eq!(overview.counts.total, 15);
        let all: Vec<&str> = overview.categories.iter().flat_map(|c| &c.sources).map(|s| s.unique_name.as_str()).collect();
        assert_eq!(all.len(), all.iter().collect::<std::collections::HashSet<_>>().len());
        for absent in [ORION, GRIMOIRE_ALIAS, ZAW_WEAPON, VINQUIBUS_MELEE] {
            assert!(!all.contains(&absent), "{absent} listed");
        }
        assert_eq!(source(&overview, BRATON).image_name.as_deref(), Some("braton.png"));
    }

    #[test]
    fn buckets_follow_catalog_caps_and_alias_credit_lands_on_canonical() {
        let earned: HashMap<String, u32> = [
            (SIRIUS.into(), 30), (BRATON.into(), 30), (KUVA.into(), 35), (STRIKE.into(), 12),
        ].into();
        let overview = build_mastery_overview(&catalog(), &corrections(), Some(&earned));
        assert_eq!((source(&overview, SIRIUS).earned_rank, source(&overview, SIRIUS).state), (Some(30), MasteryState::Mastered));
        assert_eq!(source(&overview, BRATON).state, MasteryState::Mastered);
        assert_eq!((source(&overview, KUVA).cap, source(&overview, KUVA).state), (40, MasteryState::Partial));
        assert_eq!(source(&overview, STRIKE).state, MasteryState::Partial);
        assert_eq!((source(&overview, CHAMBER).earned_rank, source(&overview, CHAMBER).state), (Some(0), MasteryState::Missing));
        let primary = overview.categories.iter().find(|c| c.category == "Primary").expect("primary");
        assert_eq!(primary.counts, MasteryCounts { total: 2, mastered: 1, partial: 1, missing: 0, unknown: 0 });
        assert_eq!(overview.counts, MasteryCounts { total: 15, mastered: 2, partial: 2, missing: 11, unknown: 0 });
    }

    #[test]
    fn no_observation_means_unknown_not_missing() {
        let overview = build_mastery_overview(&catalog(), &corrections(), None);
        assert!(overview.categories.iter().flat_map(|c| &c.sources)
            .all(|s| s.state == MasteryState::Unknown && s.earned_rank.is_none()));
        assert_eq!(overview.counts, MasteryCounts { total: 15, mastered: 0, partial: 0, missing: 0, unknown: 15 });
    }
}
