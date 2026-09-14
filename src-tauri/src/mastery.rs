use std::collections::{HashMap, HashSet};
use std::path::Path;
use tauri::Manager;
use crate::app_state::{AppState, CorrectionEntry};
use crate::catalogue::fix_category;
use crate::inventory_state::{inventory_path_aliases, load_inventory_state_cache, CREDITS_PATH};
use crate::mastery_nodes;
use crate::mastery_progress::{PlayerProgress, Provenance, ProvenanceState};
use crate::mastery_recipe::{is_blueprint, CraftPlan, Ledger};
use crate::mastery_relics::{Relics, RelicRoute};
use crate::mastery_rules::{self, Unobtainable};
use crate::monitor::CraftingJob;
use crate::settings::read_settings_map;
use crate::wfcd::{RecipeComponent, SyndicateOffer, WfcdItem};

const COLLECTION_CATEGORIES: [&str; 11] = [
    "Warframes", "Primary", "Secondary", "Melee", "Operator Weapons",
    "Archwing", "Companions", "Companion Weapons", "Vehicles", "Intrinsics", STAR_CHART,
];

const STAR_CHART: &str = "Star Chart";

#[derive(serde::Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MasteryState { Mastered, Partial, Missing, Unknown }

#[derive(serde::Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode { Normal, SteelPath }

/// A node's normal and Steel Path clears are two sources. The row's
/// `unique_name` is the node key, with `/steel_path` appended for the Steel
/// Path row.
#[derive(serde::Serialize, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct NodeInfo {
    pub(crate) key: &'static str,
    pub(crate) planet: &'static str,
    pub(crate) mode: Mode,
    pub(crate) junction: bool,
}

/// A star chart row has `cap` 1 and an `earned_rank` of 0 or 1.
#[derive(serde::Serialize, Clone, Debug)]
pub(crate) struct MasterySource {
    pub(crate) unique_name: String,
    pub(crate) name: String,
    pub(crate) category: String,
    pub(crate) image_name: Option<String>,
    pub(crate) mastery_req: Option<u32>,
    pub(crate) cap: u32,
    pub(crate) earned_rank: Option<u32>,
    pub(crate) remaining_mastery: Option<u32>,
    pub(crate) state: MasteryState,
    /// The table's class, regardless of settings.
    pub(crate) unobtainable: Option<Unobtainable>,
    /// Settings exclude the class: the source sits in the Unobtainable
    /// bucket, outside `total`, with its progress still shown.
    pub(crate) excluded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) node: Option<NodeInfo>,
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

/// Nodes and junctions share the `Missions` observation.
#[derive(serde::Serialize, Clone, Copy, Default, Debug)]
pub(crate) struct MasteryProvenance {
    pub(crate) equipment: Provenance,
    pub(crate) intrinsics: Provenance,
    pub(crate) nodes: Provenance,
    pub(crate) junctions: Provenance,
}

#[derive(serde::Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage { LevelClaim, Craft, Acquire }

/// Craft means everything is in stock. Build means intermediates need
/// crafting first. Farm means parts are short and no vendor sells them.
#[derive(serde::Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Action { Level, Claim, Spend, Craft, Build, Buy, Farm, Complete, Unlock }

/// The variant order runs by severity so `max` keeps a blocker over an
/// open question.
#[derive(serde::Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Access { Available, Unknown, Blocked }

#[derive(serde::Serialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct VendorOffer {
    pub(crate) syndicate: String,
    pub(crate) tier: String,
    pub(crate) blueprint: bool,
}

#[derive(serde::Serialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct TrackSpend {
    pub(crate) track: String,
    pub(crate) from: u32,
    pub(crate) to: u32,
}

#[derive(serde::Serialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Spend {
    pub(crate) ranks: u32,
    pub(crate) points: u32,
    pub(crate) mastery: u32,
    /// Lists only the tracks that gain a rank, in the game's order.
    pub(crate) tracks: Vec<TrackSpend>,
}

#[derive(serde::Serialize, Clone, Debug)]
pub(crate) struct Opportunity {
    #[serde(flatten)]
    pub(crate) source: MasterySource,
    pub(crate) stage: Stage,
    pub(crate) action: Action,
    pub(crate) owned: bool,
    /// None on caches from before levels were stored.
    pub(crate) owned_level: Option<u32>,
    pub(crate) build_completion_ms: Option<i64>,
    pub(crate) vendors: Vec<VendorOffer>,
    pub(crate) spend: Option<Spend>,
    pub(crate) access: Access,
    pub(crate) blockers: Vec<String>,
    /// A source that is neither owned nor building carries its plan whenever
    /// a recipe exists.
    pub(crate) craft: Option<CraftPlan>,
    /// Present when something short in the plan drops from a relic. The
    /// chance covers those parts only, and the other shortages stay in
    /// `craft`.
    pub(crate) relic: Option<RelicRoute>,
}

#[derive(serde::Serialize, Clone, Debug)]
pub(crate) struct MasteryOverview {
    pub(crate) counts: MasteryCounts,
    pub(crate) categories: Vec<MasteryCategory>,
    pub(crate) provenance: MasteryProvenance,
    pub(crate) opportunities: Vec<Opportunity>,
}

pub(crate) struct Observed<'a> {
    /// Equipment the player owns. The ledger never spends it as an ingredient.
    pub(crate) owned: &'a HashMap<String, i64>,
    pub(crate) stock: &'a HashMap<String, i64>,
    pub(crate) owned_levels: &'a HashMap<String, Vec<u32>>,
    pub(crate) mastery_rank: Option<u32>,
    pub(crate) crafting: &'a [CraftingJob],
    pub(crate) recipes: &'a HashMap<String, Vec<RecipeComponent>>,
    pub(crate) offers: &'a HashMap<String, Vec<SyndicateOffer>>,
    /// The record's `PlayerSkills`, or `None` while Intrinsics are Unknown.
    pub(crate) skills: Option<&'a HashMap<String, i64>>,
    pub(crate) relics: &'a Relics,
    pub(crate) now_ms: i64,
}

/// Planning every recipe against the inventory is too slow for the thread
/// a sync command runs on, so the work goes to the blocking pool.
#[tauri::command]
pub(crate) async fn get_mastery_overview(app: tauri::AppHandle) -> Result<MasteryOverview, String> {
    tauri::async_runtime::spawn_blocking(move || mastery_overview(&app.state::<AppState>()))
        .await
        .map_err(|e| e.to_string())
}

