use std::collections::{HashMap, HashSet};
use std::path::Path;
use tauri::Manager;
use crate::app_state::{AppState, CorrectionEntry};
use crate::catalogue::fix_category;
use crate::inventory_state::{inventory_path_aliases, load_inventory_state_cache, CREDITS_PATH};
use crate::mastery_nodes;
use crate::mastery_progress::{MasteryPlan, PlayerProgress, Provenance, ProvenanceState};
use crate::mastery_recipe::{blueprint_results, purchasable, CraftPlan, Ledger};
use crate::mastery_relics::{Relics, RelicRoute};
use crate::mastery_rules::{self, Unobtainable};
use crate::monitor::CraftingJob;
use crate::resolver::slug_variants;
use crate::settings::read_settings_map;
use crate::wfcd::{DropLocation, RecipeComponent, SyndicateOffer, WfcdItem};
use crate::wfm::{to_wfm_slug, PriceQuote};

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
/// Trade is a whole item only players sell, so it belongs to the platinum
/// view alone.
#[derive(serde::Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Action { Level, Claim, Spend, Craft, Build, Buy, Farm, Trade, Complete, Unlock }

/// The variant order runs by severity so `max` keeps a blocker over an
/// open question.
#[derive(serde::Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Access { Available, Unknown, Blocked }

/// Labels live in the frontend, so a wording change never reaches the wire.
#[derive(serde::Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Blocker {
    StillBuilding,
    MasteryRankBelow { required: u32 },
    MasteryRankNotObserved,
    CreditsShort { short: u64 },
    CreditCostUnknown,
    CreditsNotObserved,
    StandingNotObserved,
    DropSourcesUnknown,
    MissingGate { path: String, name: String },
    JunctionTasksNotObserved,
    NodeUnlockNotObserved,
}

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

/// A warframe.market listing. A price with a fetch time is a quote of that
/// age; a price without one came from a cache written before quotes were
/// stamped; no price with a fetch time means the market does not list the
/// slug; neither means nobody has asked yet.
#[derive(serde::Serialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Listing {
    pub(crate) slug: String,
    pub(crate) name: String,
    pub(crate) price: Option<u32>,
    pub(crate) fetched_at: Option<i64>,
}

#[derive(serde::Serialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct PartListing {
    pub(crate) unique_name: String,
    #[serde(flatten)]
    pub(crate) listing: Listing,
    /// The whole recipe takes this many, owned or not.
    pub(crate) needed: u32,
    /// The finish is still short this many after projected stock.
    pub(crate) short: u32,
}

#[derive(serde::Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Route { Parts, Set }

#[derive(serde::Serialize, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Cost {
    pub(crate) platinum: u32,
    pub(crate) route: Route,
}

/// What platinum buys toward one source. Each total is None while any part
/// it sums lacks a price, so a partial sum never reads as the cost.
#[derive(serde::Serialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Purchase {
    pub(crate) parts: Vec<PartListing>,
    /// Holds the complete set, or the item itself where nothing is crafted.
    pub(crate) set: Option<Listing>,
    pub(crate) missing_total: Option<u32>,
    pub(crate) full_total: Option<u32>,
    /// The cheaper of the missing parts and the set. A tie goes to the set,
    /// since it is one trade.
    pub(crate) cheapest_finish: Option<Cost>,
    pub(crate) full_purchase: Option<Cost>,
}

#[derive(serde::Serialize, Clone, PartialEq, Debug)]
pub(crate) struct DropPart {
    pub(crate) unique_name: String,
    pub(crate) name: String,
    pub(crate) needed: u32,
    /// Sorted with the best chance first and cut at `DROP_LOCATIONS_SHOWN`.
    pub(crate) locations: Vec<DropLocation>,
}

/// Drop locations for the shortages the relic route leaves out. Each
/// location is a plain label with its own chance, and nothing estimates the
/// whole item. A resource such as Orokin Cell lists over a hundred rotations,
/// so a part keeps only its best few.
#[derive(serde::Serialize, Clone, PartialEq, Debug)]
pub(crate) struct DropRoute {
    pub(crate) parts: Vec<DropPart>,
}

const DROP_LOCATIONS_SHOWN: usize = 5;

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
    pub(crate) blockers: Vec<Blocker>,
    /// A source that is neither owned nor building carries its plan whenever
    /// a recipe exists.
    pub(crate) craft: Option<CraftPlan>,
    /// Present when something short in the plan drops from a relic. The
    /// chance covers those parts only, and the other shortages stay in
    /// `craft`.
    pub(crate) relic: Option<RelicRoute>,
    /// Present when a shortage outside the relic route has a known drop
    /// location.
    pub(crate) drop: Option<DropRoute>,
    pub(crate) purchase: Option<Purchase>,
}

#[derive(serde::Serialize, Clone, Debug)]
pub(crate) struct MasteryOverview {
    pub(crate) counts: MasteryCounts,
    pub(crate) categories: Vec<MasteryCategory>,
    pub(crate) provenance: MasteryProvenance,
    pub(crate) mastery_rank: Option<u32>,
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
    /// Holds every non-relic drop location by item or component `unique_name`.
    pub(crate) drops: &'a HashMap<String, Vec<DropLocation>>,
    pub(crate) tradeable: &'a HashSet<String>,
    /// Holds every quote by slug, expired ones included.
    pub(crate) quotes: &'a HashMap<String, PriceQuote>,
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
    with_observed(state, |mut overview, observed| {
        if let Some(observed) = observed { overview.opportunities = suggest(&overview, observed); }
        overview
    })
}

#[tauri::command]
pub(crate) async fn evaluate_mastery_plan(app: tauri::AppHandle, plan: MasteryPlan) -> Result<PlanEvaluation, String> {
    tauri::async_runtime::spawn_blocking(move || {
        with_observed(&app.state::<AppState>(), |overview, observed| evaluate(&overview, observed, &plan))
    }).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn load_mastery_plan(state: tauri::State<'_, AppState>) -> Option<MasteryPlan> {
    let player = state.local_player_name.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let progress = state.mastery_progress.lock().unwrap_or_else(|e| e.into_inner());
    progress.current(player.as_deref()).and_then(|record| record.plan.clone())
}

#[tauri::command]
pub(crate) fn save_mastery_plan(state: tauri::State<'_, AppState>, plan: MasteryPlan) {
    let player = state.local_player_name.lock().unwrap_or_else(|e| e.into_inner()).clone();
    state.mastery_progress.lock().unwrap_or_else(|e| e.into_inner()).set_plan(player.as_deref(), plan);
}

/// Builds the overview and, once an inventory scan says what the player
/// owns, everything a row is planned against. Without a scan there are no
/// suggestions to make, so the closure sees no `Observed`. A scan another
/// account wrote is treated as no scan. The mastery record is kept per
/// player and the inventory cache is not, so after a player switch the
/// previous account's copies would otherwise stay listed until the next
/// full pass.
fn with_observed<R>(state: &AppState, f: impl FnOnce(MasteryOverview, Option<&Observed>) -> R) -> R {
    let player = state.local_player_name.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let excluded = excluded_classes(&state.settings_path);
    let (mut overview, skills, relic_names, tradeable, owner) = {
        let progress = state.mastery_progress.lock().unwrap_or_else(|e| e.into_inner());
        let items = state.wfcd_items.lock().unwrap_or_else(|e| e.into_inner());
        let owner = progress.owner(player.as_deref());
        let record = progress.current(player.as_deref());
        let skills = record.filter(|r| r.intrinsics.state != ProvenanceState::Unknown).map(|r| r.skills.clone());
        let relic_names: HashMap<String, String> = items.iter()
            .filter(|i| i.category == "Relics")
            .map(|i| (i.unique_name.clone(), i.name.clone()))
            .collect();
        (build_mastery_overview(&items, &state.corrections, record, &excluded), skills, relic_names, market_items(&items), owner)
    };
    let inventory = load_inventory_state_cache(&state.inventory_state_cache_path);
    if inventory.items.is_empty() || !owner.trusts_inventory(inventory.player.as_deref()) { return f(overview, None); }
    overview.mastery_rank = inventory.mastery_rank;
    let stock = inventory.stackable_quantities();
    let relics = {
        let tables = state.relic_rewards.lock().unwrap_or_else(|e| e.into_inner());
        Relics::new(&stock, &tables, &relic_names)
    };
    let crafting = state.current_crafting.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let recipes = state.recipes.lock().unwrap_or_else(|e| e.into_inner());
    let offers = state.syndicate_catalog.lock().unwrap_or_else(|e| e.into_inner());
    let drops = state.drop_locations.lock().unwrap_or_else(|e| e.into_inner());
    f(overview, Some(&Observed {
        owned: &inventory.unique_quantities(),
        stock: &stock,
        owned_levels: &inventory.owned_levels(),
        mastery_rank: inventory.mastery_rank,
        crafting: &crafting,
        recipes: &recipes,
        offers: &offers,
        skills: skills.as_ref(),
        relics: &relics,
        drops: &drops,
        tradeable: &tradeable,
        quotes: &state.wfm.quotes(),
        now_ms: chrono::Utc::now().timestamp_millis(),
    }))
}

/// Prime parts carry a ducat value; the catalogue's `tradable` flag covers
/// whole items players sell, such as Baro and syndicate weapons.
fn market_items(items: &[WfcdItem]) -> HashSet<String> {
    items.iter()
        .filter(|i| i.ducats.is_some() || i.tradable == Some(true))
        .map(|i| i.unique_name.clone())
        .collect()
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
        Some(gate) => (Access::Blocked, vec![Blocker::MissingGate { path: gate.unique_name.clone(), name: gate.name.clone() }]),
        None if node.junction => (Access::Unknown, vec![Blocker::JunctionTasksNotObserved]),
        None => (Access::Unknown, vec![Blocker::NodeUnlockNotObserved]),
    };
    Some(Opportunity {
        source: source.clone(), stage: Stage::Acquire,
        action: if node.junction { Action::Unlock } else { Action::Complete },
        owned: false, owned_level: None, build_completion_ms: None, vendors: vec![], spend: None, access, blockers, craft: None, relic: None, drop: None, purchase: None,
    })
}

