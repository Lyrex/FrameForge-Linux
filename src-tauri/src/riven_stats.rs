//! Riven stat values, ranges and names.
//!
//! The game stores a rolled stat as a 30-bit integer; the visible value depends
//! on the weapon's disposition, how many buffs and curses the riven carries and
//! its current rank. This is a port of calamity-inc/warframe-riven-info's
//! RivenParser.js, whose base caps come from that repo's riven_tags.json.
//!
//! Buff:  baseCap × (1.5 × omega × 10) × 1.25^numCurses × lerp(0.9,1.1,frac) × buffAtten[numBuffs] × (rank+1)
//! Curse: −baseCap × (1.5 × omega × 10) × lerp(0.9,1.1,frac) × curseAtten[numBuffs] × buffAtten[numCurses] × (rank+1)

/// Largest value the game's 30-bit roll field can hold.
pub const ROLL_MAX: f64 = 0x3FFF_FFFF as f64;

const BUFF_ATTEN: [f64; 6] = [0.0, 1.0, 0.66000003, 0.5, 0.40000001, 0.34999999];
const CURSE_ATTEN: [f64; 6] = [0.0, 1.0, 0.33000001, 0.5, 1.25, 1.5];

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub enum Unit {
    #[serde(rename = "%")]
    Percent,
    /// Faction damage, shown as a multiplier (×1.07) rather than a percentage.
    #[serde(rename = "x")]
    Multiplier,
    #[serde(rename = "m")]
    Metres,
    #[serde(rename = "s")]
    Seconds,
    #[serde(rename = "n")]
    Flat,
}

/// Which cap column a weapon draws from. Kitgun caps equal Pistol caps and Zaw
/// caps equal Melee caps, so those categories share a column.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RivenType {
    Archgun,
    Melee,
    Pistol,
    Rifle,
    Shotgun,
}

/// The analyzer's name for a stat, which is not always the display label.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnalyzerName {
    Fixed(&'static str),
    /// The same tag drives melee attack speed and ranged fire rate.
    PerCategory { melee: &'static str, ranged: &'static str },
    /// No analyzer stat corresponds to this tag, so it takes no part in a score.
    Unmapped,
}

pub struct StatDef {
    pub label: &'static str,
    pub unit: Unit,
    /// Base caps in [Archgun, Melee, Pistol, Rifle, Shotgun] order; `None` where
    /// the stat cannot roll on that weapon type.
    caps: [Option<f64>; 5],
    pub analyzer: AnalyzerName,
}

impl StatDef {
    fn cap(&self, rt: RivenType) -> Option<f64> {
        self.caps[rt as usize]
    }
}

const N: Option<f64> = None;
const fn c(v: f64) -> Option<f64> {
    Some(v)
}

const fn def(label: &'static str, unit: Unit, caps: [Option<f64>; 5], analyzer: AnalyzerName) -> StatDef {
    StatDef { label, unit, caps, analyzer }
}

use AnalyzerName::{Fixed, PerCategory, Unmapped};
use Unit::{Flat, Metres, Multiplier, Percent, Seconds};