fn mastery_overview(state: &AppState) -> MasteryOverview {
    let player = state.local_player_name.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let excluded = excluded_classes(&state.settings_path);
    let (mut overview, skills, relic_names) = {
        let progress = state.mastery_progress.lock().unwrap_or_else(|e| e.into_inner());
        let items = state.wfcd_items.lock().unwrap_or_else(|e| e.into_inner());
        let record = progress.current(player.as_deref());
        let skills = record.filter(|r| r.intrinsics.state != ProvenanceState::Unknown).map(|r| r.skills.clone());
        let relic_names: HashMap<String, String> = items.iter()
            .filter(|i| i.category == "Relics")
            .map(|i| (i.unique_name.clone(), i.name.clone()))
            .collect();
        (build_mastery_overview(&items, &state.corrections, record, &excluded), skills, relic_names)
    };
    let inventory = load_inventory_state_cache(&state.inventory_state_cache_path);
    // Without an inventory scan we have no idea what the player owns, so skip suggestions.
    if inventory.items.is_empty() { return overview; }
    let stock = inventory.stackable_quantities();
    let relics = {
        let tables = state.relic_rewards.lock().unwrap_or_else(|e| e.into_inner());
        Relics::new(&stock, &tables, &relic_names)
    };
    let crafting = state.current_crafting.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let recipes = state.recipes.lock().unwrap_or_else(|e| e.into_inner());
    let offers = state.syndicate_catalog.lock().unwrap_or_else(|e| e.into_inner());
    overview.opportunities = suggest(&overview, &Observed {
        owned: &inventory.unique_quantities(),
        stock: &stock,
        owned_levels: &inventory.owned_levels(),
        mastery_rank: inventory.mastery_rank,
        crafting: &crafting,
        recipes: &recipes,
        offers: &offers,
        skills: skills.as_ref(),
        relics: &relics,
        now_ms: chrono::Utc::now().timestamp_millis(),
    });
    overview
}

/// Buys the cheapest next rank across the system's tracks, ties in track
/// order, until the cheapest rank left costs more than the balance. Every
/// rank pays the same mastery, so no other order gains more.
fn plan_spend(system: &mastery_rules::IntrinsicSystem, skills: &HashMap<String, i64>) -> Option<Spend> {
    let banked = system.banked(skills);
    let mut left = banked;
    let from = system.track_ranks(skills);
    let mut ranks = from.clone();
    while let Some(next) = (0..ranks.len())
        .filter(|&i| ranks[i] < mastery_rules::INTRINSIC_RANK_CAP)
        .min_by_key(|&i| system.rank_costs[ranks[i] as usize])
    {
        let cost = system.rank_costs[ranks[next] as usize];
        if cost > left { break; }
        left -= cost;
        ranks[next] += 1;
    }
    let tracks: Vec<TrackSpend> = system.tracks.iter().zip(&from).zip(&ranks)
        .filter(|((_, from), to)| to > from)
        .map(|(((track, _), &from), &to)| TrackSpend { track: (*track).into(), from, to })
        .collect();
    if tracks.is_empty() { return None; }
    let gained: u32 = tracks.iter().map(|t| t.to - t.from).sum();
    Some(Spend {
        ranks: gained,
        points: banked - left,
        mastery: gained * mastery_rules::mastery_per_rank(system.points),
        tracks,
    })
}