fn sources_by_name(overview: &MasteryOverview) -> HashMap<&str, &MasterySource> {
    overview.categories.iter().flat_map(|c| &c.sources).map(|s| (s.unique_name.as_str(), s)).collect()
}

fn mastered_sources(overview: &MasteryOverview) -> HashSet<&str> {
    overview.categories.iter().flat_map(|c| &c.sources)
        .filter(|s| s.state == MasteryState::Mastered).map(|s| s.unique_name.as_str()).collect()
}

/// Builds the row for one source. A recipe row comes out with `Action::Farm`
/// as a placeholder and no plan until `settle` runs it against a ledger.
struct Rows<'a> {
    by_name: &'a HashMap<&'a str, &'a MasterySource>,
    building: HashMap<&'a str, i64>,
}

impl<'a> Rows<'a> {
    fn new(by_name: &'a HashMap<&'a str, &'a MasterySource>, observed: &Observed<'a>) -> Self {
        let blueprint_results = blueprint_results(observed.recipes);
        let building = observed.crafting.iter()
            .map(|job| (blueprint_results.get(job.unique_name.as_str()).copied().unwrap_or(&job.unique_name), job.completion_ms))
            .collect();
        Self { by_name, building }
    }

    fn row(&self, source: &MasterySource, observed: &Observed) -> Option<Opportunity> {
        if let Some(node) = &source.node { return node_opportunity(source, node, self.by_name); }
        if let Some(system) = mastery_rules::intrinsic_system(&source.unique_name) {
            let spend = plan_spend(system, observed.skills?)?;
            return Some(Opportunity {
                source: source.clone(), stage: Stage::LevelClaim, action: Action::Spend,
                owned: false, owned_level: None, build_completion_ms: None, vendors: vec![], spend: Some(spend),
                access: Access::Available, blockers: vec![], craft: None, relic: None, drop: None, purchase: None,
            });
        }
        let owned_level = observed.owned_levels.get(&source.unique_name).and_then(|levels| levels.iter().max().copied());
        let owned = owned_level.is_some() || observed.owned.get(&source.unique_name).is_some_and(|&copies| copies > 0);
        let build_completion_ms = self.building.get(source.unique_name.as_str()).copied();
        let vendors = vendor_offers(&source.unique_name, observed.offers);
        let action = if owned { Action::Level }
            else if build_completion_ms.is_some() { Action::Claim }
            else if observed.recipes.contains_key(&source.unique_name) { Action::Farm }
            else if !vendors.is_empty() { Action::Buy }
            else if observed.tradeable.contains(&source.unique_name) { Action::Trade }
            else { return None };
        Some(Opportunity {
            source: source.clone(), stage: Stage::Acquire, action, owned, owned_level, build_completion_ms, vendors, spend: None,
            access: Access::Available, blockers: vec![], craft: None, relic: None, drop: None, purchase: None,
        })
    }
}

fn settle<'a>(o: &mut Opportunity, ledger: &mut Ledger<'a>, observed: &Observed<'a>) {
    if o.action == Action::Farm && o.craft.is_none() {
        let plan = ledger.plan(&o.source.unique_name, &observed.recipes[&o.source.unique_name]);
        o.action = if plan.craftable_now() { Action::Craft }
            else if plan.buildable() { Action::Build }
            else if !o.vendors.is_empty() { Action::Buy }
            else { Action::Farm };
        o.relic = observed.relics.route(&plan);
        o.drop = drop_route(&plan, o.relic.as_ref(), observed.drops);
        o.craft = Some(plan);
    }
    if matches!(o.action, Action::Spend | Action::Complete | Action::Unlock) { return; }
    o.stage = match o.action {
        Action::Level | Action::Claim => Stage::LevelClaim,
        _ if o.craft.is_some() && o.relic.is_none() => Stage::Craft,
        _ => Stage::Acquire,
    };
    (o.access, o.blockers) = access(o, observed);
    o.purchase = purchase(o, observed);
}

pub(crate) fn suggest(overview: &MasteryOverview, observed: &Observed) -> Vec<Opportunity> {
    let by_name = sources_by_name(overview);
    let rows = Rows::new(&by_name, observed);
    let mastered = mastered_sources(overview);
    // Suggestions never spend mastered equipment, so they carry no allowances.
    let (mut ledger, _) = Ledger::new(observed.stock, observed.owned, &mastered, &HashMap::new());

    let mut opportunities: Vec<Opportunity> = overview.categories.iter().flat_map(|c| &c.sources)
        .filter(|s| !s.excluded && s.state != MasteryState::Mastered)
        .filter_map(|source| rows.row(source, observed))
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
        settle(&mut opportunities[i], &mut ledger, observed);
    }
    for o in opportunities.iter_mut().filter(|o| o.craft.is_none()) {
        settle(o, &mut ledger, observed);
    }
    // Ledger order only shows in the Craft stage. Relic farms drew last, and
    // in Acquire they sort by mastery like every other row.
    let craft_order = |a: &Opportunity, b: &Opportunity| if a.stage == Stage::Craft { rank(a).cmp(&rank(b)) } else { std::cmp::Ordering::Equal };
    opportunities.sort_by(|a, b| a.stage.cmp(&b.stage).then_with(|| craft_order(a, b)).then_with(|| by_mastery_then_name(a, b)));
    opportunities
}

/// A shortage the plan could cover with mastered equipment the player owns,
/// once an allowance names how many copies it may spend.
#[derive(serde::Serialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Allowable {
    pub(crate) unique_name: String,
    pub(crate) name: String,
    pub(crate) owned: u32,
}

