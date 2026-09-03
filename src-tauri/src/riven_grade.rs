//! Grading of owned rivens: scanned rivens in, graded entries out.
//!
//! Everything the frontend needs to render a riven is decided here — the stat
//! values and their ranges, and the analyzer's verdict — so the same scale
//! applies to owned rivens, auction results and the market badge.

use crate::memory_scanner::{BlobRivenEntry, BlobRivenStat, RivenState};
use crate::riven_stats::{self, RollContext, Unit};
use crate::{AppState, RivenAnalysis};

/// One rolled stat, placed in the range it could have rolled in at this rank.
#[derive(serde::Serialize)]
pub struct GradedStat {
    pub tag: String,
    /// The roll as the game stores it. Posting an auction needs the untouched
    /// value, since the market recomputes the stat from it.
    pub raw: i64,
    pub label: String,
    pub display: String,
    pub analyzer_name: Option<String>,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    /// `min` and `max` in the same units and formatting as `display`.
    pub min_display: String,
    pub max_display: String,
    pub position: f64,
    pub unit: Unit,
    pub positive: bool,
}

/// A riven the scanner holds, with its stats placed in range and its roll graded.
/// `analysis` is `None` whenever the roll cannot be judged, never a score of zero.
#[derive(serde::Serialize)]
pub struct GradedRiven {
    pub item_id: String,
    pub item_type: String,
    pub state: RivenState,
    pub compat: Option<String>,
    pub weapon_name: Option<String>,
    pub category: String,
    pub polarity: Option<String>,
    pub mastery_level: Option<u32>,
    pub mod_rank: u8,
    pub rerolls: u32,
    pub count: u32,
    pub mod_name: String,
    pub disposition: f32,
    pub buffs: Vec<GradedStat>,
    pub curses: Vec<GradedStat>,
    pub analysis: Option<RivenAnalysis>,
    pub challenge: Option<String>,
    pub complication: Option<String>,
}

impl GradedRiven {
    /// Analyzer stat names of the buffs and curses. Unmapped tags take no part
    /// in the score, so they are left out.
    fn analyzer_inputs(&self) -> (Vec<String>, Vec<String>) {
        let names = |stats: &[GradedStat]| {
            stats.iter().filter_map(|s| s.analyzer_name.clone()).collect()
        };
        (names(&self.buffs), names(&self.curses))
    }
}

fn graded_stat(stat: &BlobRivenStat, positive: bool, ctx: RollContext) -> GradedStat {
    let v = riven_stats::evaluate(&stat.tag, stat.value, positive, ctx);
    GradedStat {
        label: if v.label.is_empty() { stat.tag.clone() } else { v.label.to_string() },
        tag: stat.tag.clone(),
        raw: stat.value,
        display: v.display,
        analyzer_name: v.analyzer_name.map(str::to_string),
        value: v.value,
        min: v.min,
        max: v.max,
        min_display: v.min_display,
        max_display: v.max_display,
        position: v.position,
        unit: v.unit,
        positive,
    }
}

/// Everything about a riven except the analyzer's judgement, which needs the
/// wanted-stats database. Stats only exist once a riven is unlocked.
fn describe(riven: &BlobRivenEntry, weapon_name: Option<String>, disposition: f32) -> GradedRiven {
    let category = riven_stats::riven_category(&riven.item_type);
    let unlocked = riven.riven_state == RivenState::Unlocked;
    let ctx = RollContext {
        category,
        disposition: f64::from(disposition),
        num_buffs: riven.buffs.len(),
        num_curses: riven.curses.len(),
        rank: riven.mod_rank,
    };
    let grade = |stats: &[BlobRivenStat], positive: bool| -> Vec<GradedStat> {
        if !unlocked {
            return Vec::new();
        }
        stats.iter().map(|s| graded_stat(s, positive, ctx)).collect()
    };

    GradedRiven {
        item_id: riven.item_id.clone(),
        item_type: riven.item_type.clone(),
        state: riven.riven_state.clone(),
        compat: riven.compat.clone(),
        weapon_name,
        category: category.to_string(),
        polarity: riven.polarity.clone(),
        mastery_level: riven.lvl_req,
        mod_rank: riven.mod_rank,
        rerolls: riven.rerolls,
        count: riven.count,
        mod_name: riven.mod_name.clone(),
        disposition,
        buffs: grade(&riven.buffs, true),
        curses: grade(&riven.curses, false),
        analysis: None,
        challenge: riven.challenge_type.clone(),
        complication: riven.challenge_complication.clone(),
    }
}

