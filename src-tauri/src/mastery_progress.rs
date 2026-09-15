//! The inventory cache stays single-account; this file is what lets a second
//! account on the same machine load its own progress and leave the first
//! account's history untouched.

use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{error, warn};
use crate::cache::atomic_write;
use crate::inventory_state::InventoryStateCache;
use crate::mastery_rules::rank_to_affinity;
use crate::memory_scanner::BlobMission;

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ProvenanceState { Confirmed, Unconfirmed, #[default] Unknown }

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Provenance {
    pub(crate) state: ProvenanceState,
    /// Unix seconds; only Confirmed carries one.
    pub(crate) observed_at: Option<i64>,
}

impl Provenance {
    /// What an entry's presence or absence in the observed field means. Only
    /// a Confirmed field vouches for absence. An Unconfirmed record came from
    /// a cache that dropped zero rows, so absence there says nothing.
    pub(crate) fn resolve<T>(self, observed: Option<T>, absent: T) -> Option<T> {
        match (self.state, observed) {
            (ProvenanceState::Unknown, _) | (ProvenanceState::Unconfirmed, None) => None,
            (ProvenanceState::Confirmed, None) => Some(absent),
            (_, Some(value)) => Some(value),
        }
    }
}

/// An empty selection list is a plan the player cleared, kept until they
/// regenerate.
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct MasteryPlan {
    pub(crate) target: u32,
    /// Kept as text so a view the frontend later renames still loads. The
    /// frontend falls back to Suggestions for a name it no longer knows.
    pub(crate) view: String,
    pub(crate) selections: Vec<String>,
    #[serde(default)]
    pub(crate) allowances: HashMap<String, u32>,
    /// Saved targets stay fixed when the player earns more Intrinsic points.
    #[serde(default)]
    pub(crate) intrinsic_targets: HashMap<String, HashMap<String, u32>>,
    #[serde(default)]
    pub(crate) purchase_comparison: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub(crate) struct PlayerProgress {
    /// Affinity per unique_name as `XPInfo` reports it; ranks derive at read
    /// so a later rank-cap change recomputes instead of freezing.
    pub(crate) affinity: HashMap<String, i64>,
    pub(crate) equipment: Provenance,
    /// `PlayerSkills` as the game reports it, with ranks and banked points
    /// derived at read. Files written before Intrinsics were tracked lack
    /// this and `intrinsics`.
    #[serde(default)]
    pub(crate) skills: HashMap<String, i64>,
    #[serde(default)]
    pub(crate) intrinsics: Provenance,
    /// `Missions` entries per node key, tier included, so Steel Path state
    /// can derive later without a rescan.
    #[serde(default)]
    pub(crate) missions: HashMap<String, BlobMission>,
    /// Nodes and junctions both come from `Missions`, so one observation
    /// covers both kinds.
    #[serde(default)]
    pub(crate) nodes: Provenance,
    /// Lives with the progress because both are keyed by the same player
    /// name. Observations never touch it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) plan: Option<MasteryPlan>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
struct MasteryProgressCache {
    last_seen_player: Option<String>,
    players: HashMap<String, PlayerProgress>,
    /// Progress recorded before any player name was ever seen: a pre-provenance
    /// cache imported on first run, or a scan that landed before EE.log named
    /// the account. Moves under the first name that appears.
    unassigned: Option<PlayerProgress>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Owner {
    Unassigned,
    Player(String),
}

impl Owner {
    /// A pre-stamp cache stays trusted, so upgrading does not blank
    /// suggestions until the next scan. A stamped cache with no player was
    /// scanned before EE.log named the account; it serves only while no
    /// account has ever been seen, since any name makes it a possible leak
    /// from a different account.
    pub(crate) fn trusts_inventory(&self, stamped: bool, cache_player: Option<&str>) -> bool {
        match (self, stamped, cache_player) {
            (_, false, _) => true,
            (Owner::Unassigned, true, None) => true,
            (Owner::Player(_), true, None) => false,
            (Owner::Player(owner), true, Some(player)) => owner == player,
            (Owner::Unassigned, true, Some(_)) => false,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct ConfirmedKinds {
    owner: Owner,
    equipment: bool,
    intrinsics: bool,
    nodes: bool,
}

pub(crate) struct MasteryProgress {
    path: PathBuf,
    cache: MasteryProgressCache,
    /// What the last applied blob confirmed, or `None` when it confirmed
    /// nothing. An unchanged scan re-observes only those kinds for that
    /// player, never whoever logged in since.
    last_confirmed: Option<ConfirmedKinds>,
    /// An unchanged scan moves the stamp in memory once a minute. Writing
    /// the whole file that often bought nothing, so it reaches disk on an
    /// interval.
    last_reobserve_saved: Option<i64>,
    /// Set when the file exists but cannot be read: the session runs in
    /// memory and never overwrites history it could not load.
    write_blocked: bool,
}

impl MasteryProgress {
    pub(crate) fn load(path: PathBuf, pre_provenance: &InventoryStateCache) -> Self {
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(cache) => Self { path, cache, last_confirmed: None, last_reobserve_saved: None, write_blocked: false },
                Err(e) => Self::unreadable(path, &e),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let cache = MasteryProgressCache { unassigned: import_pre_provenance(pre_provenance), ..Default::default() };
                let progress = Self { path, cache, last_confirmed: None, last_reobserve_saved: None, write_blocked: false };
                progress.save();
                progress
            }
            Err(e) => Self::unreadable(path, &e),
        }
    }

    fn unreadable(path: PathBuf, error: &dyn std::fmt::Display) -> Self {
        error!(path = %path.display(), %error, "mastery progress unreadable; keeping the file, running in memory");
        Self { path, cache: MasteryProgressCache::default(), last_confirmed: None, last_reobserve_saved: None, write_blocked: true }
    }

    pub(crate) fn select_player(&mut self, name: Option<&str>) -> bool {
        let Some(name) = name else { return false };
        if self.cache.last_seen_player.as_deref() == Some(name) { return false; }
        self.cache.last_seen_player = Some(name.to_string());
        if self.cache.unassigned.is_some() {
            if self.cache.players.contains_key(name) {
                warn!("unassigned progress kept: {name} already has a record");
            } else if let Some(unassigned) = self.cache.unassigned.take() {
                self.cache.players.insert(name.to_string(), unassigned);
            }
            if let Some(confirmed) = self.last_confirmed.as_mut().filter(|c| c.owner == Owner::Unassigned) {
                confirmed.owner = Owner::Player(name.to_string());
            }
        }
        self.save();
        true
    }

    /// `affinity` is `XPInfo`, `skills` is `PlayerSkills` and `missions` is
    /// `Missions`. Each confirms its own source kind, and one that was not
    /// an array or object leaves its kind as it was.
    pub(crate) fn apply_blob(
        &mut self,
        name: Option<&str>,
        affinity: Option<&HashMap<String, i64>>,
        skills: Option<&HashMap<String, i64>>,
        missions: Option<&HashMap<String, BlobMission>>,
        now: i64,
    ) -> bool {
        self.select_player(name);
        if affinity.is_none() && skills.is_none() && missions.is_none() {
            self.last_confirmed = None;
            return false;
        }
        let owner = self.owner(name);
        let record = self.record_mut(&owner);
        let confirmed = Provenance { state: ProvenanceState::Confirmed, observed_at: Some(now) };
        if let Some(affinity) = affinity {
            record.affinity = affinity.clone();
            record.equipment = confirmed;
        }
        if let Some(skills) = skills {
            record.skills = skills.clone();
            record.intrinsics = confirmed;
        }
        if let Some(missions) = missions {
            record.missions = missions.clone();
            record.nodes = confirmed;
        }
        self.last_confirmed = Some(ConfirmedKinds {
            owner, equipment: affinity.is_some(), intrinsics: skills.is_some(), nodes: missions.is_some(),
        });
        self.save();
        true
    }

    /// A blob rejected after parsing. Its bytes are what the next unchanged
    /// scan compares against, so they must not count as a re-observation.
    pub(crate) fn discard_blob(&mut self) {
        self.last_confirmed = None;
    }

    const REOBSERVE_SAVE_INTERVAL: i64 = 300;

    pub(crate) fn reobserve(&mut self, name: Option<&str>, now: i64) -> bool {
        let owner = self.owner(name);
        let Some(confirmed) = self.last_confirmed.clone().filter(|c| c.owner == owner) else { return false };
        let record = self.record_mut(&owner);
        if confirmed.equipment { record.equipment.observed_at = Some(now); }
        if confirmed.intrinsics { record.intrinsics.observed_at = Some(now); }
        if confirmed.nodes { record.nodes.observed_at = Some(now); }
        if self.last_reobserve_saved.is_none_or(|at| now - at >= Self::REOBSERVE_SAVE_INTERVAL) {
            self.last_reobserve_saved = Some(now);
            self.save();
        }
        true
    }

    pub(crate) fn current(&self, name: Option<&str>) -> Option<&PlayerProgress> {
        match self.owner(name) {
            Owner::Player(player) => self.cache.players.get(&player),
            Owner::Unassigned => self.cache.unassigned.as_ref(),
        }
    }

    pub(crate) fn set_plan(&mut self, name: Option<&str>, plan: MasteryPlan) {
        let owner = self.owner(name);
        self.record_mut(&owner).plan = Some(plan);
        self.save();
    }

    pub(crate) fn clear(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        self.cache = MasteryProgressCache::default();
        self.last_confirmed = None;
        self.write_blocked = false;
    }

    pub(crate) fn owner(&self, name: Option<&str>) -> Owner {
        match name.or(self.cache.last_seen_player.as_deref()) {
            Some(player) => Owner::Player(player.to_string()),
            None => Owner::Unassigned,
        }
    }

    fn record_mut(&mut self, owner: &Owner) -> &mut PlayerProgress {
        match owner {
            Owner::Player(player) => self.cache.players.entry(player.clone()).or_default(),
            Owner::Unassigned => self.cache.unassigned.get_or_insert_with(Default::default),
        }
    }

    fn save(&self) {
        if self.write_blocked { return; }
        match serde_json::to_string(&self.cache) {
            Ok(json) => if let Err(e) = atomic_write(&self.path, json.as_bytes()) {
                error!(path = %self.path.display(), error = %e, "mastery progress not written");
            },
            Err(e) => error!(error = %e, "mastery progress not serialized"),
        }
    }
}

fn import_pre_provenance(cache: &InventoryStateCache) -> Option<PlayerProgress> {
    cache.has_account_observation().then(|| PlayerProgress {
        affinity: cache.mastery_data().into_iter()
            .map(|(path, rank)| { let affinity = rank_to_affinity(rank, &path); (path, affinity) })
            .collect(),
        equipment: Provenance { state: ProvenanceState::Unconfirmed, observed_at: None },
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory_state::{CachedItem, CREDITS_PATH};
    use crate::memory_scanner::BlobMission;
    use crate::mastery_rules::xp_to_rank;

    const BRATON: &str = "/Lotus/Weapons/Tenno/Rifle/Braton";
    const KUVA: &str = "/Lotus/Weapons/Grineer/KuvaLich/LongGuns/Karak/KuvaKarak";
    const MAG: &str = "/Lotus/Powersuits/Mag/Mag";

    fn scratch(label: &str) -> (tempfile::TempDir, PathBuf) { crate::cache::test_scratch(label) }

    fn pre_provenance_cache(ranks: &[(&str, u32)], observed: bool) -> InventoryStateCache {
        let mut cache = InventoryStateCache::default();
        for (path, rank) in ranks {
            cache.items.insert((*path).into(), CachedItem { unique_name: (*path).into(), mastery_rank: *rank, ..Default::default() });
        }
        if observed {
            cache.items.insert(CREDITS_PATH.into(), CachedItem { unique_name: CREDITS_PATH.into(), amount: 1, ..Default::default() });
        }
        cache
    }

    fn affinity(entries: &[(&str, i64)]) -> HashMap<String, i64> {
        entries.iter().map(|(path, xp)| ((*path).to_string(), *xp)).collect()
    }

    fn missions(entries: &[(&str, u32, Option<u32>)]) -> HashMap<String, BlobMission> {
        entries.iter().map(|(key, completes, tier)| ((*key).to_string(), BlobMission { completes: *completes, tier: *tier })).collect()
    }

    fn fresh(label: &str) -> (tempfile::TempDir, PathBuf, MasteryProgress) {
        let (dir, path) = scratch(label);
        let progress = MasteryProgress::load(path.clone(), &InventoryStateCache::default());
        (dir, path, progress)
    }

    #[test]
    fn pre_provenance_cache_with_account_observation_imports_as_unconfirmed() {
        let (_dir, path) = scratch("import");
        let progress = MasteryProgress::load(path.clone(), &pre_provenance_cache(&[(BRATON, 30), (KUVA, 40), (MAG, 12)], true));
        let record = progress.current(None).expect("ranks imported");
        assert_eq!(record.equipment, Provenance { state: ProvenanceState::Unconfirmed, observed_at: None });
        assert_eq!(xp_to_rank(record.affinity[BRATON], BRATON), 30);
        assert_eq!(xp_to_rank(record.affinity[KUVA], KUVA), 40);
        assert_eq!(xp_to_rank(record.affinity[MAG], MAG), 12);
        assert!(path.exists(), "import persisted");
    }

    #[test]
    fn catalogue_only_cache_imports_nothing() {
        let (_dir, path) = scratch("catalogue");
        let progress = MasteryProgress::load(path.clone(), &pre_provenance_cache(&[(BRATON, 0)], false));
        assert!(progress.current(None).is_none());
    }

    #[test]
    fn accepted_blob_confirms_equipment_and_replaces_the_previous_map() {
        let (_dir, _path, mut progress) = fresh("apply");
        let first = affinity(&[(BRATON, 450_000), (MAG, 144_000)]);
        assert!(progress.apply_blob(Some("Tenno"), Some(&first), None, None, 1_000));
        let record = progress.current(Some("Tenno")).expect("confirmed");
        assert_eq!(record.equipment, Provenance { state: ProvenanceState::Confirmed, observed_at: Some(1_000) });
        assert_eq!(record.affinity, first);

        let second = affinity(&[(BRATON, 450_000)]);
        assert!(progress.apply_blob(Some("Tenno"), Some(&second), None, None, 2_000));
        let record = progress.current(Some("Tenno")).expect("still confirmed");
        assert_eq!(record.affinity, second, "absent from a confirmed field is zero, not carried over");
        assert_eq!(record.equipment.observed_at, Some(2_000));

        assert!(!progress.apply_blob(Some("Tenno"), None, None, None, 3_000), "XPInfo not an array confirms nothing");
        let record = progress.current(Some("Tenno")).expect("previous observation kept");
        assert_eq!((record.affinity.clone(), record.equipment.observed_at), (second, Some(2_000)));
    }

    #[test]
    fn unchanged_scan_reobserves_only_for_the_confirmed_owner() {
        let (_dir, _path, mut progress) = fresh("reobserve");
        assert!(!progress.reobserve(Some("A"), 500), "nothing confirmed yet");
        progress.apply_blob(Some("A"), Some(&affinity(&[(BRATON, 450_000)])), None, None, 1_000);
        assert!(progress.reobserve(Some("A"), 1_060));
        assert_eq!(progress.current(Some("A")).expect("A").equipment.observed_at, Some(1_060));

        assert!(!progress.reobserve(Some("B"), 1_120), "B has no accepted blob; the bytes are A's");
        assert!(progress.current(Some("B")).is_none());
        assert_eq!(progress.current(Some("A")).expect("A").equipment.observed_at, Some(1_060));

        progress.apply_blob(Some("A"), None, None, None, 1_200);
        assert!(!progress.reobserve(Some("A"), 1_260), "last blob confirmed nothing");

        progress.apply_blob(Some("A"), Some(&affinity(&[(BRATON, 450_000)])), None, None, 1_300);
        progress.discard_blob();
        assert!(!progress.reobserve(Some("A"), 1_360), "rejected bytes are not an observation");
        assert_eq!(progress.current(Some("A")).expect("A").equipment.observed_at, Some(1_300));
    }

    #[test]
    fn player_switch_keeps_both_histories_and_a_missing_name_uses_the_last_seen() {
        let (_dir, path, mut progress) = fresh("switch");
        let braton = affinity(&[(BRATON, 450_000)]);
        let mag = affinity(&[(MAG, 900_000)]);
        progress.apply_blob(Some("A"), Some(&braton), None, None, 1_000);
        progress.apply_blob(Some("B"), Some(&mag), None, None, 2_000);
        assert_eq!(progress.current(Some("A")).expect("A").affinity, braton);
        assert_eq!(progress.current(Some("B")).expect("B").affinity, mag);
        assert_eq!(progress.current(None).expect("last seen is B").affinity, mag);

        let kuva = affinity(&[(KUVA, 800_000)]);
        progress.apply_blob(None, Some(&kuva), None, None, 3_000);
        assert_eq!(progress.current(Some("B")).expect("B").affinity, kuva);
        assert_eq!(progress.current(Some("A")).expect("A").equipment.observed_at, Some(1_000));

        let mut reloaded = MasteryProgress::load(path.clone(), &InventoryStateCache::default());
        let b = reloaded.current(None).expect("B survives restart");
        assert_eq!((b.affinity.clone(), b.equipment.observed_at), (kuva.clone(), Some(3_000)));
        assert_eq!(reloaded.current(Some("A")).expect("A survives restart").affinity, braton);

        let braton_and_mag = affinity(&[(BRATON, 450_000), (MAG, 900_000)]);
        reloaded.apply_blob(Some("A"), Some(&braton_and_mag), None, None, 4_000);
        assert_eq!(reloaded.current(Some("A")).expect("A").affinity, braton_and_mag);
        assert_eq!(reloaded.current(Some("B")).expect("B untouched by A's return").affinity, kuva);
    }

    #[test]
    fn progress_before_any_name_moves_under_the_first_name_only() {
        let (_dir, path) = scratch("unassigned");
        let mut progress = MasteryProgress::load(path.clone(), &pre_provenance_cache(&[(BRATON, 30)], true));
        let mag = affinity(&[(MAG, 900_000)]);
        assert!(progress.apply_blob(None, Some(&mag), None, None, 1_000), "no name known yet: replaces the import");
        assert_eq!(progress.current(None).expect("unassigned").equipment.state, ProvenanceState::Confirmed);

        assert!(progress.select_player(Some("A")));
        assert!(!progress.select_player(Some("A")));
        let a = progress.current(Some("A")).expect("A inherits the unassigned record");
        assert_eq!((a.affinity.clone(), a.equipment.observed_at), (mag, Some(1_000)));
        assert!(progress.reobserve(Some("A"), 1_060), "the confirmed blob now belongs to A");

        progress.select_player(Some("B"));
        assert!(progress.current(Some("B")).is_none(), "only the first name inherits");
    }

    #[test]
    fn unreadable_file_is_kept_and_never_overwritten() {
        let (_dir, path) = scratch("corrupt");
        std::fs::write(&path, b"{ not json").expect("scratch writable");
        let mut progress = MasteryProgress::load(path.clone(), &pre_provenance_cache(&[(BRATON, 30)], true));
        assert!(progress.current(None).is_none(), "no import over an existing file");
        progress.apply_blob(Some("A"), Some(&affinity(&[(BRATON, 450_000)])), None, None, 1_000);
        assert_eq!(progress.current(Some("A")).expect("in-memory progress works").equipment.observed_at, Some(1_000));
        assert_eq!(std::fs::read(&path).expect("file kept"), b"{ not json");
    }

    #[test]
    fn missions_confirm_nodes_separately_from_equipment() {
        let (_dir, _path, mut progress) = fresh("missions");
        let cleared = missions(&[("SolNode27", 14, Some(1)), ("EarthToVenusJunction", 2, None)]);
        assert!(progress.apply_blob(Some("A"), Some(&affinity(&[(BRATON, 450_000)])), None, Some(&cleared), 1_000));
        let record = progress.current(Some("A")).expect("confirmed");
        assert_eq!(record.missions, cleared);
        assert_eq!(record.nodes, Provenance { state: ProvenanceState::Confirmed, observed_at: Some(1_000) });

        assert!(progress.apply_blob(Some("A"), Some(&affinity(&[(BRATON, 450_000)])), None, None, 2_000), "XPInfo alone still confirms equipment");
        let record = progress.current(Some("A")).expect("kept");
        assert_eq!((record.equipment.observed_at, record.nodes.observed_at), (Some(2_000), Some(1_000)));
        assert_eq!(record.missions, cleared, "Missions not an array leaves the last observation in place");
        assert!(progress.reobserve(Some("A"), 2_060));
        assert_eq!((record_of(&progress).equipment.observed_at, record_of(&progress).nodes.observed_at), (Some(2_060), Some(1_000)),
            "an unchanged scan re-observes only what the last blob confirmed");

        assert!(progress.apply_blob(Some("A"), None, None, Some(&missions(&[("SolNode27", 15, Some(1))])), 3_000), "Missions alone confirms nodes");
        let record = progress.current(Some("A")).expect("kept");
        assert_eq!((record.equipment.observed_at, record.nodes.observed_at), (Some(2_060), Some(3_000)));
        assert_eq!(record.missions.len(), 1, "absent from a confirmed field is zero, not carried over");

        assert!(!progress.apply_blob(Some("A"), None, None, None, 4_000));
        assert!(!progress.reobserve(Some("A"), 4_060));
    }

    fn record_of(progress: &MasteryProgress) -> &PlayerProgress {
        progress.current(Some("A")).expect("A")
    }

    #[test]
    fn a_file_from_before_missions_loads_them_as_unknown() {
        let (_dir, path) = scratch("pre-missions");
        std::fs::write(&path, format!(
            r#"{{"last_seen_player":"A","players":{{"A":{{"affinity":{{"{BRATON}":450000}},"equipment":{{"state":"confirmed","observed_at":1000}}}}}},"unassigned":null}}"#
        )).expect("scratch writable");
        let progress = MasteryProgress::load(path.clone(), &InventoryStateCache::default());
        let record = progress.current(Some("A")).expect("A loads");
        assert_eq!(record.equipment.state, ProvenanceState::Confirmed);
        assert_eq!(record.nodes, Provenance::default());
        assert!(record.missions.is_empty());
    }

    #[test]
    fn a_plan_sits_under_its_player_and_outlives_observations_and_restarts() {
        let (_dir, path, mut progress) = fresh("plan");
        let plan = |target: u32, selections: &[&str]| MasteryPlan {
            target, view: "suggestions".into(), selections: selections.iter().map(|s| (*s).to_string()).collect(), allowances: HashMap::new(), intrinsic_targets: HashMap::new(), purchase_comparison: String::new(),
        };
        progress.apply_blob(Some("A"), Some(&affinity(&[(BRATON, 450_000)])), None, None, 1_000);
        assert_eq!(progress.current(Some("A")).and_then(|p| p.plan.as_ref()), None, "no plan until one is saved");
        progress.set_plan(Some("A"), plan(12, &[KUVA, MAG]));
        progress.set_plan(Some("B"), plan(5, &[]));
        progress.apply_blob(Some("A"), Some(&affinity(&[(BRATON, 450_000), (KUVA, 800_000)])), None, None, 2_000);
        assert_eq!(progress.current(Some("A")).and_then(|p| p.plan.clone()), Some(plan(12, &[KUVA, MAG])), "a new observation leaves the plan alone");

        let reloaded = MasteryProgress::load(path.clone(), &InventoryStateCache::default());
        assert_eq!(reloaded.current(Some("A")).and_then(|p| p.plan.clone()), Some(plan(12, &[KUVA, MAG])));
        assert_eq!(reloaded.current(Some("B")).and_then(|p| p.plan.clone()), Some(plan(5, &[])), "a cleared plan is still a plan");
        assert_eq!(reloaded.current(None).and_then(|p| p.plan.clone()), Some(plan(12, &[KUVA, MAG])), "no name reads the last seen player's plan");
    }

    #[test]
    fn clear_forgets_every_player_and_unblocks_writes() {
        let (_dir, path) = scratch("clear");
        std::fs::write(&path, b"{ not json").expect("scratch writable");
        let mut progress = MasteryProgress::load(path.clone(), &InventoryStateCache::default());
        progress.apply_blob(Some("A"), Some(&affinity(&[(BRATON, 450_000)])), None, None, 1_000);
        progress.clear();
        assert!(progress.current(Some("A")).is_none());
        assert!(!path.exists());
        progress.apply_blob(Some("A"), Some(&affinity(&[(BRATON, 450_000)])), None, None, 2_000);
        assert!(path.exists(), "writes resume after clear");
    }

    #[test]
    fn intrinsics_confirm_apart_from_equipment_and_a_legacy_file_leaves_them_unknown() {
        let (_dir, path) = scratch("intrinsics");
        std::fs::write(&path, r#"{"last_seen_player":"A","players":{"A":{"affinity":{},"equipment":{"state":"confirmed","observed_at":900}}},"unassigned":null}"#)
            .expect("scratch writable");
        let mut progress = MasteryProgress::load(path.clone(), &InventoryStateCache::default());
        let a = progress.current(Some("A")).expect("A loaded");
        assert_eq!((a.equipment.state, a.intrinsics), (ProvenanceState::Confirmed, Provenance::default()));

        let skills = affinity(&[("LPS_GUNNERY", 8), ("LPP_SPACE", 89_930)]);
        assert!(progress.apply_blob(Some("A"), None, Some(&skills), None, 1_000), "skills alone confirm something");
        let a = progress.current(Some("A")).expect("A");
        assert_eq!(a.intrinsics, Provenance { state: ProvenanceState::Confirmed, observed_at: Some(1_000) });
        assert_eq!((a.skills.clone(), a.equipment.observed_at), (skills.clone(), Some(900)));

        assert!(progress.reobserve(Some("A"), 1_060));
        let a = progress.current(Some("A")).expect("A");
        assert_eq!((a.intrinsics.observed_at, a.equipment.observed_at), (Some(1_060), Some(900)), "only what the last blob confirmed re-observes");

        assert!(progress.apply_blob(Some("A"), Some(&affinity(&[(BRATON, 450_000)])), None, None, 2_000));
        let a = progress.current(Some("A")).expect("A");
        assert_eq!((a.equipment.observed_at, a.intrinsics.observed_at), (Some(2_000), Some(1_060)), "skills not an object: intrinsics kept as they were");
        assert!(progress.reobserve(Some("A"), 2_060));
        let a = progress.current(Some("A")).expect("A");
        assert_eq!((a.equipment.observed_at, a.intrinsics.observed_at), (Some(2_060), Some(1_060)));

        let reloaded = MasteryProgress::load(path.clone(), &InventoryStateCache::default());
        let a = reloaded.current(Some("A")).expect("A survives restart");
        assert_eq!((a.skills.clone(), a.intrinsics.observed_at), (skills, Some(1_060)));
    }

    #[test]
    fn inventory_is_trusted_only_when_stamped_for_the_progress_owner_or_not_at_all() {
        let (_dir, _path, mut progress) = fresh("inventory-owner");
        assert!(progress.owner(None).trusts_inventory(false, None), "pre-stamp cache, nobody seen");
        assert!(progress.owner(None).trusts_inventory(true, None), "scan before login, nobody ever seen");
        assert!(!progress.owner(None).trusts_inventory(true, Some("A")), "A's scan, nobody seen: not ours");
        progress.select_player(Some("A"));
        assert!(progress.owner(None).trusts_inventory(true, Some("A")), "resolves through the last seen player");
        assert!(progress.owner(Some("B")).trusts_inventory(false, None), "pre-stamp cache is trusted as before");
        assert!(!progress.owner(Some("B")).trusts_inventory(true, None), "scan before login could be anyone's");
        assert!(!progress.owner(Some("B")).trusts_inventory(true, Some("A")), "A's scan must not serve B");
        assert!(progress.owner(Some("B")).trusts_inventory(true, Some("B")));
    }
}