#[derive(serde::Serialize, Clone, Debug)]
pub(crate) struct PlanEntry {
    pub(crate) unique_name: String,
    /// Absent when the catalogue no longer lists the path.
    pub(crate) source: Option<MasterySource>,
    /// The row as Suggestions would show it, planned in plan order against
    /// the plan's own ledger. Absent once the source is mastered, when the
    /// path repeats an earlier selection, and when nothing suggests it.
    pub(crate) opportunity: Option<Opportunity>,
    pub(crate) completed: bool,
    /// What finishing the entry adds to the projection. A repeat and a
    /// completed entry add zero.
    pub(crate) gain: Option<u32>,
    pub(crate) notes: Vec<String>,
    pub(crate) allowable: Vec<Allowable>,
}

/// A total the observed Mastery Rank bounds to the range between two
/// thresholds. `exact` is the summed earned mastery, present only when every
/// source kind is Confirmed, every source's credit is known and the sum lands
/// on the observed rank. `rank` follows `exact` where there is one and the
/// lower bound otherwise.
#[derive(serde::Serialize, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct MasteryTotal {
    pub(crate) lower: u64,
    pub(crate) upper: u64,
    pub(crate) exact: Option<u64>,
    pub(crate) rank: u32,
    pub(crate) rank_upper: u32,
}

#[derive(serde::Serialize, Clone, Debug)]
pub(crate) struct PlanEvaluation {
    pub(crate) entries: Vec<PlanEntry>,
    /// Absent until the Mastery Rank is observed, as are `gap` and `projected`.
    pub(crate) total: Option<MasteryTotal>,
    pub(crate) target_xp: u64,
    /// Mastery still to earn for the target, from the exact total or the
    /// lower bound.
    pub(crate) gap: Option<u64>,
    /// The known gains of every pending entry.
    pub(crate) gains: u64,
    /// Pending entries whose gain is unknown and so outside `gains`.
    pub(crate) unknown_gains: u32,
    pub(crate) projected: Option<MasteryTotal>,
    pub(crate) rejected_allowances: Vec<String>,
}

impl MasterySource {
    /// The credit the account holds from this source. A cleared node's is
    /// unknown, since the table carries no per-node amount.
    // TODO: carry per-node amounts in the node table. Until then an account
    // with a cleared node never gets an exact total.
    fn earned_mastery(&self) -> Option<u64> {
        let rank = u64::from(self.earned_rank?);
        match &self.node {
            Some(node) if !node.junction => (rank == 0).then_some(0),
            Some(_) => Some(rank * u64::from(mastery_nodes::JUNCTION_MASTERY)),
            None => Some(rank * u64::from(mastery_rules::mastery_per_rank(&self.unique_name))),
        }
    }
}

/// Excluded sources count as well, since a Founders item's credit stays on
/// the account whatever the settings hide.
fn earned_sum(overview: &MasteryOverview) -> Option<u64> {
    let p = &overview.provenance;
    if [p.equipment, p.intrinsics, p.nodes, p.junctions].iter().any(|kind| kind.state != ProvenanceState::Confirmed) { return None; }
    overview.categories.iter().flat_map(|c| &c.sources).map(MasterySource::earned_mastery).sum()
}

fn current_total(overview: &MasteryOverview, mastery_rank: Option<u32>) -> Option<MasteryTotal> {
    let rank = mastery_rank?;
    let exact = earned_sum(overview).filter(|&sum| {
        let derived = mastery_rules::mastery_rank_from_xp(sum);
        if derived != rank {
            tracing::warn!(sum, derived, observed = rank, "summed earned mastery does not land on the observed Mastery Rank; keeping the range");
        }
        derived == rank
    });
    Some(MasteryTotal {
        lower: mastery_rules::mastery_rank_xp(rank),
        upper: mastery_rules::mastery_rank_xp(rank + 1) - 1,
        exact,
        rank,
        rank_upper: rank,
    })
}

fn allowable(o: &Opportunity, observed: &Observed, mastered: &HashSet<&str>) -> Vec<Allowable> {
    o.craft.iter().flat_map(CraftPlan::shortages)
        .filter(|r| mastered.contains(r.unique_name.as_str()))
        .filter_map(|r| {
            let owned = observed.owned.get(&r.unique_name).copied().filter(|&n| n > 0)?;
            Some(Allowable { unique_name: r.unique_name.clone(), name: r.name.clone(), owned: u32::try_from(owned).unwrap_or(u32::MAX) })
        })
        .collect()
}

/// Runs the plan's selections in their order against one ledger, so an
/// earlier target takes stock first and a later one shows the shortage.
/// Without an inventory the entries carry their remaining mastery and no row.
// TODO: put a planned set purchase's parts into the ledger. Today a later
// target short those parts still shows the shortage.
pub(crate) fn evaluate(overview: &MasteryOverview, observed: Option<&Observed>, plan: &MasteryPlan) -> PlanEvaluation {
    let by_name = sources_by_name(overview);
    let mastered = mastered_sources(overview);
    let rows = observed.map(|observed| Rows::new(&by_name, observed));
    let (mut ledger, rejected_allowances) = match observed {
        Some(observed) => { let (ledger, rejected) = Ledger::new(observed.stock, observed.owned, &mastered, &plan.allowances); (Some(ledger), rejected) }
        None => (None, vec![]),
    };
    let mut seen: HashSet<&str> = HashSet::new();
    let mut entries: Vec<PlanEntry> = plan.selections.iter().map(|path| {
        let source = by_name.get(path.as_str()).copied();
        let mut entry = PlanEntry {
            unique_name: path.clone(), source: source.cloned(), opportunity: None, completed: false, gain: None, notes: vec![], allowable: vec![],
        };
        if !seen.insert(path.as_str()) {
            entry.notes.push("Already in the plan".into());
            entry.gain = Some(0);
            return entry;
        }
        let Some(source) = source else { entry.notes.push("Not a mastery source".into()); entry.gain = Some(0); return entry };
        if source.state == MasteryState::Mastered {
            entry.completed = true;
            entry.gain = Some(0);
            return entry;
        }
        if source.excluded { entry.notes.push("Unobtainable".into()); entry.gain = Some(0); return entry; }
        entry.gain = source.remaining_mastery;
        match (&rows, observed, ledger.as_mut()) {
            (Some(rows), Some(observed), Some(ledger)) => match rows.row(source, observed) {
                Some(mut o) => {
                    settle(&mut o, ledger, observed);
                    entry.gain = gain(&o);
                    entry.allowable = allowable(&o, observed, &mastered);
                    entry.opportunity = Some(o);
                }
                None => entry.notes.push("No known route".into()),
            },
            _ => {}
        }
        entry
    }).collect();

    // The chances are never combined across targets, so a relic two targets
    // roll gets a note on the later one.
    let mut first_holder: HashMap<String, String> = HashMap::new();
    for i in 0..entries.len() {
        let Some(o) = &entries[i].opportunity else { continue };
        let Some(route) = &o.relic else { continue };
        let name = o.source.name.clone();
        let mut shared = vec![];
        for relic in route.parts.iter().flat_map(|p| &p.relics) {
            match first_holder.get(&relic.name) {
                Some(holder) => shared.push(format!("Shares {relic} with {holder}", relic = relic.name)),
                None => { first_holder.insert(relic.name.clone(), name.clone()); }
            }
        }
        entries[i].notes.extend(shared);
    }

    let total = current_total(overview, observed.and_then(|o| o.mastery_rank));
    let target_xp = mastery_rules::mastery_rank_xp(plan.target);
    let base = total.map(|t| t.exact.unwrap_or(t.lower));
    let pending = entries.iter().filter(|e| !e.completed);
    let gains: u64 = pending.clone().filter_map(|e| e.gain).map(u64::from).sum();
    let unknown_gains = pending.filter(|e| e.gain.is_none()).count() as u32;
    let projected = total.map(|t| MasteryTotal {
        lower: t.lower + gains,
        upper: t.upper + gains,
        exact: t.exact.map(|exact| exact + gains),
        rank: mastery_rules::mastery_rank_from_xp(t.exact.unwrap_or(t.lower) + gains),
        rank_upper: mastery_rules::mastery_rank_from_xp(t.upper + gains),
    });
    PlanEvaluation {
        entries, total, target_xp,
        gap: base.map(|base| target_xp.saturating_sub(base)),
        gains, unknown_gains, projected, rejected_allowances,
    }
}