/// Every stat tag a riven can carry, with its caps per weapon type.
/// Linear lookup — the table is small and read once per stat.
pub static RIVEN_STATS: &[(&str, StatDef)] = &[
    // Melee / Zaw only.                                    AG  ME             PL  RI  SG
    ("WeaponMeleeDamageMod",             def("Damage",               Percent,    [N, c(0.0183),   N, N, N], Fixed("Base Damage"))),
    ("WeaponMeleeRangeIncMod",           def("Range",                Metres,     [N, c(0.02158),  N, N, N], Fixed("Range"))),
    ("WeaponMeleeComboEfficiencyMod",    def("Heavy Atk Efficiency", Percent,    [N, c(0.00816),  N, N, N], Fixed("Heavy Attack Efficiency"))),
    ("WeaponMeleeFinisherDamageMod",     def("Finisher Damage",      Percent,    [N, c(0.0133),   N, N, N], Unmapped)),
    ("WeaponMeleeComboInitialBonusMod",  def("Initial Combo",        Flat,       [N, c(0.27224),  N, N, N], Fixed("Initial Combo"))),
    ("WeaponMeleeComboBonusOnHitMod",    def("Combo Count Chance",   Percent,    [N, c(0.00653),  N, N, N], Fixed("Combo Count Chance"))),
    ("WeaponMeleeComboPointsOnHitMod",   def("Combo Count Chance",   Percent,    [N, c(-0.01165), N, N, N], Fixed("Combo Count Chance"))),
    ("SlideAttackCritChanceMod",         def("Slide Crit Chance",    Percent,    [N, c(0.013334), N, N, N], Fixed("Slide Critical Chance"))),
    ("ComboDurationMod",                 def("Combo Duration",       Seconds,    [N, c(0.09),     N, N, N], Unmapped)),
    ("WeaponMeleeFactionDamageCorpus",   def("Damage to Corpus",     Multiplier, [N, c(0.005),    N, N, N], Fixed("Damage to Corpus"))),
    ("WeaponMeleeFactionDamageGrineer",  def("Damage to Grineer",    Multiplier, [N, c(0.005),    N, N, N], Fixed("Damage to Grineer"))),
    ("WeaponMeleeFactionDamageInfested", def("Damage to Infested",   Multiplier, [N, c(0.005),    N, N, N], Fixed("Damage to Infested"))),
    // Ranged only.
    ("WeaponDamageAmountMod",    def("Damage",            Percent, [c(0.0111),   N, c(0.0244),    c(0.018333),  c(0.0183)],   Fixed("Base Damage"))),
    ("WeaponFireIterationsMod",  def("Multishot",         Percent, [c(0.0067),   N, c(0.0133),    c(0.01),      c(0.0133)],   Fixed("Multishot"))),
    ("WeaponAmmoMaxMod",         def("Ammo Maximum",      Percent, [c(0.0111),   N, c(0.01),      c(0.00555),   c(0.01)],     Fixed("Ammo Maximum"))),
    ("WeaponClipMaxMod",         def("Magazine Capacity", Percent, [c(0.0067),   N, c(0.005555),  c(0.005555),  c(0.005555)], Fixed("Magazine Size"))),
    ("WeaponReloadSpeedMod",     def("Reload Speed",      Percent, [c(0.0111),   N, c(0.005555),  c(0.005555),  c(0.005555)], Fixed("Reload Speed"))),
    ("WeaponProjectileSpeedMod", def("Projectile Speed",  Percent, [N,           N, c(0.01),      c(0.01),      c(0.01)],     Fixed("Projectile Flight Speed"))),
    ("WeaponZoomFovMod",         def("Zoom",              Percent, [c(0.006666), N, c(0.0089),    c(0.006666),  N],           Fixed("Zoom"))),
    ("WeaponPunctureDepthMod",   def("Punch Through",     Metres,  [c(0.03),     N, c(0.03),      c(0.03),      c(0.03)],     Fixed("Punch Through"))),
    ("WeaponRecoilReductionMod", def("Recoil",            Percent, [c(-0.01),    N, c(-0.01),     c(-0.01),     c(-0.01)],    Fixed("Recoil"))),
    ("WeaponFactionDamageCorpus",   def("Damage to Corpus",   Multiplier, [c(0.005), N, c(0.005), c(0.005), c(0.005)], Fixed("Damage to Corpus"))),
    ("WeaponFactionDamageGrineer",  def("Damage to Grineer",  Multiplier, [c(0.005), N, c(0.005), c(0.005), c(0.005)], Fixed("Damage to Grineer"))),
    ("WeaponFactionDamageInfested", def("Damage to Infested", Multiplier, [N,        N, c(0.005), c(0.005), c(0.005)], Fixed("Damage to Infested"))),
    // Every weapon.
    ("WeaponCritChanceMod",          def("Critical Chance",     Percent, [c(0.0111),  c(0.02),    c(0.016666), c(0.016666), c(0.01)],     Fixed("Critical Chance"))),
    ("WeaponCritDamageMod",          def("Critical Damage",     Percent, [c(0.0089),  c(0.01),    c(0.01),     c(0.013333), c(0.01)],     Fixed("Critical Damage"))),
    ("WeaponArmorPiercingDamageMod", def("Puncture",            Percent, [c(0.01),    c(0.0133),  c(0.01333),  c(0.01333),  c(0.01333)],  Fixed("Puncture"))),
    ("WeaponImpactDamageMod",        def("Impact",              Percent, [c(0.01),    c(0.0133),  c(0.013333), c(0.013333), c(0.013333)], Fixed("Impact"))),
    ("WeaponSlashDamageMod",         def("Slash",               Percent, [c(0.01),    c(0.0133),  c(0.013333), c(0.013333), c(0.013333)], Fixed("Slash"))),
    ("WeaponElectricityDamageMod",   def("Electricity",         Percent, [c(0.0133),  c(0.01),    c(0.01),     c(0.01),     c(0.01)],     Fixed("Electricity"))),
    ("WeaponFireDamageMod",          def("Heat",                Percent, [c(0.0133),  c(0.01),    c(0.01),     c(0.01),     c(0.01)],     Fixed("Heat"))),
    ("WeaponFreezeDamageMod",        def("Cold",                Percent, [c(0.0133),  c(0.01),    c(0.01),     c(0.01),     c(0.01)],     Fixed("Cold"))),
    ("WeaponToxinDamageMod",         def("Toxin",               Percent, [c(0.0133),  c(0.01),    c(0.01),     c(0.01),     c(0.01)],     Fixed("Toxicity"))),
    ("WeaponFireRateMod",            def("Fire Rate / Atk Spd", Percent, [c(0.00667), c(0.0061),  c(0.0083),   c(0.00667),  c(0.01)],     PerCategory { melee: "Attack Speed", ranged: "Fire Rate" })),
    ("WeaponProcTimeMod",            def("Status Duration",     Percent, [c(0.01111), c(0.01111), c(0.01111),  c(0.01111),  c(0.01111)],  Fixed("Status Duration"))),
    ("WeaponStunChanceMod",          def("Status Chance",       Percent, [c(0.0067),  c(0.01),    c(0.01),     c(0.01),     c(0.01)],     Fixed("Status Chance"))),
];

