//! Riven auction search: a buyer's filters in, graded market listings out.
//!
//! The market and the game describe the same roll differently, and this module
//! is where the two are reconciled so an auction can be graded on exactly the
//! scale the owned list uses.
//!
//! The game stores a stat as a tag plus a 30-bit roll; warframe.market publishes
//! it as an attribute `url_name` plus the finished number a player reads on the
//! mod (`9.5` for +9.5%, `1.07` for a ×1.07 faction multiplier). Grading needs
//! the game's form, so each listed attribute is converted back:
//!
//!   * the attribute `url_name` picks the stat tag — melee and ranged share
//!     several `url_name`s, so the weapon's category breaks the tie;
//!   * the number is put back into the stat's own unit, then placed in the range
//!     that stat could have rolled in on that weapon at that rank, and the roll
//!     position that lands on it is the 30-bit value. The value is linear in the
//!     roll, so interpolating between the range ends inverts the formula without
//!     restating it.
//!
//! The reconstruction is faithful to the tenth of a percent the market publishes,
//! which is the precision the market itself shows; the original roll's remaining
//! digits are not recoverable and do not change a grade.
//!
//! A weapon the market names by slug is looked up in the item list to recover its
//! `unique_name` (for the disposition) and its category. Rivens do not carry the
//! weapon category on the listing, so the riven mod path the grader reads it from
//! is synthesized from the weapon's type.

use std::collections::HashMap;

use crate::memory_scanner::{BlobRivenEntry, BlobRivenStat, RivenState};
use crate::riven_grade::{grade_riven, GradedRiven};
use crate::riven_stats::{self, AnalyzerName, RollContext, Unit, RIVEN_STATS, ROLL_MAX};
use crate::wfm;
use crate::AppState;

// ==============================================================================
// Stat name bridging
// ==============================================================================
//
// Stat tag → warframe.market v1 attribute `url_name`. Taken from live auction
// data: WFM combines the melee and ranged spelling of a stat under one name,
// using the "_/_" separator.

static WFM_ATTR: &[(&str, &str)] = &[
    // Melee / Zaw.
    ("WeaponMeleeDamageMod", "base_damage_/_melee_damage"),
    ("WeaponMeleeRangeIncMod", "range"),
    // WFM still uses the pre-rework "channeling" name for heavy attacks.
    ("WeaponMeleeComboEfficiencyMod", "channeling_efficiency"),
    ("WeaponMeleeFinisherDamageMod", "finisher_damage"),
    ("WeaponMeleeComboInitialBonusMod", "initial_combo"),
    ("WeaponMeleeComboBonusOnHitMod", "chance_to_gain_extra_combo_count"),
    ("WeaponMeleeComboPointsOnHitMod", "chance_to_gain_combo_count"),
    ("SlideAttackCritChanceMod", "critical_chance_on_slide_attack"),
    ("ComboDurationMod", "combo_duration"),
    ("WeaponMeleeFactionDamageCorpus", "damage_vs_corpus"),
    ("WeaponMeleeFactionDamageGrineer", "damage_vs_grineer"),
    ("WeaponMeleeFactionDamageInfested", "damage_vs_infested"),
    // Ranged.
    ("WeaponDamageAmountMod", "base_damage_/_melee_damage"),
    ("WeaponFireIterationsMod", "multishot"),
    ("WeaponAmmoMaxMod", "ammo_maximum"),
    ("WeaponClipMaxMod", "magazine_capacity"),
    ("WeaponReloadSpeedMod", "reload_speed"),
    ("WeaponProjectileSpeedMod", "projectile_speed"),
    ("WeaponZoomFovMod", "zoom"),
    ("WeaponPunctureDepthMod", "punch_through"),
    ("WeaponRecoilReductionMod", "recoil"),
    ("WeaponFactionDamageCorpus", "damage_vs_corpus"),
    ("WeaponFactionDamageGrineer", "damage_vs_grineer"),
    ("WeaponFactionDamageInfested", "damage_vs_infested"),
    // Every weapon.
    ("WeaponCritChanceMod", "critical_chance"),
    ("WeaponCritDamageMod", "critical_damage"),
    ("WeaponArmorPiercingDamageMod", "puncture_damage"),
    ("WeaponImpactDamageMod", "impact_damage"),
    ("WeaponSlashDamageMod", "slash_damage"),
    ("WeaponElectricityDamageMod", "electricity_damage"),
    ("WeaponFireDamageMod", "heat_damage"),
    ("WeaponFreezeDamageMod", "cold_damage"),
    ("WeaponToxinDamageMod", "toxin_damage"),
    ("WeaponFireRateMod", "fire_rate_/_attack_speed"),
    ("WeaponProcTimeMod", "status_duration"),
    ("WeaponStunChanceMod", "status_chance"),
];

