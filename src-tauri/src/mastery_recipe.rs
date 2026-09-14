//! Inventory-aware recipe expansion shared by the mastery views, the Foundry
//! and the tracked list.
//!
//! Targets plan in display order against one projected stock ledger, so an
//! ingredient two targets share is available to the first and short for the
//! second. Owned stock covers a need before anything expands. Only the
//! missing quantity is crafted, and only where the intermediate's blueprint
//! is in hand or is consumed per craft. A reusable blueprint the player lacks
//! (the Baro resource recipes, dojo research) is an acquisition of its own,
//! so a resource behind one stays a shortage rather than unfolding into the
//! hundreds of thousands of Alloy Plate its recipe asks for.

use std::collections::{HashMap, HashSet};
use crate::app_state::AppState;
use crate::inventory_state::{load_inventory_state_cache, InventoryStateCache, CREDITS_PATH};
use crate::wfcd::RecipeComponent;
use tauri::Manager;

#[derive(serde::Serialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Requirement {
    pub(crate) unique_name: String,
    pub(crate) name: String,
    pub(crate) needed: u32,
    /// How many came out of projected stock. A reusable blueprint counts once
    /// and is not consumed.
    pub(crate) from_stock: u32,
    /// The remainder that neither stock nor a build covers.
    pub(crate) short: u32,
}

#[derive(serde::Serialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Build {
    pub(crate) unique_name: String,
    pub(crate) name: String,
    pub(crate) crafts: u32,
}

#[derive(serde::Serialize, Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct CraftPlan {
    /// One line per item the target needs, blueprints included, in recipe
    /// order. A component listed twice merges into one line.
    pub(crate) requirements: Vec<Requirement>,
    pub(crate) builds: Vec<Build>,
    /// The build price of the target plus every build. It turns None as soon
    /// as any of those recipes carries no price, so a partial sum never reads
    /// as the full cost.
    pub(crate) credits: Option<u64>,
    /// Credits still missing once earlier targets took theirs. It stays 0
    /// while the cost or the balance is unknown.
    pub(crate) credits_short: u64,
}

impl CraftPlan {
    pub(crate) fn shortages(&self) -> impl Iterator<Item = &Requirement> {
        self.requirements.iter().filter(|r| r.short > 0)
    }

    pub(crate) fn craftable_now(&self) -> bool {
        self.builds.is_empty() && self.shortages().next().is_none()
    }

    pub(crate) fn buildable(&self) -> bool {
        !self.builds.is_empty() && self.shortages().next().is_none()
    }

    fn require(&mut self, component: &RecipeComponent, needed: u32, from_stock: u32, short: u32) {
        match self.requirements.iter_mut().find(|r| r.unique_name == component.unique_name) {
            Some(line) => {
                line.needed += needed;
                line.from_stock += from_stock;
                line.short += short;
            }
            None => self.requirements.push(Requirement {
                unique_name: component.unique_name.clone(), name: component.name.clone(), needed, from_stock, short,
            }),
        }
    }