pub fn stat_def(tag: &str) -> Option<&'static StatDef> {
    RIVEN_STATS.iter().find(|(t, _)| *t == tag).map(|(_, d)| d)
}

/// The weapon category a riven's item type belongs to, as the UI names it.
pub fn riven_category(item_type: &str) -> &'static str {
    if item_type.contains("Melee") {
        "Melee"
    } else if item_type.contains("Rifle") {
        "Rifle"
    } else if item_type.contains("Pistol") || item_type.contains("Kitgun") {
        "Pistol"
    } else if item_type.contains("Shotgun") {
        "Shotgun"
    } else if item_type.contains("Archgun") || item_type.contains("Archwing") {
        "Arch-gun"
    } else if item_type.contains("Zaw") {
        "Zaw"
    } else {
        "Riven"
    }
}

pub fn riven_type(category: &str) -> RivenType {
    match category {
        "Arch-gun" => RivenType::Archgun,
        "Melee" | "Zaw" => RivenType::Melee,
        "Shotgun" => RivenType::Shotgun,
        "Rifle" => RivenType::Rifle,
        // Pistol, Kitgun and anything unrecognised.
        _ => RivenType::Pistol,
    }
}

/// The analyzer's name for a tag, or `None` when the analyzer has no such stat.
pub fn analyzer_name(tag: &str, category: &str) -> Option<&'static str> {
    match stat_def(tag)?.analyzer {
        AnalyzerName::Fixed(name) => Some(name),
        AnalyzerName::PerCategory { melee, ranged } => Some(match category {
            "Melee" | "Zaw" => melee,
            _ => ranged,
        }),
        AnalyzerName::Unmapped => None,
    }
}

/// One rolled stat resolved against its weapon, rank and roll range.
pub struct StatValue {
    pub label: &'static str,
    pub display: String,
    pub analyzer_name: Option<&'static str>,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    /// `min` and `max` formatted the same way as `display`, so a range reads in
    /// the same units as the roll sitting in it.
    pub min_display: String,
    pub max_display: String,
    pub position: f64,
    pub unit: Unit,
}

/// Context a stat needs beyond its own roll: everything else on the riven.
#[derive(Debug, Clone, Copy)]
pub struct RollContext<'a> {
    pub category: &'a str,
    pub disposition: f64,
    pub num_buffs: usize,
    pub num_curses: usize,
    pub rank: u8,
}