/// A Foundry job carries the blueprint path. Each recipe lists its own
/// blueprint as the one component with an empty component list; part
/// blueprints like a chassis or barrel list their ingredients.
fn blueprint_results(recipes: &HashMap<String, Vec<RecipeComponent>>) -> HashMap<&str, &str> {
    recipes.iter().flat_map(|(result, components)| {
        components.iter()
            .filter(|c| is_blueprint(c))
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

/// A junction that is not cleared blocks everything on the planet behind it.
/// Past that gate nothing observed says whether a node is unlocked, so its
/// access is Unknown.
fn node_opportunity(source: &MasterySource, node: &NodeInfo, sources: &HashMap<&str, &MasterySource>) -> Option<Opportunity> {
    // An Unknown row (every Steel Path row, or no Missions field) has no
    // evidence of anything left to do.
    if source.state != MasteryState::Missing { return None; }
    let gate = mastery_nodes::PLANETS.iter().find(|p| p.name == node.planet).and_then(|p| p.gate);
    let (access, blockers) = match gate.and_then(|gate| sources.get(gate)).filter(|g| g.state == MasteryState::Missing) {
        Some(gate) => (Access::Blocked, vec![format!("{} not cleared", gate.name)]),
        None if node.junction => (Access::Unknown, vec!["Junction tasks not observed".into()]),
        None => (Access::Unknown, vec!["Node unlock not observed".into()]),
    };
    Some(Opportunity {
        source: source.clone(), stage: Stage::Acquire,
        action: if node.junction { Action::Unlock } else { Action::Complete },
        owned: false, owned_level: None, build_completion_ms: None, vendors: vec![], spend: None, access, blockers, craft: None, relic: None,
    })
}

pub(crate) fn suggest(overview: &MasteryOverview, observed: &Observed) -> Vec<Opportunity> {
    let blueprint_results = blueprint_results(observed.recipes);
    let building: HashMap<&str, i64> = observed.crafting.iter()
        .map(|job| (blueprint_results.get(job.unique_name.as_str()).copied().unwrap_or(&job.unique_name), job.completion_ms))
        .collect();
    let by_name: HashMap<&str, &MasterySource> = overview.categories.iter().flat_map(|c| &c.sources)
        .map(|s| (s.unique_name.as_str(), s)).collect();
    let mastered: HashSet<&str> = overview.categories.iter().flat_map(|c| &c.sources)
        .filter(|s| s.state == MasteryState::Mastered).map(|s| s.unique_name.as_str()).collect();
    // TODO: let the player authorize spending mastered equipment. Until then
    // Suggestions authorizes nothing.
    let (mut ledger, _) = Ledger::new(observed.stock, observed.owned, &mastered, &HashMap::new());

    let mut opportunities: Vec<Opportunity> = overview.categories.iter().flat_map(|c| &c.sources)
        .filter(|s| !s.excluded && s.state != MasteryState::Mastered)
        .filter_map(|source| {
            if let Some(node) = &source.node { return node_opportunity(source, node, &by_name); }
            if let Some(system) = mastery_rules::intrinsic_system(&source.unique_name) {
                let spend = plan_spend(system, observed.skills?)?;
                return Some(Opportunity {
                    source: source.clone(), stage: Stage::LevelClaim, action: Action::Spend,
                    owned: false, owned_level: None, build_completion_ms: None, vendors: vec![], spend: Some(spend),
                    access: Access::Available, blockers: vec![], craft: None, relic: None,
                });
            }
            let owned_level = observed.owned_levels.get(&source.unique_name).and_then(|levels| levels.iter().max().copied());
            let owned = owned_level.is_some() || observed.owned.get(&source.unique_name).is_some_and(|&copies| copies > 0);
            let build_completion_ms = building.get(source.unique_name.as_str()).copied();
            let vendors = vendor_offers(&source.unique_name, observed.offers);
            let action = if owned { Action::Level }
                else if build_completion_ms.is_some() { Action::Claim }
                // A placeholder until the ledger pass below settles every recipe row.
                else if observed.recipes.contains_key(&source.unique_name) { Action::Farm }
                else if !vendors.is_empty() { Action::Buy }
                else { return None };
            Some(Opportunity {
                source: source.clone(), stage: Stage::Acquire, action, owned, owned_level, build_completion_ms, vendors, spend: None,
                access: Access::Available, blockers: vec![], craft: None, relic: None,
            })
        })
        .collect();

    // Recipes draw on one ledger in display order, which is why every recipe
    // row sits in the Craft stage whatever its final action, except a relic
    // farm, which is the furthest from done and so draws last and lands in
    // Acquire. A standalone pass finds which targets are craftable on their
    // own, so those plan first and keep their ingredients instead of losing
    // them to a target short anyway.
    let mut crafting: Vec<usize> = (0..opportunities.len()).filter(|&i| opportunities[i].action == Action::Farm).collect();
    let standalone: HashMap<String, u8> = crafting.iter().map(|&i| {
        let path = &opportunities[i].source.unique_name;
        let plan = ledger.clone().plan(path, &observed.recipes[path]);
        let rank = if plan.craftable_now() { 0 }
            else if plan.buildable() { 1 }
            else if plan.shortages().any(|r| observed.relics.is_part(&r.unique_name)) { 3 }
            else { 2 };
        (path.clone(), rank)
    }).collect();
    let rank = |o: &Opportunity| standalone.get(&o.source.unique_name).copied().unwrap_or(0);
    crafting.sort_by(|&a, &b| rank(&opportunities[a]).cmp(&rank(&opportunities[b])).then_with(|| by_mastery_then_name(&opportunities[a], &opportunities[b])));
    for i in crafting {
        let o = &mut opportunities[i];
        let plan = ledger.plan(&o.source.unique_name, &observed.recipes[&o.source.unique_name]);
        o.action = if plan.craftable_now() { Action::Craft }
            else if plan.buildable() { Action::Build }
            else if !o.vendors.is_empty() { Action::Buy }
            else { Action::Farm };
        o.relic = observed.relics.route(&plan);
        o.craft = Some(plan);
    }

    // Intrinsic and star chart rows settled their stage and access when they
    // were built.
    for o in opportunities.iter_mut().filter(|o| !matches!(o.action, Action::Spend | Action::Complete | Action::Unlock)) {
        o.stage = match o.action {
            Action::Level | Action::Claim => Stage::LevelClaim,
            _ if o.craft.is_some() && o.relic.is_none() => Stage::Craft,
            _ => Stage::Acquire,
        };
        (o.access, o.blockers) = access(o, observed);
    }
    // Ledger order only shows in the Craft stage. Relic farms drew last, and
    // in Acquire they sort by mastery like every other row.
    let craft_order = |a: &Opportunity, b: &Opportunity| if a.stage == Stage::Craft { rank(a).cmp(&rank(b)) } else { std::cmp::Ordering::Equal };
    opportunities.sort_by(|a, b| a.stage.cmp(&b.stage).then_with(|| craft_order(a, b)).then_with(|| by_mastery_then_name(a, b)));
    opportunities
}

/// A spend ranks by what the banked points buy, which can be less than
/// the system's remaining mastery.
fn gain(o: &Opportunity) -> Option<u32> {
    o.spend.as_ref().map(|s| s.mastery).or(o.source.remaining_mastery)
}

fn by_mastery_then_name(a: &Opportunity, b: &Opportunity) -> std::cmp::Ordering {
    match (gain(a), gain(b)) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
    .then_with(|| a.source.name.cmp(&b.source.name))
    .then_with(|| a.source.unique_name.cmp(&b.source.unique_name))
}

fn access(o: &Opportunity, observed: &Observed) -> (Access, Vec<String>) {
    let mut access = Access::Available;
    let mut blockers = vec![];
    let mut note = |worst: Access, text: String| {
        access = access.max(worst);
        blockers.push(text);
    };
    match o.action {
        Action::Level => {}
        Action::Spend | Action::Complete | Action::Unlock => unreachable!("settled when the row was built"),
        Action::Claim => if o.build_completion_ms.is_some_and(|done| done > observed.now_ms) {
            note(Access::Blocked, "Still building".into());
        },
        Action::Craft | Action::Build | Action::Buy | Action::Farm => {
            match (o.source.mastery_req, observed.mastery_rank) {
                (Some(required), Some(rank)) if required > rank => note(Access::Blocked, format!("Requires MR {required}")),
                (Some(required), None) if required > 0 => note(Access::Unknown, "Mastery Rank not observed".into()),
                _ => {}
            }
            match o.action {
                Action::Craft | Action::Build => {
                    let plan = o.craft.as_ref().expect("a craft row carries its plan");
                    if plan.credits_short > 0 { note(Access::Blocked, format!("Needs {} more credits", thousands(plan.credits_short))); }
                    else if plan.credits.is_none() { note(Access::Unknown, "Credit cost unknown".into()); }
                    else if !observed.stock.contains_key(CREDITS_PATH) { note(Access::Unknown, "Credits not observed".into()); }
                }
                // TODO: read standing from the scan. Until then every Buy stays Unknown.
                Action::Buy => note(Access::Unknown, "Standing not observed".into()),
                _ => {
                    let plan = o.craft.as_ref().expect("a farm row carries its plan");
                    let relic_parts: Vec<&str> = o.relic.iter().flat_map(|r| &r.parts).map(|p| p.unique_name.as_str()).collect();
                    if plan.shortages().any(|r| !relic_parts.contains(&r.unique_name.as_str())) {
                        note(Access::Unknown, "Drop sources unknown".into());
                    }
                }
            }
        }
    }
    (access, blockers)
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) { out.push(','); }
        out.push(c);
    }
    out
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
    let intrinsics = progress.map(|p| p.intrinsics).unwrap_or_default();
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
            remaining_mastery: None,
            state: MasteryState::Unknown,
            unobtainable: correction.and_then(|c| c.unobtainable),
            excluded: correction.and_then(|c| c.unobtainable).is_some_and(|class| excluded.contains(&class)),
            node: None,
        });
    }

    let mut counts = MasteryCounts::default();
    let mut categories: Vec<MasteryCategory> = COLLECTION_CATEGORIES.iter()
        .map(|c| MasteryCategory { category: (*c).into(), counts: MasteryCounts::default(), sources: vec![] })
        .collect();
    let mut place = |mut source: MasterySource, rank: Option<u32>| {
        if let Some(rank) = rank {
            source.earned_rank = Some(rank);
            source.remaining_mastery = Some(source.cap.saturating_sub(rank) * mastery_rules::mastery_per_rank(&source.unique_name));
            source.state = if rank >= source.cap { MasteryState::Mastered }
                else if rank > 0 { MasteryState::Partial }
                else { MasteryState::Missing };
        }
        let category = categories.iter_mut().find(|c| c.category == source.category).expect("category comes from COLLECTION_CATEGORIES");
        category.counts.add(&source);
        counts.add(&source);
        category.sources.push(source);
    };
    for source in sources.into_values() {
        let rank = affinity.get(source.unique_name.as_str())
            .map(|&earned| mastery_rules::xp_to_rank(earned, &source.unique_name).min(source.cap));
        place(source, equipment.resolve(rank, 0));
    }
    for system in &mastery_rules::INTRINSIC_SYSTEMS {
        let source = MasterySource {
            unique_name: system.points.into(),
            name: system.name.into(),
            category: "Intrinsics".into(),
            image_name: None,
            mastery_req: None,
            cap: system.rank_cap(),
            earned_rank: None,
            remaining_mastery: None,
            state: MasteryState::Unknown,
            unobtainable: None,
            excluded: false,
            node: None,
        };
        let rank = progress.filter(|p| p.intrinsics.state != ProvenanceState::Unknown)
            .map(|p| system.track_ranks(&p.skills).iter().sum());
        place(source, rank);
    }
    // Equipment sorts by name. The star chart rows keep the table's chart
    // order, which is why they are pushed after this sort.
    for category in &mut categories {
        category.sources.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.unique_name.cmp(&b.unique_name)));
    }

    let nodes = progress.map(|p| p.nodes).unwrap_or_default();
    let chart = categories.iter_mut().find(|c| c.category == STAR_CHART).expect("category comes from COLLECTION_CATEGORIES");
    for node in mastery_nodes::all() {
        let completes = progress.and_then(|p| p.missions.get(node.key)).map(|m| m.completes > 0);
        // TODO: the Steel Path row waits on the meaning of `Tier`. Until then
        // a node first cleared on the Steel Path also reads as cleared here.
        let cleared = nodes.resolve(completes, false);
        for mode in [Mode::Normal, Mode::SteelPath] {
            let cleared = if mode == Mode::Normal { cleared } else { None };
            let source = MasterySource {
                unique_name: match mode { Mode::Normal => node.key.into(), Mode::SteelPath => format!("{}/steel_path", node.key) },
                name: node.name.into(),
                category: STAR_CHART.into(),
                image_name: None,
                mastery_req: None,
                cap: 1,
                earned_rank: cleared.map(u32::from),
                remaining_mastery: cleared.filter(|_| node.junction).map(|done| if done { 0 } else { mastery_nodes::JUNCTION_MASTERY }),
                state: match cleared { Some(true) => MasteryState::Mastered, Some(false) => MasteryState::Missing, None => MasteryState::Unknown },
                unobtainable: None,
                excluded: false,
                node: Some(NodeInfo { key: node.key, planet: node.planet.name, mode, junction: node.junction }),
            };
            chart.counts.add(&source);
            counts.add(&source);
            chart.sources.push(source);
        }
    }
    categories.retain(|c| !c.sources.is_empty());
    let provenance = MasteryProvenance { equipment, intrinsics, nodes, junctions: nodes };
    MasteryOverview { counts, categories, provenance, opportunities: vec![] }
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
    use std::sync::LazyLock;
    use crate::mastery_progress::{PlayerProgress, Provenance, ProvenanceState};
    use crate::memory_scanner::BlobMission;
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
    const RAILJACK: &str = "LPP_SPACE";
    const DRIFTER: &str = "LPP_DRIFTER";

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
            ..Default::default()
        }
    }

    fn with_missions(mut progress: PlayerProgress, cleared: &[(&str, u32, Option<u32>)]) -> PlayerProgress {
        progress.missions = cleared.iter()
            .map(|(key, completes, tier)| ((*key).to_string(), BlobMission { completes: *completes, tier: *tier }))
            .collect();
        progress.nodes = progress.equipment;
        progress
    }

    /// Every node and junction counts once per mode.
    const CHART_ROWS: u32 = 2 * 265;

    fn star_chart(overview: &MasteryOverview) -> &MasteryCategory {
        overview.categories.iter().find(|c| c.category == "Star Chart").expect("star chart listed")
    }

    fn with_skills(mut progress: PlayerProgress, state: ProvenanceState, skills: &[(&str, i64)]) -> PlayerProgress {
        progress.skills = skills.iter().map(|(field, value)| ((*field).to_string(), *value)).collect();
        progress.intrinsics = Provenance { state, observed_at: Some(2_000) };
        progress
    }

    /// The saved capture's `PlayerSkills`.
    const CAPTURED_SKILLS: [(&str, i64); 11] = [
        ("LPP_SPACE", 89_930), ("LPS_GUNNERY", 8), ("LPS_ENGINEERING", 8), ("LPS_TACTICAL", 10), ("LPS_PILOTING", 9), ("LPS_COMMAND", 10),
        ("LPP_DRIFTER", 0), ("LPS_DRIFT_RIDING", 10), ("LPS_DRIFT_COMBAT", 10), ("LPS_DRIFT_OPPORTUNITY", 10), ("LPS_DRIFT_ENDURANCE", 10),
    ];

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
            "Archwing", "Companions", "Companion Weapons", "Vehicles", "Intrinsics", "Star Chart"]);
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
        assert_eq!(names(&overview, "Intrinsics"), ["Drifter", "Railjack"]);
        assert_eq!(overview.counts.total, 18 + CHART_ROWS);
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
        assert_eq!((source(&overview, BRATON).state, source(&overview, BRATON).remaining_mastery), (MasteryState::Mastered, Some(0)));
        assert_eq!((source(&overview, KUVA).cap, source(&overview, KUVA).state, source(&overview, KUVA).remaining_mastery), (40, MasteryState::Partial, Some(500)));
        assert_eq!(source(&overview, STRIKE).state, MasteryState::Partial);
        assert_eq!((source(&overview, CHAMBER).earned_rank, source(&overview, CHAMBER).state, source(&overview, CHAMBER).remaining_mastery), (Some(0), MasteryState::Missing, Some(3_000)));
        assert_eq!(source(&overview, MECH).remaining_mastery, Some(8_000));
        let primary = overview.categories.iter().find(|c| c.category == "Primary").expect("primary");
        assert_eq!(primary.counts, MasteryCounts { total: 2, mastered: 1, partial: 1, missing: 0, unknown: 0, unobtainable: 0 });
        assert_eq!(overview.counts, MasteryCounts { total: 18 + CHART_ROWS, mastered: 2, partial: 2, missing: 12, unknown: 2 + CHART_ROWS, unobtainable: 0 });
    }

    /// Completion is the only fact a `Missions` entry establishes: a node
    /// absent from a confirmed array is Missing, and the tier says nothing
    /// about the Steel Path row until its meaning is verified.
    #[test]
    fn star_chart_rows_follow_the_confirmed_missions_field() {
        let progress = with_missions(observed(ProvenanceState::Confirmed, Some(1_000), &[]), &[
            ("SolNode27", 14, Some(1)), ("EarthToVenusJunction", 2, None), ("SolNode239", 1, Some(1)),
        ]);
        let overview = build_mastery_overview(&catalog(), &corrections(), Some(&progress), &HashSet::new());
        assert_eq!(overview.provenance.nodes, Provenance { state: ProvenanceState::Confirmed, observed_at: Some(1_000) });
        assert_eq!(overview.provenance.junctions, overview.provenance.nodes);

        let e_prime = source(&overview, "SolNode27");
        assert_eq!((e_prime.name.as_str(), e_prime.category.as_str(), e_prime.earned_rank, e_prime.state), ("E Prime", "Star Chart", Some(1), MasteryState::Mastered));
        let node = e_prime.node.as_ref().expect("node info");
        assert_eq!((node.key, node.planet, node.mode, node.junction), ("SolNode27", "Earth", Mode::Normal, false));
        let steel = source(&overview, "SolNode27/steel_path");
        assert_eq!((steel.earned_rank, steel.state, steel.node.as_ref().map(|n| n.mode)), (None, MasteryState::Unknown, Some(Mode::SteelPath)));
        let junction = source(&overview, "EarthToVenusJunction");
        assert_eq!((junction.name.as_str(), junction.state, junction.node.as_ref().map(|n| n.junction)), ("Venus Junction", MasteryState::Mastered, Some(true)));
        let mariana = source(&overview, "SolNode89");
        assert_eq!((mariana.earned_rank, mariana.state), (Some(0), MasteryState::Missing));
        assert!(overview.categories.iter().flat_map(|c| &c.sources).all(|s| s.unique_name != "SolNode239"), "a key outside the table is not a source");

        let chart = star_chart(&overview);
        assert_eq!(chart.counts, MasteryCounts { total: CHART_ROWS, mastered: 2, partial: 0, missing: 263, unknown: 265, unobtainable: 0 });
        assert_eq!(overview.counts.total, 18 + CHART_ROWS);
        let venus: Vec<(&str, Mode)> = chart.sources.iter()
            .filter(|s| s.node.as_ref().is_some_and(|n| n.planet == "Venus"))
            .map(|s| (s.name.as_str(), s.node.as_ref().expect("node").mode)).take(5).collect();
        assert_eq!(venus, [
            ("Mercury Junction", Mode::Normal), ("Mercury Junction", Mode::SteelPath),
            ("Aphrodite", Mode::Normal), ("Aphrodite", Mode::SteelPath), ("Cytherean", Mode::Normal),
        ], "junctions lead their planet, nodes follow by name, each mode paired");
        assert_eq!(overview.categories.last().map(|c| c.category.as_str()), Some("Star Chart"));
    }

    #[test]
    fn missions_field_absent_leaves_every_star_chart_row_unknown() {
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[(BRATON, 450_000)]);
        let overview = build_mastery_overview(&catalog(), &corrections(), Some(&progress), &HashSet::new());
        assert_eq!(overview.provenance.nodes, Provenance::default());
        assert_eq!(source(&overview, BRATON).state, MasteryState::Mastered);
        let chart = star_chart(&overview);
        assert!(chart.sources.iter().all(|s| s.state == MasteryState::Unknown && s.earned_rank.is_none()));
        assert_eq!(chart.counts.unknown, CHART_ROWS);
    }

    #[test]
    fn no_observation_means_unknown_not_missing() {
        let overview = build_mastery_overview(&catalog(), &corrections(), None, &HashSet::new());
        assert!(overview.categories.iter().flat_map(|c| &c.sources)
            .all(|s| s.state == MasteryState::Unknown && s.earned_rank.is_none() && s.remaining_mastery.is_none()));
        assert_eq!(overview.counts, MasteryCounts { total: 18 + CHART_ROWS, mastered: 0, partial: 0, missing: 0, unknown: 18 + CHART_ROWS, unobtainable: 0 });
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
        assert_eq!(overview.counts, MasteryCounts { total: 18 + CHART_ROWS, mastered: 1, partial: 1, missing: 0, unknown: 16 + CHART_ROWS, unobtainable: 0 });
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
        assert_eq!(overview.counts.total, 19 + CHART_ROWS);

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
        assert_eq!(overview.counts, MasteryCounts { total: 17 + CHART_ROWS, mastered: 0, partial: 0, missing: 15, unknown: 2 + CHART_ROWS, unobtainable: 3 });
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
            assert_eq!((overview.counts.total, overview.counts.unobtainable), (18 + CHART_ROWS, 2), "{class:?}");
        }

        let overview = build_mastery_overview(&items, &corrections, Some(&progress), &HashSet::new());
        assert_eq!(overview.counts, MasteryCounts { total: 20 + CHART_ROWS, mastered: 1, partial: 0, missing: 17, unknown: 2 + CHART_ROWS, unobtainable: 0 });
    }

    fn with_suggestions(items: &[WfcdItem], corrections: &HashMap<String, CorrectionEntry>, progress: Option<&PlayerProgress>, excluded: &HashSet<Unobtainable>, observed: &Observed) -> MasteryOverview {
        let mut overview = build_mastery_overview(items, corrections, progress, excluded);
        overview.opportunities = suggest(&overview, observed);
        overview
    }

    static NO_STOCK: LazyLock<HashMap<String, i64>> = LazyLock::new(HashMap::new);
    static NO_RELICS: LazyLock<Relics> = LazyLock::new(Relics::default);

    fn observed_gear<'a>(owned: &'a HashMap<String, i64>, owned_levels: &'a HashMap<String, Vec<u32>>, crafting: &'a [CraftingJob], recipes: &'a HashMap<String, Vec<RecipeComponent>>, offers: &'a HashMap<String, Vec<SyndicateOffer>>, mastery_rank: Option<u32>) -> Observed<'a> {
        Observed { owned, stock: &NO_STOCK, owned_levels, mastery_rank, crafting, recipes, offers, skills: None, relics: &NO_RELICS, now_ms: 1_000_000 }
    }

    fn observed_skills<'a>(progress: &'a PlayerProgress, recipes: &'a HashMap<String, Vec<RecipeComponent>>, offers: &'a HashMap<String, Vec<SyndicateOffer>>, owned: &'a HashMap<String, i64>, levels: &'a HashMap<String, Vec<u32>>) -> Observed<'a> {
        Observed { owned, stock: &NO_STOCK, owned_levels: levels, mastery_rank: Some(30), crafting: &[], recipes, offers, skills: Some(&progress.skills), relics: &NO_RELICS, now_ms: 1_000_000 }
    }

    fn track(track: &str, from: u32, to: u32) -> TrackSpend {
        TrackSpend { track: track.into(), from, to }
    }

    const FERRITE: &str = "/Lotus/Types/Items/MiscItems/Ferrite";
    const CHASSIS: &str = "/Lotus/Types/Recipes/Parts/Chassis";
    const CHASSIS_BP: &str = "/Lotus/Types/Recipes/Parts/ChassisBlueprint";

    fn recipe(source: &str, blueprint: &str) -> (String, Vec<RecipeComponent>) {
        let component = |unique_name: &str, name: &str, count: u32, credits: Option<u32>, components: Vec<RecipeComponent>| RecipeComponent {
            unique_name: unique_name.into(), name: name.into(), count, result_count: 1, components, credits, reusable: false,
        };
        let chassis = component(CHASSIS, "Chassis", 1, None, vec![
            component(CHASSIS_BP, "Chassis Blueprint", 1, Some(5_000), vec![]),
            component(FERRITE, "Ferrite", 100, None, vec![]),
        ]);
        (source.into(), vec![component(blueprint, "Blueprint", 1, Some(15_000), vec![]), chassis, component(FERRITE, "Ferrite", 50, None, vec![])])
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
        opportunities.iter().map(|o| (o.source.name.as_str(), o.action, o.source.remaining_mastery, o.access)).collect()
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
    fn nodes_suggest_complete_and_unlock_with_junction_gates_as_the_only_blockers() {
        let owned = HashMap::new();
        let levels = HashMap::new();
        let recipes = HashMap::new();
        let offers = HashMap::new();
        let progress = with_missions(observed(ProvenanceState::Confirmed, Some(1_000), &[]), &[
            ("EarthToVenusJunction", 1, None), ("SolNode27", 1, None),
        ]);
        let overview = with_suggestions(&catalog(), &corrections(), Some(&progress), &HashSet::new(),
            &observed_gear(&owned, &levels, &[], &recipes, &offers, Some(2)));
        let nodes: Vec<&Opportunity> = overview.opportunities.iter().filter(|o| o.source.node.is_some()).collect();
        assert_eq!(nodes.len(), 265 - 2, "every missing normal row; Steel Path rows are Unknown and absent");
        assert!(nodes.iter().all(|o| o.stage == Stage::Acquire && !o.owned && o.vendors.is_empty()));
        let find = |name: &str| nodes.iter().find(|o| o.source.name == name).unwrap_or_else(|| panic!("{name} suggested"));
        assert_eq!(nodes.iter().take(2).map(|o| o.source.name.as_str()).collect::<Vec<_>>(), ["Ceres Junction", "Eris Junction"],
            "junctions carry known mastery and lead the stage");
        let mercury = find("Mercury Junction");
        assert_eq!((mercury.action, mercury.source.remaining_mastery, mercury.access), (Action::Unlock, Some(1_000), Access::Unknown));
        assert_eq!(mercury.blockers, ["Junction tasks not observed"]);
        let mars = find("Mars Junction");
        assert_eq!((mars.action, mars.access), (Action::Unlock, Access::Unknown));
        let aphrodite = find("Aphrodite");
        assert_eq!((aphrodite.action, aphrodite.source.remaining_mastery, aphrodite.access), (Action::Complete, None, Access::Unknown));
        assert_eq!(aphrodite.blockers, ["Node unlock not observed"]);
        let apollodorus = find("Apollodorus");
        assert_eq!((apollodorus.action, apollodorus.access), (Action::Complete, Access::Blocked));
        assert_eq!(apollodorus.blockers, ["Mercury Junction not cleared"]);
        let ceres = find("Ceres Junction");
        assert_eq!((ceres.access, ceres.blockers.clone()), (Access::Blocked, vec!["Mars Junction not cleared".to_string()]));
        assert!(nodes.iter().all(|o| o.source.name != "E Prime" && o.source.name != "Venus Junction"), "cleared rows are not suggested");

        let unknown = with_suggestions(&catalog(), &corrections(), Some(&observed(ProvenanceState::Confirmed, Some(1_000), &[])), &HashSet::new(),
            &observed_gear(&owned, &levels, &[], &recipes, &offers, Some(2)));
        assert!(unknown.opportunities.iter().all(|o| o.source.node.is_none()), "no Missions field: nothing to suggest");
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

    #[test]
    fn each_intrinsic_system_is_one_row_of_summed_track_ranks() {
        let progress = with_skills(observed(ProvenanceState::Confirmed, Some(1_000), &[]), ProvenanceState::Confirmed, &CAPTURED_SKILLS);
        let overview = build_mastery_overview(&catalog(), &corrections(), Some(&progress), &HashSet::new());
        assert_eq!(overview.provenance.intrinsics, Provenance { state: ProvenanceState::Confirmed, observed_at: Some(2_000) });
        assert_eq!(names(&overview, "Intrinsics"), ["Drifter", "Railjack"]);
        let railjack = source(&overview, RAILJACK);
        assert_eq!((railjack.cap, railjack.earned_rank, railjack.remaining_mastery, railjack.state), (50, Some(45), Some(7_500), MasteryState::Partial));
        assert_eq!((railjack.image_name.as_deref(), railjack.mastery_req, railjack.unobtainable), (None, None, None));
        let drifter = source(&overview, DRIFTER);
        assert_eq!((drifter.cap, drifter.earned_rank, drifter.remaining_mastery, drifter.state), (40, Some(40), Some(0), MasteryState::Mastered));
        let intrinsics = overview.categories.iter().find(|c| c.category == "Intrinsics").expect("intrinsics");
        assert_eq!(intrinsics.counts, MasteryCounts { total: 2, mastered: 1, partial: 1, missing: 0, unknown: 0, unobtainable: 0 });

        // A confirmed object with no track fields is an account that never earned a rank.
        let progress = with_skills(observed(ProvenanceState::Confirmed, Some(1_000), &[]), ProvenanceState::Confirmed, &[]);
        let overview = build_mastery_overview(&catalog(), &corrections(), Some(&progress), &HashSet::new());
        assert_eq!((source(&overview, RAILJACK).earned_rank, source(&overview, RAILJACK).state), (Some(0), MasteryState::Missing));

        // A rank past 10 or below 0 is not one the game hands out.
        let progress = with_skills(observed(ProvenanceState::Confirmed, Some(1_000), &[]), ProvenanceState::Confirmed, &[("LPS_DRIFT_RIDING", 12), ("LPS_DRIFT_COMBAT", -3)]);
        let overview = build_mastery_overview(&catalog(), &corrections(), Some(&progress), &HashSet::new());
        assert_eq!(source(&overview, DRIFTER).earned_rank, Some(10));
    }

    #[test]
    fn intrinsics_stay_unknown_while_equipment_is_confirmed() {
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[(BRATON, 450_000)]);
        let overview = build_mastery_overview(&catalog(), &corrections(), Some(&progress), &HashSet::new());
        assert_eq!(overview.provenance.intrinsics, Provenance::default());
        assert_eq!(source(&overview, BRATON).state, MasteryState::Mastered);
        for system in [RAILJACK, DRIFTER] {
            assert_eq!((source(&overview, system).earned_rank, source(&overview, system).state), (None, MasteryState::Unknown), "{system}");
        }
        assert_eq!(overview.counts.unknown, 2 + CHART_ROWS);
    }

    /// Worked by hand from the wiki's cost tables. 18 of the 20 Railjack
    /// points buy seven ranks and the eighth, Tactical 4 at 8, is out of
    /// reach. Of the 460 Drifter points, Endurance 9 at 205 goes first and
    /// Combat 10 at 255 wins the tie with Endurance 10.
    #[test]
    fn banked_points_buy_the_cheapest_rank_first_across_a_systems_tracks() {
        let progress = with_skills(observed(ProvenanceState::Confirmed, Some(1_000), &[]), ProvenanceState::Confirmed, &[
            ("LPP_SPACE", 20), ("LPS_TACTICAL", 3), ("LPS_PILOTING", 5), ("LPS_COMMAND", 2),
            ("LPP_DRIFTER", 460), ("LPS_DRIFT_COMBAT", 9), ("LPS_DRIFT_RIDING", 10), ("LPS_DRIFT_OPPORTUNITY", 10), ("LPS_DRIFT_ENDURANCE", 8),
        ]);
        let owned = HashMap::new();
        let levels: HashMap<String, Vec<u32>> = [(KUVA.to_string(), vec![0])].into();
        let (recipes, offers) = (HashMap::new(), HashMap::new());
        let overview = with_suggestions(&catalog(), &corrections(), Some(&progress), &HashSet::new(), &observed_skills(&progress, &recipes, &offers, &owned, &levels));
        assert_eq!(summary(&overview.opportunities), [
            ("Railjack", Action::Spend, Some(60_000), Access::Available),
            ("Kuva Karak", Action::Level, Some(4_000), Access::Available),
            ("Drifter", Action::Spend, Some(4_500), Access::Available),
        ], "a spend sorts by what the points buy");
        assert!(overview.opportunities.iter().all(|o| o.stage == Stage::LevelClaim && o.blockers.is_empty() && o.vendors.is_empty()));
        let railjack = overview.opportunities[0].spend.as_ref().expect("spend");
        assert_eq!((railjack.ranks, railjack.points, railjack.mastery), (7, 18, 10_500));
        assert_eq!(railjack.tracks, [track("Gunnery", 0, 3), track("Engineering", 0, 3), track("Command", 2, 3)]);
        let drifter = overview.opportunities[2].spend.as_ref().expect("spend");
        assert_eq!((drifter.ranks, drifter.points, drifter.mastery), (2, 460, 3_000));
        assert_eq!(drifter.tracks, [track("Combat", 9, 10), track("Endurance", 8, 9)]);
    }

    #[test]
    fn no_spend_without_enough_banked_points_or_without_an_observation() {
        let (owned, levels, recipes, offers) = (HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new());
        let progress = with_skills(observed(ProvenanceState::Confirmed, Some(1_000), &[]), ProvenanceState::Confirmed, &CAPTURED_SKILLS);
        let overview = with_suggestions(&catalog(), &corrections(), Some(&progress), &HashSet::new(), &observed_skills(&progress, &recipes, &offers, &owned, &levels));
        assert_eq!(summary(&overview.opportunities), [("Railjack", Action::Spend, Some(7_500), Access::Available)], "a mastered Drifter has nothing to buy");
        let railjack = overview.opportunities[0].spend.as_ref().expect("spend");
        assert_eq!((railjack.ranks, railjack.points, railjack.mastery), (5, 2_048, 7_500));
        assert_eq!(railjack.tracks, [track("Piloting", 9, 10), track("Gunnery", 8, 10), track("Engineering", 8, 10)]);

        // One system has nothing banked and the other sits one point short of its cheapest rank.
        let progress = with_skills(observed(ProvenanceState::Confirmed, Some(1_000), &[]), ProvenanceState::Confirmed, &[("LPS_GUNNERY", 4), ("LPP_DRIFTER", 19)]);
        let overview = with_suggestions(&catalog(), &corrections(), Some(&progress), &HashSet::new(), &observed_skills(&progress, &recipes, &offers, &owned, &levels));
        assert!(overview.opportunities.is_empty(), "{:?}", summary(&overview.opportunities));

        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[]);
        let overview = with_suggestions(&catalog(), &corrections(), Some(&progress), &HashSet::new(), &observed_gear(&owned, &levels, &[], &recipes, &offers, Some(30)));
        assert!(overview.opportunities.is_empty(), "Unknown Intrinsics suggest nothing");
    }

    #[test]
    fn craft_rows_split_now_from_builds_and_farming_and_draw_on_one_ledger_in_display_order() {
        const SIRIUS_BP: &str = "/Lotus/Types/Recipes/WarframeRecipes/SiriusOrionBlueprint";
        const BRATON_BP: &str = "/Lotus/Types/Recipes/Weapons/BratonBlueprint";
        const IMPERATOR_BP: &str = "/Lotus/Types/Recipes/Weapons/ImperatorBlueprint";
        const SWEEPER_BP: &str = "/Lotus/Types/Recipes/Weapons/SweeperBlueprint";
        let owned = HashMap::new();
        let levels = HashMap::new();
        let without_chassis = |(path, mut components): (String, Vec<RecipeComponent>)| {
            components.retain(|c| c.unique_name != CHASSIS);
            (path, components)
        };
        let recipes: HashMap<String, Vec<RecipeComponent>> = [
            without_chassis(recipe(SIRIUS, SIRIUS_BP)), recipe(BRATON, BRATON_BP), without_chassis(recipe(IMPERATOR, IMPERATOR_BP)), recipe(SWEEPER, SWEEPER_BP),
        ].into();
        let offers: HashMap<String, Vec<SyndicateOffer>> = [("Cephalon Simaris".to_string(), vec![offer(SWEEPER_BP, "Neutral", Some(SWEEPER))])].into();
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[]);
        // Ferrite runs out after Sirius, Imperator and Braton's chassis build,
        // so Sweeper goes short. Credits run out after Sirius and Imperator,
        // so Braton goes short.
        let stock: HashMap<String, i64> = [
            (SIRIUS_BP, 1), (BRATON_BP, 1), (CHASSIS_BP, 1), (IMPERATOR_BP, 1), (FERRITE, 250), (CREDITS_PATH, 35_000),
        ].into_iter().map(|(path, n)| (path.to_string(), n)).collect();
        let observed_stock = Observed { stock: &stock, ..observed_gear(&owned, &levels, &[], &recipes, &offers, Some(30)) };
        let overview = with_suggestions(&catalog(), &corrections(), Some(&progress), &HashSet::new(), &observed_stock);

        // Craftable targets draw first, so the rows read top to bottom as the
        // ledger ran.
        assert_eq!(summary(&overview.opportunities), [
            ("Sirius & Orion", Action::Craft, Some(6_000), Access::Available),
            ("Imperator", Action::Craft, Some(3_000), Access::Available),
            ("Braton", Action::Build, Some(3_000), Access::Blocked),
            ("Sweeper", Action::Buy, Some(3_000), Access::Unknown),
        ]);
        assert!(overview.opportunities.iter().all(|o| o.stage == Stage::Craft));
        let plan = |i: usize| overview.opportunities[i].craft.as_ref().expect("recipe rows carry a plan");
        assert_eq!((plan(0).credits, plan(0).credits_short), (Some(15_000), 0));
        let braton = plan(2);
        assert_eq!(braton.builds.iter().map(|b| (b.name.as_str(), b.crafts)).collect::<Vec<_>>(), [("Chassis", 1)]);
        assert_eq!((braton.credits, braton.credits_short), (Some(20_000), 15_000));
        assert_eq!(overview.opportunities[2].blockers, ["Needs 15,000 more credits"]);
        let sweeper = plan(3);
        assert_eq!(sweeper.shortages().map(|r| (r.name.as_str(), r.short)).collect::<Vec<_>>(), [("Blueprint", 1), ("Chassis Blueprint", 1), ("Ferrite", 150)]);
        assert_eq!(overview.opportunities[3].blockers, ["Standing not observed"]);

        // Without the vendor the same shortage becomes a farm. A craft with no
        // credit balance observed cannot promise it is affordable.
        let mut stock = stock;
        stock.remove(CREDITS_PATH);
        let no_offers = HashMap::new();
        let observed_stock = Observed { stock: &stock, ..observed_gear(&owned, &levels, &[], &recipes, &no_offers, Some(30)) };
        let overview = with_suggestions(&catalog(), &corrections(), Some(&progress), &HashSet::new(), &observed_stock);
        assert_eq!(summary(&overview.opportunities), [
            ("Sirius & Orion", Action::Craft, Some(6_000), Access::Unknown),
            ("Imperator", Action::Craft, Some(3_000), Access::Unknown),
            ("Braton", Action::Build, Some(3_000), Access::Unknown),
            ("Sweeper", Action::Farm, Some(3_000), Access::Unknown),
        ]);
        assert_eq!(overview.opportunities[0].blockers, ["Credits not observed"]);
        assert_eq!(overview.opportunities[3].blockers, ["Drop sources unknown"]);
    }

    #[test]
    fn relic_farms_draw_last_land_in_acquire_and_carry_their_chance() {
        use crate::mastery_relics::Coverage;
        use crate::wfcd::RelicReward;
        const SIRIUS_BP: &str = "/Lotus/Types/Recipes/WarframeRecipes/SiriusOrionBlueprint";
        const BRATON_BP: &str = "/Lotus/Types/Recipes/Weapons/BratonBlueprint";
        const LITH: &str = "/Lotus/Types/Game/Projections/T1VoidProjectionGBronze";
        let owned = HashMap::new();
        let levels = HashMap::new();
        let (sirius, mut components) = recipe(SIRIUS, SIRIUS_BP);
        components.retain(|c| c.unique_name != CHASSIS);
        let recipes: HashMap<String, Vec<RecipeComponent>> = [(sirius, components), recipe(BRATON, BRATON_BP)].into();
        let offers = HashMap::new();
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[]);
        let reward = |unique_name: &str, chance: f64| RelicReward { unique_name: unique_name.into(), name: String::new(), rarity: String::new(), image_name: None, chance: Some(chance) };
        let tables: HashMap<String, Vec<RelicReward>> = [
            (LITH.to_string(), vec![reward(SIRIUS_BP, 25.0), reward("/Lotus/Upgrades/Mods/Immortal/ImmortalOneMod", 75.0)]),
        ].into();
        let names: HashMap<String, String> = [(LITH.to_string(), "Lith A1 Intact".to_string())].into();
        // Sixty Ferrite covers neither target. Sirius outranks Braton on
        // mastery, but as a relic farm it draws after Braton's plain farm.
        let stock: HashMap<String, i64> = [(LITH, 2), (FERRITE, 60), (CREDITS_PATH, 100_000)]
            .into_iter().map(|(path, n)| (path.to_string(), n)).collect();
        let relics = Relics::new(&stock, &tables, &names);
        let observed_stock = Observed { stock: &stock, relics: &relics, ..observed_gear(&owned, &levels, &[], &recipes, &offers, Some(30)) };
        let overview = with_suggestions(&catalog(), &corrections(), Some(&progress), &HashSet::new(), &observed_stock);
        assert_eq!(summary(&overview.opportunities), [
            ("Braton", Action::Farm, Some(3_000), Access::Unknown),
            ("Sirius & Orion", Action::Farm, Some(6_000), Access::Unknown),
        ]);
        assert_eq!(overview.opportunities.iter().map(|o| o.stage).collect::<Vec<_>>(), [Stage::Craft, Stage::Acquire]);
        let braton = &overview.opportunities[0];
        let sirius = &overview.opportunities[1];
        let ferrite = |o: &Opportunity| o.craft.as_ref().expect("recipe rows carry a plan").requirements.iter().find(|r| r.unique_name == FERRITE).map(|r| (r.from_stock, r.short));
        assert_eq!((ferrite(braton), ferrite(sirius)), (Some((60, 90)), Some((0, 50))));
        assert!(braton.relic.is_none());
        let route = sirius.relic.as_ref().expect("a relic part is short");
        assert_eq!(route.parts.iter().map(|p| (p.name.as_str(), p.needed, p.relics.len())).collect::<Vec<_>>(), [("Blueprint", 1, 1)]);
        let Coverage::Complete { probability } = route.coverage else { panic!("every part has a relic: {:?}", route.coverage) };
        assert!((probability - (1.0 - 0.75 * 0.75)).abs() < 1e-12, "{probability}");
        // The Ferrite short beyond the relic parts keeps the drop sources unknown.
        assert_eq!(sirius.blockers, ["Drop sources unknown"]);

        // With Ferrite in hand the relics are Sirius's only shortage and the
        // row is available. Without any relic the route still exists and
        // carries no chance.
        let mut stock = stock;
        stock.insert(FERRITE.into(), 1_000);
        stock.remove(LITH);
        let relics = Relics::new(&stock, &tables, &names);
        let observed_stock = Observed { stock: &stock, relics: &relics, ..observed_gear(&owned, &levels, &[], &recipes, &offers, Some(30)) };
        let overview = with_suggestions(&catalog(), &corrections(), Some(&progress), &HashSet::new(), &observed_stock);
        let sirius = overview.opportunities.iter().find(|o| o.source.name == "Sirius & Orion").expect("listed");
        assert_eq!((sirius.stage, sirius.access, sirius.blockers.clone()), (Stage::Acquire, Access::Available, vec![]));
        assert_eq!(sirius.relic.as_ref().map(|r| r.coverage.clone()), Some(Coverage::Partial { missing: vec!["Blueprint".into()], short: vec![] }));
    }

    #[test]
    fn an_unpriced_recipe_and_a_mastery_lock_show_on_the_craft_row() {
        const BRATON_BP: &str = "/Lotus/Types/Recipes/Weapons/BratonBlueprint";
        let owned = HashMap::new();
        let levels = HashMap::new();
        let (path, mut components) = recipe(BRATON, BRATON_BP);
        components[0].credits = None;
        let recipes: HashMap<String, Vec<RecipeComponent>> = [(path, components)].into();
        let offers = HashMap::new();
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[]);
        let stock: HashMap<String, i64> = [(BRATON_BP, 1), (CHASSIS, 1), (FERRITE, 50), (CREDITS_PATH, 0)]
            .into_iter().map(|(path, n)| (path.to_string(), n)).collect();
        let observed_stock = Observed { stock: &stock, ..observed_gear(&owned, &levels, &[], &recipes, &offers, Some(1)) };
        let overview = with_suggestions(&catalog(), &corrections(), Some(&progress), &HashSet::new(), &observed_stock);
        assert_eq!(summary(&overview.opportunities), [("Braton", Action::Craft, Some(3_000), Access::Blocked)]);
        assert_eq!(overview.opportunities[0].blockers, ["Requires MR 2", "Credit cost unknown"]);
    }
}