fn wfm_attr(tag: &str) -> Option<&'static str> {
    WFM_ATTR.iter().find(|(t, _)| *t == tag).map(|(_, a)| *a)
}

/// The market attribute a buyer's analyzer stat name asks for. Where two tags
/// carry the same analyzer name (melee and ranged damage, faction damage) they
/// also share a market attribute, so either tag answers.
fn attr_for_analyzer_name(name: &str) -> Option<&'static str> {
    RIVEN_STATS
        .iter()
        .find(|(_, def)| match def.analyzer {
            AnalyzerName::Fixed(n) => n == name,
            AnalyzerName::PerCategory { melee, ranged } => melee == name || ranged == name,
            AnalyzerName::Unmapped => false,
        })
        .and_then(|(tag, _)| wfm_attr(tag))
}

/// The stat tag a listed attribute means on this weapon. Melee weapons take the
/// melee spelling of a shared attribute, everything else the ranged one.
fn tag_for_attr(attr: &str, category: &str) -> Option<&'static str> {
    let melee = matches!(category, "Melee" | "Zaw");
    WFM_ATTR
        .iter()
        .filter(|(_, a)| *a == attr)
        .map(|(tag, _)| *tag)
        .find(|tag| tag.contains("Melee") == melee)
        // A stat that exists in only one spelling is used whatever the weapon.
        .or_else(|| WFM_ATTR.iter().find(|(_, a)| *a == attr).map(|(tag, _)| *tag))
}

// ==============================================================================
// Query building
// ==============================================================================

/// What the user is looking for. Every field beyond the weapon narrows the search.
#[derive(serde::Deserialize, Default)]
pub struct AuctionQuery {
    /// Weapon display name. Empty searches every weapon.
    pub weapon: String,
    pub positive_stats: Vec<String>,
    pub negative_stat: Option<String>,
    pub polarity: Option<String>,
    pub mastery_min: Option<u32>,
    pub mastery_max: Option<u32>,
    pub rerolls_min: Option<u32>,
    pub rerolls_max: Option<u32>,
    pub buyout_only: bool,
}

fn build_query(q: &AuctionQuery) -> Vec<(String, String)> {
    let mut params = vec![("type".to_string(), "riven".to_string())];
    if !q.weapon.trim().is_empty() {
        params.push(("weapon_url_name".into(), wfm::to_wfm_slug(q.weapon.trim())));
    }
    let positives: Vec<&str> = q.positive_stats.iter().filter_map(|s| attr_for_analyzer_name(s)).collect();
    if !positives.is_empty() {
        params.push(("positive_stats".into(), positives.join(",")));
    }
    if let Some(attr) = q.negative_stat.as_deref().and_then(attr_for_analyzer_name) {
        params.push(("negative_stats".into(), attr.to_string()));
    }
    if let Some(polarity) = q.polarity.as_deref().filter(|p| !p.is_empty()) {
        params.push(("polarity".into(), polarity.to_string()));
    }
    for (key, value) in [
        ("mastery_rank_min", q.mastery_min),
        ("mastery_rank_max", q.mastery_max),
        ("re_rolls_min", q.rerolls_min),
        ("re_rolls_max", q.rerolls_max),
    ] {
        if let Some(v) = value {
            params.push((key.into(), v.to_string()));
        }
    }
    // "with" is WFM's name for "has a buyout price", as against "direct", which
    // would also drop auctions that can be bought outright but take bids.
    if q.buyout_only {
        params.push(("buyout_policy".into(), "with".into()));
    }
    params
}

