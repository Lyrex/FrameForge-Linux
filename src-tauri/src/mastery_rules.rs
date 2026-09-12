//! The game's mastery rules, kept apart from the catalogue: WFCD says which
//! equipment exists and carries `maxLevelCap` where it knows one, but it ships
//! no cap for Necramechs and marks Amp Prisms non-masterable although the
//! game's XPInfo credits them. Every consumer
//! derives rank, cap, and masterability through here rather than from the
//! catalogue fields directly.

pub(crate) const DEFAULT_RANK_CAP: u32 = 30;

/// Warframe-like equipment needs 1000·rank² affinity per rank and awards 200
/// mastery per rank; everything else masterable is a weapon at 500·rank² and
/// 100. Pet weapons share the `/Types/Friendly/Pets/` prefix with the pets
/// themselves, so the pet side is matched on model parts and power suits.
pub(crate) fn is_warframe_like(path: &str) -> bool {
    path.contains("/Powersuits/")
        || path.contains("/SentinelPowersuits/")
        || path.ends_with("PetPowerSuit")
        || path.contains("/MoaPetParts/")
        || path.contains("/ZanukaPetParts/")
        || path.contains("/Vehicles/Hoverboard/")
        || path.ends_with("/RailJack/DefaultHarness")
}

pub(crate) fn xp_to_rank(xp: i64, path: &str) -> u32 {
    let base = if is_warframe_like(path) { 1000.0f64 } else { 500.0f64 };
    (xp.max(0) as f64 / base).sqrt().floor() as u32
}

pub(crate) fn rank_cap(path: &str, catalogue_cap: Option<u32>) -> u32 {
    catalogue_cap.unwrap_or(if path.contains("/EntratiMech/") { 40 } else { DEFAULT_RANK_CAP })
}

pub(crate) fn earned_rank(xp: i64, path: &str, catalogue_cap: Option<u32>) -> u32 {
    xp_to_rank(xp, path).min(rank_cap(path, catalogue_cap))
}

pub(crate) fn masterable(wfcd: Option<bool>, path: &str) -> Option<bool> {
    if !path.ends_with("Blueprint") {
        // Amp Prisms (barrels) grant mastery; WFCD incorrectly says false.
        if path.contains("/OperatorAmplifiers/") && path.contains("/Barrel/") {
            return Some(true);
        }
        // Operator amp weapons (Sirocco, etc.) grant mastery.
        if path.contains("/Operator/Pistols/") {
            return Some(true);
        }
    }
    wfcd
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUND_WEAPON: &str = "/Lotus/Types/Friendly/Pets/ZanukaPets/ZanukaPetMeleeWeaponIS";
    const MOA_WEAPON: &str = "/Lotus/Types/Friendly/Pets/MoaPets/MoaPetComponents/HextraWeapon";
    const HOUND_MODEL: &str = "/Lotus/Types/Friendly/Pets/ZanukaPets/ZanukaPetParts/ZanukaPetPartHeadA";
    const MOA_MODEL: &str = "/Lotus/Types/Friendly/Pets/MoaPets/MoaPetParts/MoaPetHeadLambeo";
    const VULPAPHYLA: &str = "/Lotus/Types/Friendly/Pets/CreaturePets/ArmoredInfestedCatbrowPetPowerSuit";
    const KUBROW: &str = "/Lotus/Types/Game/KubrowPet/GuardKubrowPetPowerSuit";
    const SENTINEL: &str = "/Lotus/Types/Sentinels/SentinelPowersuits/CarrierPowerSuit";
    const SENTINEL_WEAPON: &str = "/Lotus/Types/Sentinels/SentinelWeapons/Gremlin";
    const KDRIVE: &str = "/Lotus/Types/Vehicles/Hoverboard/HoverboardParts/PartComponents/HoverboardSolarisA/HoverboardSolarisADeck";
    const PLEXUS: &str = "/Lotus/Types/Game/CrewShip/RailJack/DefaultHarness";
    const SUN_AND_MOON: &str = "/Lotus/Types/Friendly/PlayerControllable/Weapons/DuviriDualSwordsWeapon";
    const VOIDRIG: &str = "/Lotus/Powersuits/EntratiMech/NechroTech";

    /// Hound and MOA weapons at 640,341 XP are rank 30 in the game's profile;
    /// the warframe base would leave them at 25.
    #[test]
    fn pet_weapons_rank_on_the_weapon_base_and_pets_on_the_warframe_base() {
        for weapon in [HOUND_WEAPON, MOA_WEAPON, SENTINEL_WEAPON, SUN_AND_MOON] {
            assert!(!is_warframe_like(weapon), "{weapon}");
            assert_eq!(earned_rank(640_341, weapon, None), 30, "{weapon}");
        }
        for suit in [HOUND_MODEL, MOA_MODEL, VULPAPHYLA, KUBROW, SENTINEL, KDRIVE, PLEXUS, VOIDRIG] {
            assert!(is_warframe_like(suit), "{suit}");
            assert_eq!(xp_to_rank(640_341, suit), 25, "{suit}");
        }
    }

    #[test]
    fn rank_40_equipment_caps_at_40() {
        const KUVA_NUKOR: &str = "/Lotus/Weapons/Grineer/KuvaLich/Secondaries/Nukor/KuvaNukor";
        const CODA_POX: &str = "/Lotus/Weapons/Infested/InfestedLich/Pistols/CodaPox";
        const PARACESIS: &str = "/Lotus/Weapons/Orokin/BallasSword/BallasSwordWeapon";
        const TENET_EXEC: &str = "/Lotus/Weapons/Tenno/Melee/Swords/CrpBigSlash/CrpBigSlash";
        for path in [KUVA_NUKOR, CODA_POX, PARACESIS, TENET_EXEC] {
            assert_eq!(rank_cap(path, Some(40)), 40, "{path}");
        }
        assert_eq!(rank_cap(VOIDRIG, None), 40);
        assert_eq!(rank_cap(KUBROW, None), 30);
        // Rank 40 on the weapon base is 800,000 affinity; on the Necramech base 1,600,000.
        assert_eq!(earned_rank(799_999, KUVA_NUKOR, Some(40)), 39);
        assert_eq!(earned_rank(129_043_438, KUVA_NUKOR, Some(40)), 40);
        assert_eq!(earned_rank(3_527_278, VOIDRIG, None), 40);
    }

    #[test]
    fn negative_xp_is_rank_zero() {
        assert_eq!(xp_to_rank(-1, KUBROW), 0);
    }
}