    fn build(&mut self, component: &RecipeComponent, crafts: u32) {
        match self.builds.iter_mut().find(|b| b.unique_name == component.unique_name) {
            Some(line) => line.crafts += crafts,
            None => self.builds.push(Build { unique_name: component.unique_name.clone(), name: component.name.clone(), crafts }),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Ledger<'a> {
    stock: HashMap<&'a str, i64>,
}

impl<'a> Ledger<'a> {
    /// Owned equipment stays out of stock, so a weapon the player owns is
    /// never spent on the next one. An allowance admits up to that many
    /// copies of a mastered piece the player owns. The returned list names
    /// the allowances refused, whether unmastered, of unknown mastery, or not
    /// owned.
    pub(crate) fn new(
        stock: &'a HashMap<String, i64>,
        equipment: &'a HashMap<String, i64>,
        mastered: &HashSet<&str>,
        allowances: &HashMap<String, u32>,
    ) -> (Self, Vec<String>) {
        let mut stock: HashMap<&'a str, i64> = stock.iter()
            .filter(|(path, _)| !equipment.contains_key(*path))
            .map(|(path, &n)| (path.as_str(), n))
            .collect();
        let mut rejected = vec![];
        for (path, &allowed) in allowances {
            match equipment.get_key_value(path) {
                Some((path, &owned)) if mastered.contains(path.as_str()) => {
                    stock.insert(path, i64::from(allowed).min(owned));
                }
                _ => rejected.push(path.clone()),
            }
        }
        rejected.sort();
        (Self { stock }, rejected)
    }

    pub(crate) fn plan(&mut self, target: &str, components: &'a [RecipeComponent]) -> CraftPlan {
        let mut plan = CraftPlan { credits: Some(0), ..Default::default() };
        self.expand(target, components, 1, &mut plan);
        if let (Some(cost), Some(&balance)) = (plan.credits, self.stock.get(CREDITS_PATH)) {
            let taken = cost.min(balance.max(0) as u64);
            self.stock.insert(CREDITS_PATH, balance - taken as i64);
            plan.credits_short = cost - taken;
        }
        plan
    }

    fn expand(&mut self, target: &str, components: &'a [RecipeComponent], crafts: u32, plan: &mut CraftPlan) {
        let mut priced = false;
        for (count, component) in merged(components) {
            // A recipe occasionally lists its own result among the ingredients.
            if component.unique_name == target { continue; }
            let blueprint = is_blueprint(component);
            if blueprint {
                plan.credits = match (plan.credits, component.credits) {
                    (Some(total), Some(price)) => Some(total + u64::from(price) * u64::from(crafts)),
                    _ => None,
                };
                priced = true;
            }
            let keep = blueprint && component.reusable;
            let needed = if keep { 1 } else { count * crafts };
            let path = component.unique_name.as_str();
            let have = self.stock.get(path).copied().unwrap_or(0).max(0);
            let from_stock = needed.min(u32::try_from(have).unwrap_or(u32::MAX));
            if !keep { *self.stock.entry(path).or_insert(0) -= i64::from(from_stock); }
            let missing = needed - from_stock;
            let expands = missing > 0 && !component.components.is_empty() && self.can_expand(component);
            plan.require(component, needed, from_stock, if expands { 0 } else { missing });
            if expands {
                let per_craft = component.result_count.max(1);
                let builds = missing.div_ceil(per_craft);
                let surplus = builds * per_craft - missing;
                if surplus > 0 { *self.stock.entry(path).or_insert(0) += i64::from(surplus); }
                plan.build(component, builds);
                self.expand(target, &component.components, builds, plan);
            }
        }
        if !priced { plan.credits = None; }
    }

    /// A missing consumable blueprint shows up as a shortage like any part,
    /// because the player buys or finds one per craft. A missing reusable
    /// blueprint is a one-off acquisition that gates the whole recipe, so the
    /// intermediate does not expand until it is in hand.
    fn can_expand(&self, intermediate: &RecipeComponent) -> bool {
        intermediate.components.iter().filter(|c| is_blueprint(c))
            .all(|bp| !bp.reusable || self.stock.get(bp.unique_name.as_str()).is_some_and(|&n| n > 0))
    }
}

pub(crate) fn plan_all(inventory: &InventoryStateCache, recipes: &HashMap<String, Vec<RecipeComponent>>, targets: &[String]) -> Vec<CraftPlan> {
    let (stock, owned) = (inventory.stackable_quantities(), inventory.unique_quantities());
    let (mut ledger, _) = Ledger::new(&stock, &owned, &HashSet::new(), &HashMap::new());
    targets.iter().map(|target| ledger.plan(target, recipes.get(target).map_or(&[], Vec::as_slice))).collect()
}

#[tauri::command]
pub(crate) async fn plan_crafts(app: tauri::AppHandle, unique_names: Vec<String>) -> Result<Vec<CraftPlan>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let inventory = load_inventory_state_cache(&state.inventory_state_cache_path);
        let recipes = state.recipes.lock().unwrap_or_else(|e| e.into_inner());
        plan_all(&inventory, &recipes, &unique_names)
    }).await.map_err(|e| e.to_string())
}