// ==============================================================================
// Normalization
// ==============================================================================

/// One auction, normalized to the riven shape the owned list uses.
#[derive(serde::Serialize)]
pub struct GradedAuction {
    pub auction_id: String,
    pub url: String,
    pub starting_price: Option<f64>,
    pub buyout_price: Option<f64>,
    pub top_bid: Option<f64>,
    pub seller_name: String,
    pub seller_status: String,
    pub seller_reputation: i64,
    pub riven: GradedRiven,
}

/// What the item list knows about a weapon the market names by slug.
#[derive(Clone)]
struct Weapon {
    name: String,
    unique_name: String,
    /// Riven mod path, from which the grader reads the weapon's category.
    item_type: &'static str,
}

/// The riven mod path a weapon's rivens carry. Kitguns draw pistol caps and zaws
/// melee caps, which is what their own categories resolve to.
fn riven_item_type(item_type: &str, product_category: &str) -> &'static str {
    const RIFLE: &str = "/Lotus/Upgrades/Mods/Randomized/LotusRifleRandomModRare";
    const SHOTGUN: &str = "/Lotus/Upgrades/Mods/Randomized/LotusShotgunRandomModRare";
    const PISTOL: &str = "/Lotus/Upgrades/Mods/Randomized/LotusPistolRandomModRare";
    const MELEE: &str = "/Lotus/Upgrades/Mods/Randomized/LotusMeleeRandomModRare";
    const ARCHGUN: &str = "/Lotus/Upgrades/Mods/Randomized/LotusArchgunRandomModRare";
    const ZAW: &str = "/Lotus/Upgrades/Mods/Randomized/LotusModularMeleeZawRandomModRare";

    match item_type {
        "Shotgun" => SHOTGUN,
        "Pistol" | "Dual Pistols" | "Kitgun Component" => PISTOL,
        "Melee" => MELEE,
        "Zaw Component" => ZAW,
        "Arch-Gun" | "Arch-Melee" => ARCHGUN,
        _ => match product_category {
            "Pistols" => PISTOL,
            "Melee" => MELEE,
            "SpaceGuns" | "SpaceMelee" => ARCHGUN,
            // Bows, snipers and launchers all roll rifle rivens.
            _ => RIFLE,
        },
    }
}

/// The 30-bit roll that produces `published`, the value the market shows for this
/// stat. The value is linear in the roll, so the stat's own range at this rank
/// gives the position without repeating the formula that built it.
fn roll_for_value(tag: &str, positive: bool, ctx: RollContext, published: f64) -> i64 {
    let range = riven_stats::evaluate(tag, 0, positive, ctx);
    let value = match range.unit {
        Unit::Percent => published / 100.0,
        // Faction damage is published as the whole multiplier, e.g. 1.07.
        Unit::Multiplier => published.abs() - 1.0,
        Unit::Metres | Unit::Seconds | Unit::Flat => published,
    };
    let span = range.max - range.min;
    if span == 0.0 {
        return 0;
    }
    let position = ((value - range.min) / span).clamp(0.0, 1.0);
    (position * ROLL_MAX).round() as i64
}