/// The catalogue names a prime part blueprint with the suffix the market
/// drops, so a quote can sit under either spelling.
fn listing(slug: String, name: String, quotes: &HashMap<String, PriceQuote>) -> Listing {
    let quote = slug_variants(&slug).iter().find_map(|s| quotes.get(s)).copied().unwrap_or(PriceQuote { price: None, fetched_at: None });
    Listing { slug, name, price: quote.price, fetched_at: quote.fetched_at }
}

/// A source is a platinum candidate when the market sells a part it is
/// still short of, or the whole item where no part is. Craft and Build
/// already have everything in stock, so there is nothing to buy for them.
fn purchase(o: &Opportunity, observed: &Observed) -> Option<Purchase> {
    if !matches!(o.action, Action::Buy | Action::Farm | Action::Trade) { return None; }
    let source = &o.source;
    let tradeable = |path: &str| observed.tradeable.contains(path);
    let recipe = observed.recipes.get(&source.unique_name);
    let requirements = o.craft.as_ref().map(|plan| plan.requirements.as_slice()).unwrap_or_default();
    let parts: Vec<PartListing> = recipe.map(|components| purchasable(&source.unique_name, components, &tradeable)).unwrap_or_default()
        .into_iter()
        .map(|part| PartListing {
            listing: listing(to_wfm_slug(&part.name), part.name, observed.quotes),
            needed: part.needed,
            short: requirements.iter().find(|r| r.unique_name == part.unique_name).map_or(0, |r| r.short),
            unique_name: part.unique_name,
        })
        .collect();
    let slug = to_wfm_slug(&source.name);
    let set = match recipe {
        Some(_) if !parts.is_empty() || tradeable(&source.unique_name) => Some(listing(format!("{slug}_set"), format!("{} Set", source.name), observed.quotes)),
        None if tradeable(&source.unique_name) => Some(listing(slug, source.name.clone(), observed.quotes)),
        _ => None,
    };
    let short = parts.iter().any(|p| p.short > 0);
    if !short && !(parts.is_empty() && set.is_some()) { return None; }

    let total = |count: fn(&PartListing) -> u32| -> Option<u32> {
        parts.iter().filter(|p| count(p) > 0).map(|p| p.listing.price.map(|price| price * count(p))).sum()
    };
    let missing_total = if short { total(|p| p.short) } else { None };
    let full_total = if parts.is_empty() { None } else { total(|p| p.needed) };
    let set_price = set.as_ref().and_then(|s| s.price);
    let cheaper = |parts: Option<u32>| match (parts, set_price) {
        (Some(parts), Some(set)) if parts < set => Some(Cost { platinum: parts, route: Route::Parts }),
        (Some(parts), None) => Some(Cost { platinum: parts, route: Route::Parts }),
        (_, Some(set)) => Some(Cost { platinum: set, route: Route::Set }),
        (None, None) => None,
    };
    Some(Purchase {
        cheapest_finish: cheaper(missing_total),
        full_purchase: cheaper(full_total),
        parts, set, missing_total, full_total,
    })
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

fn drop_route(plan: &CraftPlan, relic: Option<&RelicRoute>, drops: &HashMap<String, Vec<DropLocation>>) -> Option<DropRoute> {
    let parts: Vec<DropPart> = plan.shortages()
        .filter(|r| !relic.is_some_and(|route| route.parts.iter().any(|p| p.unique_name == r.unique_name)))
        .filter_map(|r| {
            let mut locations = drops.get(&r.unique_name)?.clone();
            locations.sort_by(|a, b| b.chance.partial_cmp(&a.chance).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.location.cmp(&b.location)));
            locations.truncate(DROP_LOCATIONS_SHOWN);
            Some(DropPart { unique_name: r.unique_name.clone(), name: r.name.clone(), needed: r.short, locations })
        })
        .collect();
    if parts.is_empty() { return None; }
    Some(DropRoute { parts })
}