pub(crate) fn is_blueprint(component: &RecipeComponent) -> bool {
    component.components.is_empty() && component.unique_name.ends_with("Blueprint")
}

/// Akbolto lists Bolto twice rather than once with count 2.
fn merged(components: &[RecipeComponent]) -> Vec<(u32, &RecipeComponent)> {
    let mut out: Vec<(u32, &RecipeComponent)> = vec![];
    for component in components {
        match out.iter_mut().find(|(_, seen)| seen.unique_name == component.unique_name) {
            Some((count, _)) => *count += component.count,
            None => out.push((component.count, component)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory_state::{CachedItem, InventoryStateCache};
    use std::sync::LazyLock;

    const FROST: &str = "/Lotus/Powersuits/Frost/Frost";
    const FROST_BP: &str = "/Lotus/Types/Recipes/WarframeRecipes/FrostBlueprint";
    const CHASSIS: &str = "/Lotus/Types/Recipes/WarframeRecipes/FrostChassisComponent";
    const CHASSIS_BP: &str = "/Lotus/Types/Recipes/WarframeRecipes/FrostChassisBlueprint";
    const FERRITE: &str = "/Lotus/Types/Items/MiscItems/Ferrite";
    const CELL: &str = "/Lotus/Types/Items/MiscItems/OrokinCell";
    const CELL_BP: &str = "/Lotus/Types/Recipes/Components/OrokinCellResourceBlueprint";
    const ALLOY: &str = "/Lotus/Types/Items/MiscItems/AlloyPlate";
    const AKBOLTO: &str = "/Lotus/Weapons/Tenno/Akimbo/AkimboBolto";
    const AKBOLTO_BP: &str = "/Lotus/Types/Recipes/Weapons/AkBoltoBlueprint";
    const BOLTO: &str = "/Lotus/Weapons/Tenno/Pistol/CrossBow";
    const BOLTO_BP: &str = "/Lotus/Types/Recipes/Weapons/BoltoBlueprint";
    const LATO: &str = "/Lotus/Weapons/Tenno/Pistol/Pistol";
    const BALLA: &str = "/Lotus/Weapons/Ostron/Melee/ModularMelee01/Tip/TipOne";
    const BALLA_BP: &str = "/Lotus/Weapons/Ostron/Melee/ModularMelee01/Tip/TipOneBlueprint";
    const IRADITE: &str = "/Lotus/Types/Items/Gameplay/Eidolon/Resources/IraditeItem";
    const PAIR: &str = "/Lotus/Types/Items/MiscItems/Pair";
    const PAIR_BP: &str = "/Lotus/Types/Recipes/Components/PairBlueprint";
    const TARGET: &str = "/Lotus/Types/Target";
    const TARGET_BP: &str = "/Lotus/Types/Recipes/TargetBlueprint";

    static NO_EQUIPMENT: LazyLock<HashMap<String, i64>> = LazyLock::new(HashMap::new);

    fn leaf(path: &str, count: u32) -> RecipeComponent {
        RecipeComponent {
            unique_name: path.into(), name: path.rsplit('/').next().expect("path has a segment").into(),
            count, result_count: 1, components: vec![], credits: None, reusable: false,
        }
    }

    fn blueprint(path: &str, credits: Option<u32>, reusable: bool) -> RecipeComponent {
        RecipeComponent { credits, reusable, ..leaf(path, 1) }
    }

    fn part(path: &str, count: u32, result_count: u32, components: Vec<RecipeComponent>) -> RecipeComponent {
        RecipeComponent { result_count, components, ..leaf(path, count) }
    }

    fn cell(count: u32) -> RecipeComponent {
        part(CELL, count, 1, vec![blueprint(CELL_BP, Some(0), true), leaf(ALLOY, 50_000)])
    }

    fn frost() -> Vec<RecipeComponent> {
        vec![
            blueprint(FROST_BP, Some(25_000), false),
            part(CHASSIS, 1, 1, vec![blueprint(CHASSIS_BP, Some(15_000), false), leaf(FERRITE, 1_000)]),
            cell(1),
        ]
    }

    fn akbolto() -> Vec<RecipeComponent> {
        let bolto = || part(BOLTO, 1, 1, vec![blueprint(BOLTO_BP, Some(20_000), false), leaf(LATO, 1), cell(2)]);
        vec![blueprint(AKBOLTO_BP, Some(20_000), false), bolto(), bolto(), cell(1)]
    }

    fn stock_of(entries: &[(&str, i64)]) -> HashMap<String, i64> {
        entries.iter().map(|(path, n)| ((*path).to_string(), *n)).collect()
    }

    fn ledger<'a>(stock: &'a HashMap<String, i64>, equipment: &'a HashMap<String, i64>) -> Ledger<'a> {
        let (ledger, rejected) = Ledger::new(stock, equipment, &HashSet::new(), &HashMap::new());
        assert!(rejected.is_empty());
        ledger
    }

    fn short(plan: &CraftPlan) -> Vec<(&str, u32)> {
        plan.shortages().map(|r| (r.unique_name.as_str(), r.short)).collect()
    }

    fn builds(plan: &CraftPlan) -> Vec<(&str, u32)> {
        plan.builds.iter().map(|b| (b.unique_name.as_str(), b.crafts)).collect()
    }

    fn line<'p>(plan: &'p CraftPlan, path: &str) -> &'p Requirement {
        plan.requirements.iter().find(|r| r.unique_name == path).unwrap_or_else(|| panic!("{path} listed"))
    }

    #[test]
    fn an_owned_intermediate_is_taken_as_is_and_only_the_missing_remainder_expands() {
        let recipe = frost();
        let stock = stock_of(&[(FROST_BP, 1), (CHASSIS, 1), (CELL, 1), (CREDITS_PATH, 100_000)]);
        let plan = ledger(&stock, &NO_EQUIPMENT).plan(FROST, &recipe);
        assert!(plan.craftable_now());
        assert_eq!((plan.credits, plan.credits_short), (Some(25_000), 0));
        assert!(!plan.requirements.iter().any(|r| r.unique_name == FERRITE));

        let recipe = akbolto();
        let stock = stock_of(&[(AKBOLTO_BP, 1), (BOLTO_BP, 1), (CELL, 3), (CREDITS_PATH, 100_000)]);
        let plan = ledger(&stock, &NO_EQUIPMENT).plan(AKBOLTO, &recipe);
        assert_eq!(builds(&plan), [(BOLTO, 2)]);
        assert_eq!(short(&plan), [(BOLTO_BP, 1), (LATO, 2), (CELL, 2)]);
        assert_eq!(plan.credits, Some(60_000));
        assert!(!plan.craftable_now() && !plan.buildable());

        let stock = stock_of(&[(AKBOLTO_BP, 1), (BOLTO_BP, 1), (LATO, 1), (CELL, 3), (CREDITS_PATH, 100_000)]);
        let plan = ledger(&stock, &NO_EQUIPMENT).plan(AKBOLTO, &recipe);
        assert_eq!(short(&plan), [(BOLTO_BP, 1), (LATO, 1), (CELL, 2)]);
        let bolto = line(&plan, BOLTO);
        assert_eq!((bolto.needed, bolto.from_stock, bolto.short), (2, 0, 0));

        let stock = stock_of(&[(AKBOLTO_BP, 1), (BOLTO, 1), (BOLTO_BP, 1), (LATO, 1), (CELL, 3), (CREDITS_PATH, 100_000)]);
        let plan = ledger(&stock, &NO_EQUIPMENT).plan(AKBOLTO, &recipe);
        assert_eq!(builds(&plan), [(BOLTO, 1)]);
        assert!(plan.buildable());
        assert_eq!(plan.credits, Some(40_000));
        let bolto = line(&plan, BOLTO);
        assert_eq!((bolto.needed, bolto.from_stock, bolto.short), (2, 1, 0));
    }

    #[test]
    fn a_shared_ingredient_goes_to_the_first_target_and_leaves_the_second_short() {
        let (frost, akbolto) = (frost(), akbolto());
        let stock = stock_of(&[(FROST_BP, 1), (CHASSIS, 1), (AKBOLTO_BP, 1), (BOLTO, 2), (CELL, 1), (CREDITS_PATH, 30_000)]);
        let mut ledger = ledger(&stock, &NO_EQUIPMENT);
        let first = ledger.plan(FROST, &frost);
        let second = ledger.plan(AKBOLTO, &akbolto);
        assert!(first.craftable_now());
        assert_eq!(short(&second), [(CELL, 1)]);
        assert_eq!((second.credits, second.credits_short), (Some(20_000), 15_000));
    }

    #[test]
    fn a_batch_yield_leaves_its_surplus_for_the_next_target() {
        let recipe = |n| vec![blueprint(TARGET_BP, Some(0), false),
            part(PAIR, n, 2, vec![blueprint(PAIR_BP, Some(0), false), leaf(FERRITE, 10)])];
        let (three, one) = (recipe(3), recipe(1));
        let stock = stock_of(&[(TARGET_BP, 2), (PAIR_BP, 2), (FERRITE, 20)]);
        let mut ledger = ledger(&stock, &NO_EQUIPMENT);
        let first = ledger.plan(TARGET, &three);
        assert_eq!(builds(&first), [(PAIR, 2)]);
        assert!(first.buildable());
        let second = ledger.plan(TARGET, &one);
        assert!(second.craftable_now());
        assert_eq!((line(&second, PAIR).from_stock, line(&second, PAIR).short), (1, 0));
    }

    #[test]
    fn a_reusable_blueprint_is_needed_once_and_never_consumed() {
        let strike = || vec![blueprint(BALLA_BP, Some(1_000), true), leaf(IRADITE, 20)];
        let (first, second) = (strike(), strike());
        let stock = stock_of(&[(BALLA_BP, 1), (IRADITE, 40), (CREDITS_PATH, 5_000)]);
        let mut shared = ledger(&stock, &NO_EQUIPMENT);
        assert!(shared.plan(BALLA, &first).craftable_now());
        let plan = shared.plan(BALLA, &second);
        assert!(plan.craftable_now());
        assert_eq!(plan.requirements[0], Requirement { unique_name: BALLA_BP.into(), name: "TipOneBlueprint".into(), needed: 1, from_stock: 1, short: 0 });

        let stock = stock_of(&[(IRADITE, 40)]);
        let plan = ledger(&stock, &NO_EQUIPMENT).plan(BALLA, &first);
        assert_eq!(short(&plan), [(BALLA_BP, 1)]);
    }

    #[test]
    fn a_resource_behind_a_reusable_blueprint_the_player_lacks_stays_a_shortage() {
        let recipe = frost();
        let stock = stock_of(&[(FROST_BP, 1), (CHASSIS, 1), (ALLOY, 1_000_000)]);
        let plan = ledger(&stock, &NO_EQUIPMENT).plan(FROST, &recipe);
        assert_eq!(short(&plan), [(CELL, 1)]);
        assert!(builds(&plan).is_empty());

        let stock = stock_of(&[(FROST_BP, 1), (CHASSIS, 1), (ALLOY, 1_000_000), (CELL_BP, 1)]);
        let plan = ledger(&stock, &NO_EQUIPMENT).plan(FROST, &recipe);
        assert_eq!(builds(&plan), [(CELL, 1)]);
        assert!(plan.buildable());
    }

    #[test]
    fn owned_equipment_is_never_an_ingredient_and_only_a_mastered_allowance_admits_it() {
        let recipe = akbolto();
        let stock = stock_of(&[(AKBOLTO_BP, 1), (BOLTO_BP, 2), (LATO, 5), (CELL, 5), (CREDITS_PATH, 100_000)]);
        let equipment = stock_of(&[(BOLTO, 2), (LATO, 1)]);
        let plan = ledger(&stock, &equipment).plan(AKBOLTO, &recipe);
        assert_eq!(builds(&plan), [(BOLTO, 2)]);
        assert_eq!(short(&plan), [(LATO, 2)]);

        let mastered: HashSet<&str> = [BOLTO].into();
        let allowances: HashMap<String, u32> = [(BOLTO.to_string(), 5), (LATO.to_string(), 1), (AKBOLTO.to_string(), 1)].into();
        let (mut ledger, rejected) = Ledger::new(&stock, &equipment, &mastered, &allowances);
        assert_eq!(rejected, [AKBOLTO, LATO]);
        let plan = ledger.plan(AKBOLTO, &recipe);
        assert!(plan.craftable_now());
        assert_eq!((line(&plan, BOLTO).needed, line(&plan, BOLTO).from_stock), (2, 2));
    }

    #[test]
    fn tracked_targets_draw_on_one_ledger_in_order_and_spare_owned_equipment() {
        let recipes: HashMap<String, Vec<RecipeComponent>> = [(FROST.to_string(), frost()), (AKBOLTO.to_string(), akbolto())].into();
        let mut inventory = InventoryStateCache::default();
        for (path, amount, category) in [
            (FROST_BP, 1, "Blueprints"), (CHASSIS, 1, "Parts"), (AKBOLTO_BP, 1, "Blueprints"),
            (BOLTO_BP, 1, "Blueprints"), (BOLTO, 1, "Secondary"), (LATO, 1, "Secondary"), (CELL, 1, "Resources"),
        ] {
            inventory.items.insert(path.into(), CachedItem { unique_name: path.into(), amount, category: category.into(), ..Default::default() });
        }

        let [frost, akbolto] = <[CraftPlan; 2]>::try_from(plan_all(&inventory, &recipes, &[FROST.into(), AKBOLTO.into()])).expect("one plan per target");
        assert!(frost.craftable_now());
        assert!(!frost.requirements.iter().any(|r| r.unique_name == FERRITE));
        // The owned Bolto and Lato are equipment, so neither is an ingredient.
        assert_eq!(builds(&akbolto), [(BOLTO, 2)]);
        assert_eq!(short(&akbolto), [(BOLTO_BP, 1), (LATO, 2), (CELL, 5)]);

        let [akbolto, frost] = <[CraftPlan; 2]>::try_from(plan_all(&inventory, &recipes, &[AKBOLTO.into(), FROST.into()])).expect("one plan per target");
        assert_eq!(short(&akbolto), [(BOLTO_BP, 1), (LATO, 2), (CELL, 4)]);
        assert_eq!(short(&frost), [(CELL, 1)]);

        assert!(plan_all(&inventory, &recipes, &["/Lotus/Types/Unknown".into()])[0].requirements.is_empty());
    }

    #[test]
    fn credits_stay_unknown_without_a_price_and_short_only_against_an_observed_balance() {
        let recipe = vec![blueprint(FROST_BP, None, false), leaf(FERRITE, 1)];
        let stock = stock_of(&[(FROST_BP, 1), (FERRITE, 1), (CREDITS_PATH, 10)]);
        let plan = ledger(&stock, &NO_EQUIPMENT).plan(FROST, &recipe);
        assert_eq!((plan.credits, plan.credits_short), (None, 0));
        assert!(plan.craftable_now());

        let recipe = frost();
        let stock = stock_of(&[(FROST_BP, 1), (CHASSIS_BP, 1), (FERRITE, 1_000), (CELL, 1)]);
        let plan = ledger(&stock, &NO_EQUIPMENT).plan(FROST, &recipe);
        assert_eq!((plan.credits, plan.credits_short), (Some(40_000), 0));
        assert!(plan.buildable());

        let stock = stock_of(&[(FROST_BP, 1), (CHASSIS_BP, 1), (FERRITE, 1_000), (CELL, 1), (CREDITS_PATH, 10_000)]);
        let plan = ledger(&stock, &NO_EQUIPMENT).plan(FROST, &recipe);
        assert_eq!(plan.credits_short, 30_000);
    }
}