/// Read the listed attributes as game stats. An attribute name the tag table does
/// not know is kept under its own name rather than dropped: it still shows on the
/// listing, and dropping it would misreport how many stats the riven carries,
/// which is what the other stats' ranges are scaled by.
fn stats_from_attributes(
    attributes: &[serde_json::Value],
    category: &str,
    ctx: RollContext,
) -> (Vec<BlobRivenStat>, Vec<BlobRivenStat>) {
    let mut buffs = Vec::new();
    let mut curses = Vec::new();
    for attr in attributes {
        let Some(url_name) = attr["url_name"].as_str() else { continue };
        let positive = attr["positive"].as_bool().unwrap_or(true);
        let tag = tag_for_attr(url_name, category).unwrap_or(url_name);
        let stat = BlobRivenStat {
            tag: tag.to_string(),
            value: roll_for_value(tag, positive, ctx, attr["value"].as_f64().unwrap_or(0.0)),
        };
        if positive {
            buffs.push(stat);
        } else {
            curses.push(stat);
        }
    }
    (buffs, curses)
}

/// Normalize and grade one listing. `None` for anything that is not a riven
/// auction with an id — a listing shaped differently from the rest is skipped
/// rather than failing the search.
fn normalize(
    auction: &serde_json::Value,
    weapons: &HashMap<String, Weapon>,
    dispositions: &HashMap<String, f32>,
) -> Option<GradedAuction> {
    let auction_id = auction["id"].as_str()?.to_string();
    let item = &auction["item"];
    let slug = item["weapon_url_name"].as_str().unwrap_or_default();
    let weapon = weapons.get(slug);
    let item_type = weapon.map_or(
        "/Lotus/Upgrades/Mods/Randomized/LotusRifleRandomModRare",
        |w| w.item_type,
    );
    let disposition = weapon
        .and_then(|w| dispositions.get(&w.unique_name).copied())
        .unwrap_or(1.0);

    let attributes = item["attributes"].as_array().cloned().unwrap_or_default();
    let num_curses = attributes.iter().filter(|a| a["positive"].as_bool() == Some(false)).count();
    let mod_rank = item["mod_rank"].as_u64().unwrap_or(0) as u8;
    let category = riven_stats::riven_category(item_type);
    let ctx = RollContext {
        category,
        disposition: f64::from(disposition),
        num_buffs: attributes.len() - num_curses,
        num_curses,
        rank: mod_rank,
    };
    // Every stat value depends on the weapon's type and disposition. Without a
    // weapon both are guesses, and a melee roll read on the rifle scale prints a
    // wrong number as if it were fact — so the listing shows no stats at all.
    let (buffs, curses) = match weapon {
        Some(_) => stats_from_attributes(&attributes, category, ctx),
        None => (Vec::new(), Vec::new()),
    };

    let entry = BlobRivenEntry {
        item_id: auction_id.clone(),
        item_type: item_type.to_string(),
        riven_state: RivenState::Unlocked,
        compat: weapon.map(|w| w.unique_name.clone()),
        challenge_type: None,
        challenge_complication: None,
        lvl_req: item["mastery_level"].as_u64().map(|v| v as u32),
        // The market's own polarity names, which is what the listing shows.
        polarity: item["polarity"].as_str().map(str::to_string),
        buffs,
        curses,
        mod_rank,
        count: 1,
        rerolls: item["re_rolls"].as_u64().unwrap_or(0) as u32,
        mod_name: item["name"].as_str().unwrap_or_default().to_string(),
    };

    let owner = &auction["owner"];
    Some(GradedAuction {
        url: format!("https://warframe.market/auction/{auction_id}"),
        auction_id,
        starting_price: auction["starting_price"].as_f64(),
        buyout_price: auction["buyout_price"].as_f64(),
        top_bid: auction["top_bid"].as_f64(),
        seller_name: owner["ingame_name"].as_str().unwrap_or_default().to_string(),
        seller_status: owner["status"].as_str().unwrap_or("offline").to_string(),
        seller_reputation: owner["reputation"].as_i64().unwrap_or(0),
        riven: grade_riven(&entry, weapon.map(|w| w.name.clone()), disposition),
    })
}

// ==============================================================================
// Command
// ==============================================================================