/// The stat's value at a given roll position, `roll` running 0.9 to 1.1.
/// A tag with no cap for this weapon type has no scale, so the roll position
/// itself stands in for the value — which is what the game screen shows.
fn value_at(tag: &str, positive: bool, ctx: RollContext, roll: f64) -> f64 {
    let cap = stat_def(tag).and_then(|d| d.cap(riven_type(ctx.category)));
    let Some(base_cap) = cap else {
        let frac = (roll - 0.9) / 0.2;
        return if positive { frac } else { -frac };
    };

    // SPECIFIC_FIT_ATTENUATION × omega × base drain.
    let atten = 1.5 * ctx.disposition * 10.0;
    let nb = ctx.num_buffs.min(BUFF_ATTEN.len() - 1);
    let nc = ctx.num_curses.min(BUFF_ATTEN.len() - 1);
    let rank = f64::from(ctx.rank) + 1.0;

    if positive {
        base_cap * atten * 1.25_f64.powi(ctx.num_curses as i32) * roll * BUFF_ATTEN[nb] * rank
    } else {
        -(base_cap * atten * roll * CURSE_ATTEN[nb] * BUFF_ATTEN[nc] * rank)
    }
}

fn format_value(unit: Unit, v: f64) -> String {
    // Negative values already carry their sign.
    let sign = if v >= 0.0 { "+" } else { "" };
    match unit {
        Unit::Percent => format!("{sign}{:.1}%", v * 100.0),
        Unit::Multiplier => format!("x{:.2}", 1.0 + v),
        Unit::Metres => format!("{sign}{v:.1}m"),
        Unit::Seconds => format!("{sign}{v:.1}s"),
        Unit::Flat => format!("{sign}{}", v.round() as i64),
    }
}