fn access(o: &Opportunity, observed: &Observed) -> (Access, Vec<Blocker>) {
    let mut access = Access::Available;
    let mut blockers = vec![];
    let mut note = |worst: Access, blocker: Blocker| {
        access = access.max(worst);
        blockers.push(blocker);
    };
    match o.action {
        Action::Level => {}
        Action::Spend | Action::Complete | Action::Unlock => unreachable!("settled when the row was built"),
        Action::Claim => if o.build_completion_ms.is_some_and(|done| done > observed.now_ms) {
            note(Access::Blocked, Blocker::StillBuilding);
        },
        Action::Craft | Action::Build | Action::Buy | Action::Farm | Action::Trade => {
            match (o.source.mastery_req, observed.mastery_rank) {
                (Some(required), Some(rank)) if required > rank => note(Access::Blocked, Blocker::MasteryRankBelow { required }),
                (Some(required), None) if required > 0 => note(Access::Unknown, Blocker::MasteryRankNotObserved),
                _ => {}
            }
            match o.action {
                Action::Craft | Action::Build => {
                    let plan = o.craft.as_ref().expect("a craft row carries its plan");
                    if plan.credits_short > 0 { note(Access::Blocked, Blocker::CreditsShort { short: plan.credits_short }); }
                    else if plan.credits.is_none() { note(Access::Unknown, Blocker::CreditCostUnknown); }
                    else if !observed.stock.contains_key(CREDITS_PATH) { note(Access::Unknown, Blocker::CreditsNotObserved); }
                }
                // TODO: read standing from the scan. Until then every Buy stays Unknown.
                Action::Buy => note(Access::Unknown, Blocker::StandingNotObserved),
                Action::Trade => {}
                _ => {
                    let plan = o.craft.as_ref().expect("a farm row carries its plan");
                    let located: Vec<&str> = o.relic.iter().flat_map(|r| &r.parts).map(|p| p.unique_name.as_str())
                        .chain(o.drop.iter().flat_map(|r| &r.parts).map(|p| p.unique_name.as_str()))
                        .collect();
                    if plan.shortages().any(|r| !located.contains(&r.unique_name.as_str())) {
                        note(Access::Unknown, Blocker::DropSourcesUnknown);
                    }
                }
            }
        }
    }
    (access, blockers)
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
    MasteryOverview { counts, categories, provenance, mastery_rank: None, opportunities: vec![] }
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
    static NO_MARKET: LazyLock<HashSet<String>> = LazyLock::new(HashSet::new);
    static NO_QUOTES: LazyLock<HashMap<String, PriceQuote>> = LazyLock::new(HashMap::new);
    static NO_DROPS: LazyLock<HashMap<String, Vec<DropLocation>>> = LazyLock::new(HashMap::new);

    fn observed_gear<'a>(owned: &'a HashMap<String, i64>, owned_levels: &'a HashMap<String, Vec<u32>>, crafting: &'a [CraftingJob], recipes: &'a HashMap<String, Vec<RecipeComponent>>, offers: &'a HashMap<String, Vec<SyndicateOffer>>, mastery_rank: Option<u32>) -> Observed<'a> {
        Observed { owned, stock: &NO_STOCK, owned_levels, mastery_rank, crafting, recipes, offers, skills: None, relics: &NO_RELICS, drops: &NO_DROPS, tradeable: &NO_MARKET, quotes: &NO_QUOTES, now_ms: 1_000_000 }
    }

    fn observed_skills<'a>(progress: &'a PlayerProgress, recipes: &'a HashMap<String, Vec<RecipeComponent>>, offers: &'a HashMap<String, Vec<SyndicateOffer>>, owned: &'a HashMap<String, i64>, levels: &'a HashMap<String, Vec<u32>>) -> Observed<'a> {
        Observed { skills: Some(&progress.skills), ..observed_gear(owned, levels, &[], recipes, offers, Some(30)) }
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
        assert_eq!(voidrig.blockers, [Blocker::StillBuilding]);
        assert!(overview.opportunities[2].blockers.is_empty());
        let sweeper = &overview.opportunities[5];
        assert_eq!(sweeper.vendors, [
            VendorOffer { syndicate: "Cephalon Simaris".into(), tier: "Neutral".into(), blueprint: true },
            VendorOffer { syndicate: "Steel Meridian".into(), tier: "General".into(), blueprint: false },
        ]);
        assert_eq!(sweeper.blockers, [Blocker::StandingNotObserved]);
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
        assert_eq!(locked.opportunities[0].blockers, [Blocker::MasteryRankBelow { required: 2 }, Blocker::StandingNotObserved]);
        let unranked = with_suggestions(&catalog(), &corrections(), Some(&progress), &HashSet::new(),
            &observed_gear(&owned, &levels, &[], &recipes, &offers, None));
        assert_eq!(summary(&unranked.opportunities), [("Sweeper", Action::Buy, Some(3_000), Access::Unknown)]);
        assert_eq!(unranked.opportunities[0].blockers, [Blocker::MasteryRankNotObserved, Blocker::StandingNotObserved]);
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
        assert_eq!(mercury.blockers, [Blocker::JunctionTasksNotObserved]);
        let mars = find("Mars Junction");
        assert_eq!((mars.action, mars.access), (Action::Unlock, Access::Unknown));
        let aphrodite = find("Aphrodite");
        assert_eq!((aphrodite.action, aphrodite.source.remaining_mastery, aphrodite.access), (Action::Complete, None, Access::Unknown));
        assert_eq!(aphrodite.blockers, [Blocker::NodeUnlockNotObserved]);
        let apollodorus = find("Apollodorus");
        assert_eq!((apollodorus.action, apollodorus.access), (Action::Complete, Access::Blocked));
        assert_eq!(apollodorus.blockers, [Blocker::MissingGate { path: "VenusToMercuryJunction".into(), name: "Mercury Junction".into() }]);
        let ceres = find("Ceres Junction");
        assert_eq!((ceres.access, ceres.blockers.clone()), (Access::Blocked, vec![Blocker::MissingGate { path: "EarthToMarsJunction".into(), name: "Mars Junction".into() }]));
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
        assert_eq!(overview.opportunities[2].blockers, [Blocker::CreditsShort { short: 15_000 }]);
        let sweeper = plan(3);
        assert_eq!(sweeper.shortages().map(|r| (r.name.as_str(), r.short)).collect::<Vec<_>>(), [("Blueprint", 1), ("Chassis Blueprint", 1), ("Ferrite", 150)]);
        assert_eq!(overview.opportunities[3].blockers, [Blocker::StandingNotObserved]);

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
        assert_eq!(overview.opportunities[0].blockers, [Blocker::CreditsNotObserved]);
        assert_eq!(overview.opportunities[3].blockers, [Blocker::DropSourcesUnknown]);
    }

    const BRATON_PRIME: &str = "/Lotus/Weapons/Tenno/Rifle/BratonPrime";
    const BRATON_PRIME_BP: &str = "/Lotus/Types/Recipes/Weapons/BratonPrimeBlueprint";
    const BARREL: &str = "/Lotus/Types/Recipes/Weapons/WeaponParts/BratonPrimeBarrel";
    const RECEIVER: &str = "/Lotus/Types/Recipes/Weapons/WeaponParts/BratonPrimeReceiverComponent";
    const RECEIVER_BP: &str = "/Lotus/Types/Recipes/Weapons/WeaponParts/BratonPrimeReceiver";
    const PRISMA_GORGON: &str = "/Lotus/Weapons/Grineer/LongGuns/VoidTraderGorgon/VTGorgon";
    const DERA_VANDAL: &str = "/Lotus/Weapons/ClanTech/Energy/DeraVandal";
    const DERA_VANDAL_BP: &str = "/Lotus/Types/Recipes/Weapons/DeraVandalBlueprint";
    const NOW: i64 = 1_700_000_000;

    /// The receiver blueprint sits inside the built receiver, as prime part
    /// blueprints do.
    fn braton_prime_recipe() -> (String, Vec<RecipeComponent>) {
        let component = |unique_name: &str, name: &str, credits: Option<u32>, components: Vec<RecipeComponent>| RecipeComponent {
            unique_name: unique_name.into(), name: name.into(), count: 1, result_count: 1, components, credits, reusable: false,
        };
        (BRATON_PRIME.into(), vec![
            component(BRATON_PRIME_BP, "Braton Prime Blueprint", Some(15_000), vec![]),
            component(BARREL, "Braton Prime Barrel", None, vec![]),
            component(RECEIVER, "Braton Prime Receiver", None, vec![
                component(RECEIVER_BP, "Braton Prime Receiver Blueprint", Some(5_000), vec![]),
                component(FERRITE, "Ferrite", None, vec![]),
            ]),
        ])
    }

    fn quote(price: Option<u32>, fetched_at: Option<i64>) -> PriceQuote {
        PriceQuote { price, fetched_at }
    }

    fn quotes(entries: &[(&str, PriceQuote)]) -> HashMap<String, PriceQuote> {
        entries.iter().map(|(slug, q)| ((*slug).to_string(), *q)).collect()
    }

    fn catalog_with_market() -> Vec<WfcdItem> {
        let mut items = catalog();
        items.push(item("Braton Prime", BRATON_PRIME, "Rifle", "LongGuns", "Primary", Some(true)));
        items.push(item("Prisma Gorgon", PRISMA_GORGON, "Rifle", "LongGuns", "Primary", Some(true)));
        items.push(item("Dera Vandal", DERA_VANDAL, "Rifle", "LongGuns", "Primary", Some(true)));
        items
    }

    fn purchase_of(overview: &MasteryOverview, path: &str) -> Purchase {
        overview.opportunities.iter().find(|o| o.source.unique_name == path)
            .unwrap_or_else(|| panic!("{path} suggested"))
            .purchase.clone().unwrap_or_else(|| panic!("{path} has a purchase"))
    }

    #[test]
    fn cheapest_finish_prefers_a_cheaper_set_and_full_purchase_ignores_owned_parts() {
        let owned = HashMap::new();
        let levels = HashMap::new();
        let recipes: HashMap<String, Vec<RecipeComponent>> = [braton_prime_recipe()].into();
        let offers = HashMap::new();
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[]);
        let stock: HashMap<String, i64> = [(BARREL, 1), (FERRITE, 100), (CREDITS_PATH, 50_000)]
            .into_iter().map(|(path, n)| (path.to_string(), n)).collect();
        let tradeable: HashSet<String> = [BRATON_PRIME_BP, BARREL, RECEIVER_BP].into_iter().map(String::from).collect();
        // The receiver quote is seven hours old and the barrel's age is unknown.
        // Both still price the finish, and the row says how old they are.
        let mut market = quotes(&[
            ("braton_prime_blueprint", quote(Some(25), Some(NOW - 60))),
            ("braton_prime_barrel", quote(Some(15), None)),
            ("braton_prime_receiver", quote(Some(30), Some(NOW - 7 * 3600))),
            ("braton_prime_set", quote(Some(40), Some(NOW - 120))),
        ]);
        let with_market = |market: &HashMap<String, PriceQuote>| {
            let observed_stock = Observed { stock: &stock, tradeable: &tradeable, quotes: market, ..observed_gear(&owned, &levels, &[], &recipes, &offers, Some(30)) };
            with_suggestions(&catalog_with_market(), &corrections(), Some(&progress), &HashSet::new(), &observed_stock)
        };

        let overview = with_market(&market);
        let braton = purchase_of(&overview, BRATON_PRIME);
        assert_eq!(braton.parts.iter().map(|p| (p.listing.slug.as_str(), p.needed, p.short, p.listing.price, p.listing.fetched_at)).collect::<Vec<_>>(), [
            ("braton_prime_blueprint", 1, 1, Some(25), Some(NOW - 60)),
            ("braton_prime_barrel", 1, 0, Some(15), None),
            ("braton_prime_receiver_blueprint", 1, 1, Some(30), Some(NOW - 7 * 3600)),
        ]);
        assert_eq!(braton.set.as_ref().map(|s| (s.slug.as_str(), s.name.as_str(), s.price)), Some(("braton_prime_set", "Braton Prime Set", Some(40))));
        assert_eq!((braton.missing_total, braton.full_total), (Some(55), Some(70)));
        assert_eq!(braton.cheapest_finish, Some(Cost { platinum: 40, route: Route::Set }));
        assert_eq!(braton.full_purchase, Some(Cost { platinum: 40, route: Route::Set }));

        // A dearer set loses the finish to the two missing parts but still
        // beats buying all three.
        market.insert("braton_prime_set".into(), quote(Some(60), Some(NOW)));
        let braton = purchase_of(&with_market(&market), BRATON_PRIME);
        assert_eq!(braton.cheapest_finish, Some(Cost { platinum: 55, route: Route::Parts }));
        assert_eq!(braton.full_purchase, Some(Cost { platinum: 60, route: Route::Set }));

        // A part nobody has quoted leaves the parts route unpriced, while the
        // set still prices the finish. With the set gone too there is no price.
        market.remove("braton_prime_receiver");
        let braton = purchase_of(&with_market(&market), BRATON_PRIME);
        assert_eq!((braton.missing_total, braton.full_total), (None, None));
        assert_eq!(braton.parts[2].listing, Listing { slug: "braton_prime_receiver_blueprint".into(), name: "Braton Prime Receiver Blueprint".into(), price: None, fetched_at: None });
        assert_eq!(braton.cheapest_finish, Some(Cost { platinum: 60, route: Route::Set }));
        market.insert("braton_prime_set".into(), quote(None, Some(NOW)));
        let braton = purchase_of(&with_market(&market), BRATON_PRIME);
        assert_eq!((braton.cheapest_finish, braton.full_purchase), (None, None));
        assert_eq!(braton.set.as_ref().map(|s| (s.price, s.fetched_at)), Some((None, Some(NOW))));
    }

    #[test]
    fn whole_items_trade_only_with_platinum_and_a_finished_recipe_buys_nothing() {
        let owned = HashMap::new();
        let levels = HashMap::new();
        let recipes: HashMap<String, Vec<RecipeComponent>> = [braton_prime_recipe(), recipe(DERA_VANDAL, DERA_VANDAL_BP)].into();
        let offers = HashMap::new();
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[]);
        let stock: HashMap<String, i64> = [(BRATON_PRIME_BP, 1), (BARREL, 1), (RECEIVER, 1), (CREDITS_PATH, 50_000)]
            .into_iter().map(|(path, n)| (path.to_string(), n)).collect();
        let tradeable: HashSet<String> = [BRATON_PRIME_BP, BARREL, RECEIVER_BP, PRISMA_GORGON, DERA_VANDAL].into_iter().map(String::from).collect();
        let market = quotes(&[("prisma_gorgon", quote(Some(90), Some(NOW))), ("dera_vandal_set", quote(Some(35), Some(NOW)))]);
        let observed_stock = Observed { stock: &stock, tradeable: &tradeable, quotes: &market, ..observed_gear(&owned, &levels, &[], &recipes, &offers, Some(30)) };
        let overview = with_suggestions(&catalog_with_market(), &corrections(), Some(&progress), &HashSet::new(), &observed_stock);

        // Braton Prime is craftable from stock, so platinum has nothing to buy.
        let braton = overview.opportunities.iter().find(|o| o.source.unique_name == BRATON_PRIME).expect("craft row");
        assert_eq!((braton.action, braton.purchase.is_none()), (Action::Craft, true));

        // Prisma Gorgon has no recipe and no vendor, so it becomes a Trade row
        // for the platinum view only, priced as the whole item.
        let gorgon = overview.opportunities.iter().find(|o| o.source.unique_name == PRISMA_GORGON).expect("trade row");
        assert_eq!((gorgon.action, gorgon.stage, gorgon.access), (Action::Trade, Stage::Acquire, Access::Available));
        let gorgon = gorgon.purchase.as_ref().expect("whole item purchase");
        assert!(gorgon.parts.is_empty());
        assert_eq!(gorgon.set.as_ref().map(|s| (s.slug.as_str(), s.name.as_str())), Some(("prisma_gorgon", "Prisma Gorgon")));
        assert_eq!((gorgon.missing_total, gorgon.full_total), (None, None));
        assert_eq!(gorgon.cheapest_finish, Some(Cost { platinum: 90, route: Route::Set }));

        // Dera Vandal's set is on the market while its parts are not.
        let dera = purchase_of(&overview, DERA_VANDAL);
        assert!(dera.parts.is_empty());
        assert_eq!(dera.set.as_ref().map(|s| (s.slug.as_str(), s.price)), Some(("dera_vandal_set", Some(35))));
        assert_eq!(dera.cheapest_finish, Some(Cost { platinum: 35, route: Route::Set }));

        // Sweeper has neither recipe nor vendor nor market, so it gets no row.
        assert!(!overview.opportunities.iter().any(|o| o.source.unique_name == SWEEPER));
    }

    #[test]
    fn drop_locations_route_a_farm_and_lift_the_unknown_blocker() {
        use crate::mastery_relics::Coverage;
        use crate::wfcd::RelicReward;
        const SIRIUS_BP: &str = "/Lotus/Types/Recipes/WarframeRecipes/SiriusOrionBlueprint";
        const BRATON_BP: &str = "/Lotus/Types/Recipes/Weapons/BratonBlueprint";
        const LITH: &str = "/Lotus/Types/Game/Projections/T1VoidProjectionGBronze";
        let owned = HashMap::new();
        let levels = HashMap::new();
        let recipes: HashMap<String, Vec<RecipeComponent>> = [recipe(SIRIUS, SIRIUS_BP), recipe(BRATON, BRATON_BP)].into_iter()
            .map(|(source, mut components)| { components.retain(|c| c.unique_name != CHASSIS); (source, components) })
            .collect();
        let offers = HashMap::new();
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[]);
        let stock: HashMap<String, i64> = [(FERRITE, 1_000), (CREDITS_PATH, 100_000)]
            .into_iter().map(|(path, n)| (path.to_string(), n)).collect();
        let at = |location: &str, chance: Option<f64>| DropLocation { location: location.into(), chance };
        // The seven locations arrive unsorted, and the cut at five drops the
        // 1% cache and the chanceless vendor.
        let drops: HashMap<String, Vec<DropLocation>> = [(SIRIUS_BP.to_string(), vec![
            at("Cephalon Simaris", None),
            at("Earth/Mantle (Capture)", Some(7.5)),
            at("Corrupted Vor", Some(50.0)),
            at("Venus/Orb Vallis Bounty, Rotation B", Some(33.33)),
            at("Venus/Orb Vallis Bounty, Rotation A", Some(33.33)),
            at("Hallowed Flame Mission Caches", Some(1.0)),
            at("Saturn/Titan (Survival), Rotation C", Some(12.5)),
        ])].into();
        let observed_stock = Observed { stock: &stock, drops: &drops, ..observed_gear(&owned, &levels, &[], &recipes, &offers, Some(30)) };
        let overview = with_suggestions(&catalog(), &corrections(), Some(&progress), &HashSet::new(), &observed_stock);
        let row = |name: &str| overview.opportunities.iter().find(|o| o.source.name == name).expect("listed");
        let braton = row("Braton");
        assert_eq!((braton.action, braton.access, braton.blockers.clone()), (Action::Farm, Access::Unknown, vec![Blocker::DropSourcesUnknown]));
        assert!(braton.drop.is_none());
        // A located drop leaves the row in the Craft stage, since only a relic
        // farm moves to Acquire.
        let sirius = row("Sirius & Orion");
        assert_eq!((sirius.action, sirius.stage, sirius.access, sirius.blockers.clone()), (Action::Farm, Stage::Craft, Access::Available, vec![]));
        let route = sirius.drop.as_ref().expect("the blueprint has drop locations");
        assert_eq!(route.parts.iter().map(|p| (p.unique_name.as_str(), p.name.as_str(), p.needed)).collect::<Vec<_>>(), [(SIRIUS_BP, "Blueprint", 1)]);
        assert_eq!(route.parts[0].locations.iter().map(|d| (d.location.as_str(), d.chance)).collect::<Vec<_>>(), [
            ("Corrupted Vor", Some(50.0)),
            ("Venus/Orb Vallis Bounty, Rotation A", Some(33.33)),
            ("Venus/Orb Vallis Bounty, Rotation B", Some(33.33)),
            ("Saturn/Titan (Survival), Rotation C", Some(12.5)),
            ("Earth/Mantle (Capture)", Some(7.5)),
        ]);

        // Once a relic drops the blueprint the relic route owns it, even with
        // no relic in stock, and the drop route has nothing left to list.
        let reward = |unique_name: &str, chance: f64| RelicReward { unique_name: unique_name.into(), name: String::new(), rarity: String::new(), image_name: None, chance: Some(chance) };
        let tables: HashMap<String, Vec<RelicReward>> = [(LITH.to_string(), vec![reward(SIRIUS_BP, 25.0)])].into();
        let relics = Relics::new(&stock, &tables, &HashMap::new());
        let observed_stock = Observed { stock: &stock, drops: &drops, relics: &relics, ..observed_gear(&owned, &levels, &[], &recipes, &offers, Some(30)) };
        let overview = with_suggestions(&catalog(), &corrections(), Some(&progress), &HashSet::new(), &observed_stock);
        let sirius = overview.opportunities.iter().find(|o| o.source.name == "Sirius & Orion").expect("listed");
        assert_eq!(sirius.relic.as_ref().map(|r| r.coverage.clone()), Some(Coverage::Partial { missing: vec!["Blueprint".into()], short: vec![] }));
        assert!(sirius.drop.is_none());
        assert_eq!(sirius.blockers, []);
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
        assert_eq!(sirius.blockers, [Blocker::DropSourcesUnknown]);

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

    fn plan(target: u32, selections: &[&str]) -> MasteryPlan {
        MasteryPlan { target, view: "suggestions".into(), selections: selections.iter().map(|s| (*s).to_string()).collect(), allowances: HashMap::new() }
    }

    fn entry_summary(evaluation: &PlanEvaluation) -> Vec<(&str, Option<Action>, Option<u32>, bool, Vec<String>)> {
        evaluation.entries.iter()
            .map(|e| (e.unique_name.as_str(), e.opportunity.as_ref().map(|o| o.action), e.gain, e.completed, e.notes.clone()))
            .collect()
    }

    #[test]
    fn plan_order_decides_shortages_and_a_repeat_or_finished_selection_earns_nothing() {
        const SIRIUS_BP: &str = "/Lotus/Types/Recipes/WarframeRecipes/SiriusOrionBlueprint";
        const BRATON_BP: &str = "/Lotus/Types/Recipes/Weapons/BratonBlueprint";
        let owned = HashMap::new();
        let levels = HashMap::new();
        let (sirius, mut components) = recipe(SIRIUS, SIRIUS_BP);
        components.retain(|c| c.unique_name != CHASSIS);
        let recipes: HashMap<String, Vec<RecipeComponent>> = [(sirius, components), recipe(BRATON, BRATON_BP)].into();
        let offers = HashMap::new();
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[(GRIMOIRE, 450_000)]);
        // Sixty Ferrite covers one target's fifty, and whichever plans first gets it.
        let stock: HashMap<String, i64> = [(SIRIUS_BP, 1), (BRATON_BP, 1), (CHASSIS, 1), (FERRITE, 60), (CREDITS_PATH, 100_000)]
            .into_iter().map(|(path, n)| (path.to_string(), n)).collect();
        let observed_stock = Observed { stock: &stock, ..observed_gear(&owned, &levels, &[], &recipes, &offers, Some(2)) };
        let overview = build_mastery_overview(&catalog(), &corrections(), Some(&progress), &HashSet::new());

        let evaluation = evaluate(&overview, Some(&observed_stock), &plan(4, &[SIRIUS, BRATON, SIRIUS, GRIMOIRE, "/Lotus/Weapons/Nope"]));
        assert_eq!(entry_summary(&evaluation), [
            (SIRIUS, Some(Action::Craft), Some(6_000), false, vec![]),
            (BRATON, Some(Action::Farm), Some(3_000), false, vec![]),
            (SIRIUS, None, Some(0), false, vec!["Already in the plan".into()]),
            (GRIMOIRE, None, Some(0), true, vec![]),
            ("/Lotus/Weapons/Nope", None, Some(0), false, vec!["Not a mastery source".into()]),
        ]);
        let braton = evaluation.entries[1].opportunity.as_ref().and_then(|o| o.craft.as_ref()).expect("recipe rows carry a plan");
        assert_eq!(braton.shortages().map(|r| (r.name.as_str(), r.short)).collect::<Vec<_>>(), [("Ferrite", 40)]);
        assert!(evaluation.entries[4].source.is_none());
        assert_eq!(evaluation.entries[3].source.as_ref().map(|s| s.name.as_str()), Some("Grimoire"));

        // MR 2 sits between 10,000 and 22,499. The two pending entries add
        // 9,000, which reaches MR 3 only from the top of the range.
        assert_eq!(evaluation.total, Some(MasteryTotal { lower: 10_000, upper: 22_499, exact: None, rank: 2, rank_upper: 2 }));
        assert_eq!((evaluation.target_xp, evaluation.gap, evaluation.gains, evaluation.unknown_gains), (40_000, Some(30_000), 9_000, 0));
        assert_eq!(evaluation.projected, Some(MasteryTotal { lower: 19_000, upper: 31_499, exact: None, rank: 2, rank_upper: 3 }));

        let reversed = evaluate(&overview, Some(&observed_stock), &plan(4, &[BRATON, SIRIUS]));
        assert_eq!(entry_summary(&reversed).iter().map(|(path, action, ..)| (*path, *action)).collect::<Vec<_>>(),
            [(BRATON, Some(Action::Craft)), (SIRIUS, Some(Action::Farm))]);

        // Without an inventory scan the entries keep their remaining mastery
        // and nothing is bounded.
        let unobserved = evaluate(&overview, None, &plan(4, &[SIRIUS, GRIMOIRE]));
        assert_eq!(entry_summary(&unobserved), [(SIRIUS, None, Some(6_000), false, vec![]), (GRIMOIRE, None, Some(0), true, vec![])]);
        assert_eq!((unobserved.total, unobserved.gap, unobserved.gains, unobserved.projected), (None, None, 6_000, None));
    }

    /// Worked by hand: 3,000 for the Braton, 3,500 for the rank-35 Kuva
    /// Karak, 6,000 each for Sirius and the excluded Excalibur Prime, 1,200
    /// for the rank-12 Balla, 67,500 for 45 Railjack ranks and 60,000 for
    /// 40 Drifter ranks make 147,200, which is MR 7.
    #[test]
    fn total_is_a_range_from_the_rank_and_exact_only_when_every_kind_is_confirmed_and_the_sum_lands_on_it() {
        let mut items = catalog();
        items.push(item("Excalibur Prime", EXCALIBUR_PRIME, "Warframe", "Suits", "Warframes", Some(true)));
        let mut corrections = corrections();
        corrections.insert(EXCALIBUR_PRIME.into(), CorrectionEntry { path: EXCALIBUR_PRIME.into(), unobtainable: Some(Unobtainable::Founders), ..Default::default() });
        let progress = with_skills(with_missions(observed(ProvenanceState::Confirmed, Some(1_000), &[
            (ORION, 900_000), (BRATON, 450_000), (KUVA, 612_500), (STRIKE, 72_000), (EXCALIBUR_PRIME, 900_000),
        ]), &[]), ProvenanceState::Confirmed, &CAPTURED_SKILLS);
        let full = build_mastery_overview(&items, &corrections, Some(&progress), &Unobtainable::ALL.into());
        for kind in [full.provenance.equipment, full.provenance.intrinsics, full.provenance.nodes, full.provenance.junctions] {
            assert_eq!(kind.state, ProvenanceState::Confirmed);
        }
        let (owned, levels, recipes, offers) = (HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new());
        let at = |rank| observed_gear(&owned, &levels, &[], &recipes, &offers, Some(rank));

        // Every Steel Path row is Unknown today, so the real overview never
        // sums, however much is confirmed.
        let evaluation = evaluate(&full, Some(&at(7)), &plan(8, &[]));
        assert_eq!(evaluation.total, Some(MasteryTotal { lower: 122_500, upper: 159_999, exact: None, rank: 7, rank_upper: 7 }));

        let mut known = full.clone();
        known.categories.retain(|c| c.category != "Star Chart");
        let evaluation = evaluate(&known, Some(&at(7)), &plan(8, &[]));
        assert_eq!(evaluation.total, Some(MasteryTotal { lower: 122_500, upper: 159_999, exact: Some(147_200), rank: 7, rank_upper: 7 }));
        assert_eq!((evaluation.gap, evaluation.projected.map(|p| p.exact)), (Some(12_800), Some(Some(147_200))));

        // The same sum under an observed MR 8 lands short, so the range stands.
        let evaluation = evaluate(&known, Some(&at(8)), &plan(9, &[]));
        assert_eq!(evaluation.total, Some(MasteryTotal { lower: 160_000, upper: 202_499, exact: None, rank: 8, rank_upper: 8 }));
        assert_eq!(evaluation.gap, Some(42_500));

        let mut unconfirmed = known.clone();
        unconfirmed.provenance.intrinsics.state = ProvenanceState::Unconfirmed;
        assert_eq!(evaluate(&unconfirmed, Some(&at(7)), &plan(8, &[])).total.and_then(|t| t.exact), None);
    }

    #[test]
    fn two_targets_rolling_the_same_relic_are_flagged_on_the_later_one() {
        use crate::wfcd::RelicReward;
        const SIRIUS_BP: &str = "/Lotus/Types/Recipes/WarframeRecipes/SiriusOrionBlueprint";
        const BRATON_BP: &str = "/Lotus/Types/Recipes/Weapons/BratonBlueprint";
        const LITH: &str = "/Lotus/Types/Game/Projections/T1VoidProjectionGBronze";
        let owned = HashMap::new();
        let levels = HashMap::new();
        let without_chassis = |(path, mut components): (String, Vec<RecipeComponent>)| {
            components.retain(|c| c.unique_name != CHASSIS);
            (path, components)
        };
        let recipes: HashMap<String, Vec<RecipeComponent>> = [without_chassis(recipe(SIRIUS, SIRIUS_BP)), without_chassis(recipe(BRATON, BRATON_BP))].into();
        let offers = HashMap::new();
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[]);
        let reward = |unique_name: &str, chance: f64| RelicReward { unique_name: unique_name.into(), name: String::new(), rarity: String::new(), image_name: None, chance: Some(chance) };
        let tables: HashMap<String, Vec<RelicReward>> = [(LITH.to_string(), vec![reward(SIRIUS_BP, 25.0), reward(BRATON_BP, 25.0), reward("/Lotus/Upgrades/Mods/Immortal/ImmortalOneMod", 50.0)])].into();
        let names: HashMap<String, String> = [(LITH.to_string(), "Lith A1 Intact".to_string())].into();
        let stock: HashMap<String, i64> = [(LITH, 3), (FERRITE, 1_000), (CREDITS_PATH, 100_000)]
            .into_iter().map(|(path, n)| (path.to_string(), n)).collect();
        let relics = Relics::new(&stock, &tables, &names);
        let observed_stock = Observed { stock: &stock, relics: &relics, ..observed_gear(&owned, &levels, &[], &recipes, &offers, Some(30)) };
        let overview = build_mastery_overview(&catalog(), &corrections(), Some(&progress), &HashSet::new());
        let evaluation = evaluate(&overview, Some(&observed_stock), &plan(31, &[BRATON, SIRIUS]));
        assert_eq!(entry_summary(&evaluation), [
            (BRATON, Some(Action::Farm), Some(3_000), false, vec![]),
            (SIRIUS, Some(Action::Farm), Some(6_000), false, vec!["Shares Lith A1 Intact with Braton".into()]),
        ]);
    }

    #[test]
    fn an_allowance_spends_a_mastered_owned_piece_and_is_refused_for_an_unmastered_one() {
        const KUVA_BP: &str = "/Lotus/Types/Recipes/Weapons/KuvaKarakBlueprint";
        let component = |unique_name: &str, name: &str, count: u32, credits: Option<u32>| RecipeComponent {
            unique_name: unique_name.into(), name: name.into(), count, result_count: 1, components: vec![], credits, reusable: false,
        };
        let recipes: HashMap<String, Vec<RecipeComponent>> = [(KUVA.to_string(), vec![
            component(KUVA_BP, "Blueprint", 1, Some(15_000)), component(BRATON, "Braton", 1, None), component(FERRITE, "Ferrite", 50, None),
        ])].into();
        let owned: HashMap<String, i64> = [(BRATON.to_string(), 1), (SIRIUS.to_string(), 1)].into();
        let levels = HashMap::new();
        let offers = HashMap::new();
        let progress = observed(ProvenanceState::Confirmed, Some(1_000), &[(BRATON, 450_000)]);
        let stock: HashMap<String, i64> = [(KUVA_BP, 1), (FERRITE, 100), (CREDITS_PATH, 100_000)]
            .into_iter().map(|(path, n)| (path.to_string(), n)).collect();
        let observed_stock = Observed { stock: &stock, ..observed_gear(&owned, &levels, &[], &recipes, &offers, Some(30)) };
        let overview = build_mastery_overview(&catalog(), &corrections(), Some(&progress), &HashSet::new());

        let evaluation = evaluate(&overview, Some(&observed_stock), &plan(31, &[KUVA]));
        let kuva = &evaluation.entries[0];
        assert_eq!(kuva.opportunity.as_ref().map(|o| o.action), Some(Action::Farm));
        assert_eq!(kuva.allowable, [Allowable { unique_name: BRATON.into(), name: "Braton".into(), owned: 1 }]);
        assert!(evaluation.rejected_allowances.is_empty());

        let mut allowed = plan(31, &[KUVA]);
        allowed.allowances = [(BRATON.to_string(), 1), (SIRIUS.to_string(), 1)].into();
        let evaluation = evaluate(&overview, Some(&observed_stock), &allowed);
        let kuva = &evaluation.entries[0];
        assert_eq!(kuva.opportunity.as_ref().map(|o| o.action), Some(Action::Craft));
        assert!(kuva.allowable.is_empty());
        assert_eq!(evaluation.rejected_allowances, [SIRIUS]);
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
        assert_eq!(overview.opportunities[0].blockers, [Blocker::MasteryRankBelow { required: 2 }, Blocker::CreditCostUnknown]);
    }
}