/// Search warframe.market for rivens matching `query`, graded on the same scale
/// as the owned list. No result is an empty list, never an error: the frontend
/// tells "nothing matched" and "the market is unreachable" apart.
#[tauri::command]
pub async fn search_riven_auctions(
    state: tauri::State<'_, AppState>,
    query: AuctionQuery,
) -> Result<Vec<GradedAuction>, String> {
    let wfm = state.wfm.clone();
    let dispositions = state.weapon_dispositions.lock().unwrap_or_else(|e| e.into_inner()).clone();
    // Only weapons carry a disposition, so that is also the filter that keeps
    // the slug map to the items rivens can exist for.
    let weapons: HashMap<String, Weapon> = state
        .wfcd_items
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|i| i.omega_attenuation.is_some())
        .map(|i| {
            (
                wfm::to_wfm_slug(&i.name),
                Weapon {
                    name: i.name.clone(),
                    unique_name: i.unique_name.clone(),
                    item_type: riven_item_type(&i.item_type, &i.product_category),
                },
            )
        })
        .collect();
    let params = build_query(&query);

    // The market call blocks, and so does the analyzer's first look at the
    // wanted-stats sheet.
    tauri::async_runtime::spawn_blocking(move || {
        let response = wfm.search_riven_auctions(&params)?;
        let listings = response["payload"]["auctions"].as_array().cloned().unwrap_or_default();
        Ok(listings.iter().filter_map(|a| normalize(a, &weapons, &dispositions)).collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Grading reads the wanted-stats database, which fetches a Google Sheet on
    /// first use. Seeding it keeps the tests off the network and their verdicts
    /// independent of what the sheet currently says.
    fn seed_riven_db() {
        let entry = crate::RivenEntry {
            weapon: "Braton Prime".into(),
            stat_alternatives: vec![vec![vec!["Critical Damage".into()], vec!["Multishot".into()]]],
            stat_groups: vec![vec!["Critical Damage".into()], vec!["Multishot".into()]],
            safe_negatives: Vec::new(),
            notes: String::new(),
        };
        let mut db = std::collections::HashMap::new();
        db.insert("braton prime".to_string(), entry);
        *crate::RIVEN_DB.write().unwrap_or_else(|e| e.into_inner()) = Some(db);
    }

    fn weapons() -> HashMap<String, Weapon> {
        HashMap::from([(
            "braton_prime".to_string(),
            Weapon {
                name: "Braton Prime".into(),
                unique_name: "/Lotus/Weapons/Tenno/Rifle/BratonPrime".into(),
                item_type: "/Lotus/Upgrades/Mods/Randomized/LotusRifleRandomModRare",
            },
        )])
    }

    fn dispositions() -> HashMap<String, f32> {
        HashMap::from([("/Lotus/Weapons/Tenno/Rifle/BratonPrime".to_string(), 1.0)])
    }

    fn param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
        params.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    #[test]
    fn an_empty_query_asks_only_for_riven_auctions() {
        let params = build_query(&AuctionQuery::default());
        assert_eq!(params, vec![("type".to_string(), "riven".to_string())]);
    }

    #[test]
    fn every_filter_reaches_the_request_under_the_markets_own_parameter_names() {
        let params = build_query(&AuctionQuery {
            weapon: "Braton Prime".into(),
            positive_stats: vec!["Critical Damage".into(), "Multishot".into()],
            negative_stat: Some("Zoom".into()),
            polarity: Some("madurai".into()),
            mastery_min: Some(8),
            mastery_max: Some(16),
            rerolls_min: Some(0),
            rerolls_max: Some(5),
            buyout_only: true,
        });

        assert_eq!(param(&params, "weapon_url_name"), Some("braton_prime"));
        assert_eq!(param(&params, "positive_stats"), Some("critical_damage,multishot"));
        assert_eq!(param(&params, "negative_stats"), Some("zoom"));
        assert_eq!(param(&params, "polarity"), Some("madurai"));
        assert_eq!(param(&params, "mastery_rank_min"), Some("8"));
        assert_eq!(param(&params, "mastery_rank_max"), Some("16"));
        assert_eq!(param(&params, "re_rolls_min"), Some("0"));
        assert_eq!(param(&params, "re_rolls_max"), Some("5"));
        assert_eq!(param(&params, "buyout_policy"), Some("with"));
    }

    #[test]
    fn a_stat_the_market_spells_for_both_melee_and_ranged_asks_under_one_name() {
        assert_eq!(attr_for_analyzer_name("Base Damage"), Some("base_damage_/_melee_damage"));
        assert_eq!(attr_for_analyzer_name("Attack Speed"), Some("fire_rate_/_attack_speed"));
        assert_eq!(attr_for_analyzer_name("Fire Rate"), Some("fire_rate_/_attack_speed"));
        assert_eq!(attr_for_analyzer_name("Damage to Corpus"), Some("damage_vs_corpus"));
        assert_eq!(attr_for_analyzer_name("Nonexistent Stat"), None);
    }

    #[test]
    fn a_shared_attribute_reads_as_the_spelling_the_weapon_can_roll() {
        assert_eq!(tag_for_attr("base_damage_/_melee_damage", "Rifle"), Some("WeaponDamageAmountMod"));
        assert_eq!(tag_for_attr("base_damage_/_melee_damage", "Melee"), Some("WeaponMeleeDamageMod"));
        assert_eq!(tag_for_attr("base_damage_/_melee_damage", "Zaw"), Some("WeaponMeleeDamageMod"));
        // Range only exists as a melee stat, so a ranged riven still resolves it.
        assert_eq!(tag_for_attr("range", "Rifle"), Some("WeaponMeleeRangeIncMod"));
        assert_eq!(tag_for_attr("critical_chance", "Rifle"), Some("WeaponCritChanceMod"));
        assert_eq!(tag_for_attr("invented_stat", "Rifle"), None);
    }

    #[test]
    fn every_tag_the_analyzer_scores_has_a_market_attribute() {
        for (tag, def) in RIVEN_STATS {
            if def.analyzer != AnalyzerName::Unmapped {
                assert!(wfm_attr(tag).is_some(), "{tag} has no warframe.market attribute");
            }
        }
    }

    /// A rank-8, two-buff one-curse Braton Prime riven, as /v1/auctions/search
    /// returns it.
    const LISTING: &str = r#"{
      "payload": { "auctions": [{
        "id": "62b1f0a0e0e1a20001a2b3c4",
        "starting_price": 40,
        "buyout_price": 120,
        "top_bid": 55,
        "is_direct_sell": false,
        "visible": true,
        "owner": { "ingame_name": "Ordis", "status": "ingame", "reputation": 17 },
        "item": {
          "type": "riven",
          "weapon_url_name": "braton_prime",
          "name": "visi-critacan",
          "mastery_level": 12,
          "mod_rank": 8,
          "re_rolls": 3,
          "polarity": "madurai",
          "attributes": [
            { "url_name": "critical_damage", "positive": true,  "value": 150.0 },
            { "url_name": "multishot",       "positive": true,  "value": 110.0 },
            { "url_name": "zoom",            "positive": false, "value": -30.0 }
          ]
        }
      }]}
    }"#;

    fn normalized(body: &str) -> Vec<GradedAuction> {
        seed_riven_db();
        let json: serde_json::Value = serde_json::from_str(body).expect("fixture is valid JSON");
        json["payload"]["auctions"]
            .as_array()
            .expect("fixture carries an auction array")
            .iter()
            .filter_map(|a| normalize(a, &weapons(), &dispositions()))
            .collect()
    }

    #[test]
    fn a_listing_becomes_a_graded_riven_with_its_auction_fields() {
        let results = normalized(LISTING);
        assert_eq!(results.len(), 1);
        let a = &results[0];

        assert_eq!(a.auction_id, "62b1f0a0e0e1a20001a2b3c4");
        assert_eq!(a.url, "https://warframe.market/auction/62b1f0a0e0e1a20001a2b3c4");
        assert_eq!(a.starting_price, Some(40.0));
        assert_eq!(a.buyout_price, Some(120.0));
        assert_eq!(a.top_bid, Some(55.0));
        assert_eq!(a.seller_name, "Ordis");
        assert_eq!(a.seller_status, "ingame");
        assert_eq!(a.seller_reputation, 17);

        let riven = &a.riven;
        assert_eq!(riven.weapon_name.as_deref(), Some("Braton Prime"));
        assert_eq!(riven.compat.as_deref(), Some("/Lotus/Weapons/Tenno/Rifle/BratonPrime"));
        assert_eq!(riven.category, "Rifle");
        assert_eq!(riven.mastery_level, Some(12));
        assert_eq!(riven.mod_rank, 8);
        assert_eq!(riven.rerolls, 3);
        assert_eq!(riven.mod_name, "visi-critacan");
        assert_eq!(riven.buffs.len(), 2);
        assert_eq!(riven.curses.len(), 1);
    }

    #[test]
    fn a_listed_value_survives_the_trip_through_the_game_representation() {
        let riven = &normalized(LISTING)[0].riven;

        // What the market published, back to the tenth of a percent it published it to.
        assert_eq!(riven.buffs[0].display, "+150.0%");
        assert_eq!(riven.buffs[1].display, "+110.0%");
        assert_eq!(riven.curses[0].display, "-30.0%");
        assert_eq!(riven.buffs[0].analyzer_name.as_deref(), Some("Critical Damage"));
        assert_eq!(riven.curses[0].tag, "WeaponZoomFovMod");
        assert!(riven.buffs[0].position > 0.0 && riven.buffs[0].position < 1.0);
    }

    #[test]
    fn a_listing_is_graded_on_the_same_scale_as_an_owned_riven() {
        let results = normalized(LISTING);
        let analysis = results[0].riven.analysis.as_ref().expect("the seeded weapon is covered");
        assert_eq!(analysis.matched_positives, vec!["Critical Damage", "Multishot"]);
        assert!(analysis.verdict.starts_with("GREAT"));
    }

    #[test]
    fn a_weapon_the_wanted_stats_database_does_not_cover_stays_ungraded() {
        let body = LISTING.replace("braton_prime", "unknown_weapon");
        let results = normalized(&body);
        assert_eq!(results.len(), 1);
        assert!(results[0].riven.analysis.is_none());
        assert_eq!(results[0].riven.weapon_name, None);
        // Nor are the stats shown: without the weapon there is no scale to read
        // the listed values against.
        assert!(results[0].riven.buffs.is_empty());
        assert!(results[0].riven.curses.is_empty());
    }

    #[test]
    fn a_search_that_matched_nothing_is_an_empty_list() {
        assert!(normalized(r#"{ "payload": { "auctions": [] } }"#).is_empty());
    }

    #[test]
    fn a_malformed_listing_is_skipped_without_taking_the_search_with_it() {
        let body = r#"{ "payload": { "auctions": [
          { "item": { "weapon_url_name": "braton_prime" } },
          { "id": "aaa", "owner": null, "item": {
              "weapon_url_name": "braton_prime",
              "mod_rank": "8",
              "attributes": [
                { "positive": true, "value": 10 },
                { "url_name": "critical_damage", "positive": "yes", "value": "150.0" }
              ]
          }},
          { "id": "bbb" }
        ]}}"#;
        let results = normalized(body);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].auction_id, "aaa");
        assert_eq!(results[0].seller_name, "");
        assert_eq!(results[0].seller_status, "offline");
        assert_eq!(results[0].starting_price, None);
        // A string where a number belongs reads as absent, not as a wrong roll.
        assert_eq!(results[0].riven.mod_rank, 0);
        assert_eq!(results[0].riven.buffs.len(), 1);
        assert_eq!(results[1].riven.buffs.len(), 0);
    }
}