/// Resolve one rolled stat: its value, the range it could have rolled in at this
/// rank, and where it sits in that range.
pub fn evaluate(tag: &str, raw: i64, positive: bool, ctx: RollContext) -> StatValue {
    let position = raw as f64 / ROLL_MAX;
    let value = value_at(tag, positive, ctx, 0.9 + position * 0.2);
    let def = stat_def(tag);
    // Without a cap the value is the roll position rather than a physical
    // quantity, so it reads as a percentage whatever the stat's own unit is.
    let unit = def
        .filter(|d| d.cap(riven_type(ctx.category)).is_some())
        .map_or(Unit::Percent, |d| d.unit);

    let min = value_at(tag, positive, ctx, 0.9);
    let max = value_at(tag, positive, ctx, 1.1);

    StatValue {
        label: def.map_or("", |d| d.label),
        display: format_value(unit, value),
        analyzer_name: analyzer_name(tag, ctx.category),
        value,
        min,
        max,
        min_display: format_value(unit, min),
        max_display: format_value(unit, max),
        position,
        unit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The analyzer's whole stat vocabulary, as the analyzer form offers it.
    const ALL_STATS: &[&str] = &[
        "Critical Damage", "Critical Chance", "Multishot", "Base Damage",
        "Fire Rate", "Status Chance", "Toxicity", "Heat", "Electricity",
        "Cold", "Punch Through", "Reload Speed", "Magazine Size",
        "Projectile Flight Speed", "Status Duration",
        "Damage to Infested", "Damage to Grineer", "Damage to Corpus",
        "Attack Speed", "Range", "Combo Count Chance", "Initial Combo",
        "Heavy Attack Efficiency", "Slide Critical Chance",
        "Zoom", "Recoil", "Puncture", "Impact", "Slash", "Ammo Maximum",
    ];

    /// Tags the analyzer has no stat for. A new tag must be added here or given
    /// an analyzer name — never left to fall through unnoticed.
    const UNMAPPED: &[&str] = &["WeaponMeleeFinisherDamageMod", "ComboDurationMod"];

    #[test]
    fn every_stat_tag_either_maps_to_an_analyzer_stat_or_is_listed_as_unmapped() {
        for (tag, def) in RIVEN_STATS {
            match def.analyzer {
                AnalyzerName::Unmapped => assert!(
                    UNMAPPED.contains(tag),
                    "{tag} is unmapped but not listed as such"
                ),
                AnalyzerName::Fixed(name) => {
                    assert!(!UNMAPPED.contains(tag), "{tag} is both mapped and listed unmapped");
                    assert!(ALL_STATS.contains(&name), "{tag} maps to unknown analyzer stat {name}");
                }
                AnalyzerName::PerCategory { melee, ranged } => {
                    assert!(ALL_STATS.contains(&melee), "{tag} maps to unknown analyzer stat {melee}");
                    assert!(ALL_STATS.contains(&ranged), "{tag} maps to unknown analyzer stat {ranged}");
                }
            }
        }
        for tag in UNMAPPED {
            assert!(stat_def(tag).is_some(), "{tag} is listed unmapped but is not a known tag");
        }
    }

    #[test]
    fn fire_rate_becomes_attack_speed_on_melee_weapons() {
        assert_eq!(analyzer_name("WeaponFireRateMod", "Rifle"), Some("Fire Rate"));
        assert_eq!(analyzer_name("WeaponFireRateMod", "Melee"), Some("Attack Speed"));
        assert_eq!(analyzer_name("WeaponFireRateMod", "Zaw"), Some("Attack Speed"));
    }

    #[test]
    fn an_unknown_tag_has_no_analyzer_name() {
        assert_eq!(analyzer_name("WeaponInventedMod", "Rifle"), None);
    }

    fn rifle(buffs: usize, curses: usize) -> RollContext<'static> {
        RollContext { category: "Rifle", disposition: 1.0, num_buffs: buffs, num_curses: curses, rank: 8 }
    }

    #[test]
    fn a_maxed_buff_reaches_the_top_of_its_range() {
        // 0.018333 × 15 × 1.25 × 1.1 × 0.66 × 9 for a two-buff one-curse riven.
        let s = evaluate("WeaponDamageAmountMod", 0x3FFF_FFFF, true, rifle(2, 1));
        assert!((s.value - 2.246_0).abs() < 1e-4, "value {}", s.value);
        assert!((s.value - s.max).abs() < 1e-9);
        assert!((s.min - 1.837_7).abs() < 1e-4, "min {}", s.min);
        assert!((s.position - 1.0).abs() < 1e-9);
        assert_eq!(s.display, "+224.6%");
        assert_eq!(s.label, "Damage");
    }

    #[test]
    fn a_curse_is_negative_and_grows_with_the_roll() {
        // Midway through the range, so the value sits midway between min and max.
        let s = evaluate("WeaponCritChanceMod", 0x1FFF_FFFF, false, rifle(2, 1));
        assert!((s.position - 0.5).abs() < 1e-8, "position {}", s.position);
        assert!((s.value - (s.min + s.max) / 2.0).abs() < 1e-9);
        assert!((s.min - -0.668_2).abs() < 1e-4, "min {}", s.min);
        assert!((s.max - -0.816_7).abs() < 1e-4, "max {}", s.max);
        assert_eq!(s.display, "-74.2%");
    }

    #[test]
    fn a_stat_with_no_cap_for_the_category_falls_back_to_the_roll_position() {
        // Melee range cannot roll on a rifle, so there is no cap to scale by.
        let s = evaluate("WeaponMeleeRangeIncMod", 0x1FFF_FFFF, true, rifle(2, 0));
        assert!((s.value - 0.5).abs() < 1e-8, "value {}", s.value);
        assert!(s.min.abs() < 1e-9, "min {}", s.min);
        assert!((s.max - 1.0).abs() < 1e-9, "max {}", s.max);
        assert_eq!(s.display, "+50.0%");
        // The label and analyzer name still come from the tag.
        assert_eq!(s.label, "Range");
        assert_eq!(s.analyzer_name, Some("Range"));
    }

    #[test]
    fn faction_damage_displays_as_a_multiplier() {
        let ctx = RollContext { category: "Rifle", disposition: 1.0, num_buffs: 2, num_curses: 0, rank: 0 };
        let s = evaluate("WeaponFactionDamageGrineer", 0x3FFF_FFFF, true, ctx);
        // 0.005 × 15 × 1.1 × 0.66 × 1 = 0.05445.
        assert_eq!(s.display, "x1.05");
    }

    #[test]
    fn categories_follow_the_riven_item_type() {
        assert_eq!(riven_category("/Lotus/Upgrades/Mods/Randomized/LotusMeleeRandomModRare"), "Melee");
        assert_eq!(riven_category("/Lotus/Upgrades/Mods/Randomized/LotusRifleRandomModRare"), "Rifle");
        assert_eq!(riven_category("/Lotus/Upgrades/Mods/Randomized/LotusModularPistolRandomModRare"), "Pistol");
        assert_eq!(riven_category("/Lotus/Upgrades/Mods/Randomized/LotusArchgunRandomModRare"), "Arch-gun");
        assert_eq!(riven_category("/Lotus/Upgrades/Mods/Randomized/Something"), "Riven");
    }
}