/// Grade one riven. `analysis` stays `None` when the roll cannot be judged: an
/// unrevealed or revealed-but-locked riven has no stats yet, and an unlocked
/// riven whose weapon the wanted-stats database does not cover has no scale.
pub fn grade_riven(riven: &BlobRivenEntry, weapon_name: Option<String>, disposition: f32) -> GradedRiven {
    let mut graded = describe(riven, weapon_name, disposition);
    if graded.state == RivenState::Unlocked {
        if let Some(weapon) = graded.weapon_name.clone() {
            let (positives, negatives) = graded.analyzer_inputs();
            graded.analysis = crate::analyze_riven(weapon, positives, negatives);
        }
    }
    graded
}

/// Grades every riven in the persisted inventory cache. Needs no network: the
/// scanner's last known state and the cached dispositions are enough.
#[tauri::command]
pub async fn grade_owned_rivens(state: tauri::State<'_, AppState>) -> Result<Vec<GradedRiven>, String> {
    let path = state.inventory_state_cache_path.clone();
    let dispositions = state.weapon_dispositions.lock().unwrap_or_else(|e| e.into_inner()).clone();

    tauri::async_runtime::spawn_blocking(move || {
        let cache = crate::load_inventory_state_cache(&path);
        cache.rivens.iter()
            .map(|riven| {
                let compat = riven.compat.as_deref();
                let weapon_name = compat
                    .and_then(|c| cache.items.get(c))
                    .map(|item| item.name.clone())
                    .filter(|name| !name.is_empty());
                let disposition = compat.and_then(|c| dispositions.get(c).copied()).unwrap_or(1.0);
                grade_riven(riven, weapon_name, disposition)
            })
            .collect()
    })
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unlocked_rifle_riven() -> BlobRivenEntry {
        BlobRivenEntry {
            item_id: "abc123".into(),
            item_type: "/Lotus/Upgrades/Mods/Randomized/LotusRifleRandomModRare".into(),
            riven_state: RivenState::Unlocked,
            compat: Some("/Lotus/Weapons/Tenno/Rifle/BurstRifle".into()),
            challenge_type: None,
            challenge_complication: None,
            lvl_req: Some(12),
            polarity: Some("AP_ATTACK".into()),
            buffs: vec![
                BlobRivenStat { tag: "WeaponDamageAmountMod".into(), value: 0x3FFF_FFFF },
                BlobRivenStat { tag: "WeaponFireRateMod".into(), value: 0x1FFF_FFFF },
            ],
            curses: vec![BlobRivenStat { tag: "WeaponCritChanceMod".into(), value: 0x1FFF_FFFF }],
            mod_rank: 8,
            count: 1,
            rerolls: 3,
            mod_name: "visitin".into(),
        }
    }

    /// The analyzer reads its wanted stats from a process-wide database that is
    /// normally loaded from the riven sheet.
    fn seed_riven_db() {
        let entry = crate::RivenEntry {
            weapon: "Braton".into(),
            stat_alternatives: vec![vec![vec!["Base Damage".into()], vec!["Fire Rate".into()]]],
            stat_groups: vec![vec!["Base Damage".into()], vec!["Fire Rate".into()]],
            safe_negatives: Vec::new(),
            notes: String::new(),
        };
        let mut db = std::collections::HashMap::new();
        db.insert("braton".to_string(), entry);
        *crate::RIVEN_DB.write().unwrap_or_else(|e| e.into_inner()) = Some(db);
    }

    #[test]
    fn a_covered_weapon_is_scored_and_its_rolls_placed_in_range() {
        seed_riven_db();
        let graded = grade_riven(&unlocked_rifle_riven(), Some("Braton".into()), 1.0);

        let analysis = graded.analysis.expect("the seeded weapon is covered");
        assert_eq!(analysis.matched_positives, vec!["Base Damage", "Fire Rate"]);
        assert!(analysis.missing_positives.is_empty());
        assert_eq!(analysis.safe_negatives_present, vec!["Critical Chance"]);
        assert!(analysis.verdict.starts_with("GREAT"), "verdict was {}", analysis.verdict);
        assert!((analysis.score - 1.0).abs() < 1e-9);

        // A maximum roll sits at the top of its range, a half roll in the middle.
        assert!((graded.buffs[0].position - 1.0).abs() < 1e-9);
        assert!((graded.buffs[1].position - 0.5).abs() < 1e-6);
        assert_eq!(graded.buffs[0].display, graded.buffs[0].max_display);
    }

    #[test]
    fn an_unlocked_riven_carries_its_stats_with_ranges_and_analyzer_names() {
        let graded = describe(&unlocked_rifle_riven(), Some("Braton".into()), 1.0);

        assert_eq!(graded.category, "Rifle");
        assert_eq!(graded.mastery_level, Some(12));
        assert_eq!(graded.rerolls, 3);
        assert_eq!(graded.buffs.len(), 2);
        assert_eq!(graded.curses.len(), 1);

        let damage = &graded.buffs[0];
        assert_eq!(damage.analyzer_name.as_deref(), Some("Base Damage"));
        assert_eq!(damage.display, "+224.6%");
        assert!(damage.min < damage.value && (damage.value - damage.max).abs() < 1e-9);
        assert!((damage.position - 1.0).abs() < 1e-9);

        assert_eq!(graded.buffs[1].analyzer_name.as_deref(), Some("Fire Rate"));
        assert!(graded.curses[0].value < 0.0);
        assert!(!graded.curses[0].positive);
    }

    #[test]
    fn a_melee_riven_reads_the_shared_fire_rate_tag_as_attack_speed() {
        let mut riven = unlocked_rifle_riven();
        riven.item_type = "/Lotus/Upgrades/Mods/Randomized/LotusMeleeRandomModRare".into();
        let graded = describe(&riven, Some("Skana".into()), 1.0);

        assert_eq!(graded.category, "Melee");
        assert_eq!(graded.buffs[1].analyzer_name.as_deref(), Some("Attack Speed"));
    }

    #[test]
    fn unmapped_tags_are_shown_but_kept_out_of_the_analyzer_call() {
        let mut riven = unlocked_rifle_riven();
        riven.item_type = "/Lotus/Upgrades/Mods/Randomized/LotusMeleeRandomModRare".into();
        riven.buffs[1].tag = "ComboDurationMod".into();
        let graded = describe(&riven, Some("Skana".into()), 1.0);

        assert_eq!(graded.buffs[1].label, "Combo Duration");
        assert_eq!(graded.buffs[1].analyzer_name, None);

        let (positives, negatives) = graded.analyzer_inputs();
        assert_eq!(positives, vec!["Base Damage"]);
        assert_eq!(negatives, vec!["Critical Chance"]);
    }

    #[test]
    fn a_riven_the_scanner_could_not_name_the_weapon_for_is_never_graded() {
        let graded = grade_riven(&unlocked_rifle_riven(), None, 1.0);
        assert!(graded.analysis.is_none());
        assert_eq!(graded.weapon_name, None);
    }

    #[test]
    fn an_unrevealed_riven_has_no_stats_and_no_grade() {
        let mut riven = unlocked_rifle_riven();
        riven.riven_state = RivenState::Unrevealed;
        riven.compat = None;
        riven.count = 4;

        let graded = grade_riven(&riven, None, 1.0);
        assert!(graded.buffs.is_empty() && graded.curses.is_empty());
        assert!(graded.analysis.is_none());
        assert_eq!(graded.count, 4);
    }

    #[test]
    fn a_revealed_riven_keeps_its_challenge_and_stays_ungraded() {
        let mut riven = unlocked_rifle_riven();
        riven.riven_state = RivenState::Revealed;
        riven.compat = None;
        riven.challenge_type = Some("/Lotus/Types/Challenges/RandomizedHeadshot".into());

        let graded = grade_riven(&riven, None, 1.0);
        assert!(graded.buffs.is_empty());
        assert!(graded.analysis.is_none());
        assert_eq!(graded.challenge.as_deref(), Some("/Lotus/Types/Challenges/RandomizedHeadshot"));
    }

    #[test]
    fn a_graded_riven_serializes_under_the_names_the_frontend_reads() {
        let graded = describe(&unlocked_rifle_riven(), Some("Braton".into()), 1.15);
        let json = serde_json::to_value(&graded).expect("GradedRiven derives Serialize");

        assert_eq!(json["state"], "unlocked");
        assert_eq!(json["weapon_name"], "Braton");
        assert_eq!(json["mastery_level"], 12);
        assert_eq!(json["analysis"], serde_json::Value::Null);
        assert_eq!(json["buffs"][0]["unit"], "%");
        assert_eq!(json["buffs"][0]["analyzer_name"], "Base Damage");
        assert_eq!(json["buffs"][0]["positive"], true);
        assert!(json["disposition"].as_f64().is_some());
    }
}
