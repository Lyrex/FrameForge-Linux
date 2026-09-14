//! The star chart nodes and junctions that award mastery, keyed the way
//! account state names them. The list follows the wiki's Mastery Rank
//! Checklist, and the keys are the region pages' internal names. Hubs, relays
//! and Conclave nodes award nothing and are absent. Dark Sector nodes are on
//! the checklist and stay in.
//!
//! A junction awards 1,000 mastery. A node awards an amount of its own that
//! the wiki records for some planets only, so nodes carry no amount here and
//! their remaining mastery reads Unknown.
//!
//! TODO: Höllvania (SolNode850 to SolNode858) is not on the checklist yet.
//! Add its nodes once they are confirmed to award mastery.
//! TODO: no removed node is listed. A node dropped from the chart keeps its
//! row here under `Unobtainable::RemovedNode` once one is identified.

pub(crate) const JUNCTION_MASTERY: u32 = 1_000;

#[derive(Debug)]
pub(crate) struct Planet {
    pub(crate) name: &'static str,
    /// The junction the planet is entered through. Nothing on the planet is
    /// reachable before it is cleared, short of a squad invite onto the node.
    pub(crate) gate: Option<&'static str>,
    /// Each entry is the node key and its display name. A junction is listed
    /// under the planet it sits on, so the Sedna Junction is on Pluto although
    /// its key says Eris.
    pub(crate) junctions: &'static [(&'static str, &'static str)],
    pub(crate) nodes: &'static [(&'static str, &'static str)],
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Node {
    pub(crate) key: &'static str,
    pub(crate) planet: &'static Planet,
    pub(crate) name: &'static str,
    pub(crate) junction: bool,
}

pub(crate) fn all() -> impl Iterator<Item = Node> {
    PLANETS.iter().flat_map(|planet| {
        let junctions = planet.junctions.iter().map(move |&(key, name)| Node { key, planet, name, junction: true });
        let nodes = planet.nodes.iter().map(move |&(key, name)| Node { key, planet, name, junction: false });
        junctions.chain(nodes)
    })
}

pub(crate) const PLANETS: &[Planet] = &[
    Planet { name: "Mercury", gate: Some("VenusToMercuryJunction"), junctions: &[], nodes: &[
        ("SolNode94", "Apollodorus"),
        ("SolNode223", "Boethius"),
        ("SolNode119", "Caloris"),
        ("SolNode12", "Elion"),
        ("SolNode130", "Lares"),
        ("SolNode103", "M Prime"),
        ("SolNode224", "Odin"),
        ("SolNode226", "Pantheon"),
        ("SolNode225", "Suisei"),
        ("SolNode28", "Terminus"),
        ("SolNode108", "Tolstoj"),
    ] },
    Planet { name: "Venus", gate: Some("EarthToVenusJunction"), junctions: &[
        ("VenusToMercuryJunction", "Mercury Junction"),
    ], nodes: &[
        ("SolNode2", "Aphrodite"),
        ("SolNode23", "Cytherean"),
        ("SolNode128", "E Gate"),
        ("SolNode104", "Fossa"),
        ("SolNode61", "Ishtar"),
        ("SolNode101", "Kiliken"),
        ("SolNode109", "Linea"),
        ("ClanNode1", "Malva"),
        ("SolNode902", "Montes"),
        ("SolNode129", "Orb Vallis"),
        ("ClanNode0", "Romula"),
        ("SolNode22", "Tessera"),
        ("SolNode66", "Unda"),
        ("SolNode123", "V Prime"),
        ("SolNode107", "Venera"),
    ] },
    Planet { name: "Earth", gate: None, junctions: &[
        ("EarthToMarsJunction", "Mars Junction"),
        ("EarthToVenusJunction", "Venus Junction"),
    ], nodes: &[
        ("SolNode79", "Cambria"),
        ("SolNode75", "Cervantes"),
        ("ClanNode2", "Coba"),
        ("SolNode27", "E Prime"),
        ("SolNode903", "Erpo"),
        ("SolNode59", "Eurasia"),
        ("SolNode39", "Everest"),
        ("SolNode85", "Gaia"),
        ("SolNode26", "Lith"),
        ("SolNode63", "Mantle"),
        ("SolNode89", "Mariana"),
        ("SolNode24", "Oro"),
        ("SolNode15", "Pacific"),
        ("SolNode228", "Plains of Eidolon"),
        ("ClanNode3", "Tikal"),
    ] },
    Planet { name: "Lua", gate: None, junctions: &[], nodes: &[
        ("SolNode308", "Apollo"),
        ("SolNode310", "Circulus"),
        ("SolNode304", "Copernicus"),
        ("SolNode301", "Grimaldi"),
        ("SolNode306", "Pavlov"),
        ("SolNode300", "Plato"),
        ("SolNode305", "Stöfler"),
        ("SolNode302", "Tycho"),
        ("SolNode309", "Yuvarium"),
        ("SolNode307", "Zeipel"),
    ] },
    Planet { name: "Mars", gate: Some("EarthToMarsJunction"), junctions: &[
        ("MarsToCeresJunction", "Ceres Junction"),
        ("MarsToPhobosJunction", "Phobos Junction"),
    ], nodes: &[
        ("SolNode106", "Alator"),
        ("SolNode45", "Ara"),
        ("SolNode113", "Ares"),
        ("SolNode41", "Arval"),
        ("SolNode16", "Augustus"),
        ("SolNode65", "Gradivus"),
        ("SolNode58", "Hellas"),
        ("ClanNode8", "Kadesh"),
        ("SolNode36", "Martialis"),
        ("SolNode30", "Olympus"),
        ("SolNode46", "Spear"),
        ("SolNode904", "Syrtis"),
        ("SolNode11", "Tharsis"),
        ("SolNode450", "Tyana Pass"),
        ("SolNode14", "Ultor"),
        ("SolNode68", "Vallis"),
        ("ClanNode9", "Wahiba"),
        ("SolNode99", "War"),
    ] },
    Planet { name: "Deimos", gate: None, junctions: &[], nodes: &[
        ("SolNode721", "Armatus"),
        ("SolNode229", "Cambion Drift"),
        ("SolNode718", "Cambire"),
        ("SolNode709", "Dirus"),
        ("SolNode715", "Effervo"),
        ("SolNode713", "Exequias"),
        ("SolNode710", "Formido"),
        ("SolNode706", "Horend"),
        ("SolNode707", "Hyf"),
        ("SolNode712", "Magnacidium"),
        ("SolNode719", "Munio"),
        ("SolNode716", "Nex"),
        ("SolNode717", "Persto"),
        ("SolNode708", "Phlegyas"),
        ("SolNode711", "Terrorem"),
        ("SolNode720", "Testudo"),
    ] },
    Planet { name: "Phobos", gate: Some("MarsToPhobosJunction"), junctions: &[], nodes: &[
        ("SettlementNode11", "Gulliver"),
        ("SettlementNode20", "Iliad"),
        ("SettlementNode10", "Kepler"),
        ("ClanNode10", "Memphis"),
        ("SettlementNode12", "Monolith"),
        ("SettlementNode1", "Roche"),
        ("SettlementNode15", "Sharpless"),
        ("SettlementNode14", "Shklovsky"),
        ("SettlementNode2", "Skyresh"),
        ("SettlementNode3", "Stickney"),
        ("ClanNode11", "Zeugma"),
    ] },
    Planet { name: "Ceres", gate: Some("MarsToCeresJunction"), junctions: &[
        ("CeresToJupiterJunction", "Jupiter Junction"),
    ], nodes: &[
        ("SolNode132", "Bode"),
        ("SolNode149", "Casta"),
        ("SolNode147", "Cinxia"),
        ("SolNode146", "Draco"),
        ("SolNode144", "Exta"),
        ("ClanNode23", "Gabii"),
        ("SolNode141", "Ker"),
        ("SolNode140", "Kiste"),
        ("SolNode139", "Lex"),
        ("SolNode138", "Ludi"),
        ("SolNode137", "Nuovo"),
        ("SolNode131", "Pallas"),
        ("ClanNode22", "Seimeni"),
        ("SolNode135", "Thon"),
    ] },
    Planet { name: "Jupiter", gate: Some("CeresToJupiterJunction"), junctions: &[
        ("JupiterToEuropaJunction", "Europa Junction"),
        ("JupiterToSaturnJunction", "Saturn Junction"),
    ], nodes: &[
        ("SolNode88", "Adrastea"),
        ("SolNode97", "Amalthea"),
        ("SolNode73", "Ananke"),
        ("SolNode25", "Callisto"),
        ("ClanNode5", "Cameria"),
        ("SolNode74", "Carme"),
        ("SolNode121", "Carpo"),
        ("SolNode100", "Elara"),
        ("SolNode905", "Galilea"),
        ("SolNode87", "Ganymede"),
        ("SolNode125", "Io"),
        ("SolNode126", "Metis"),
        ("ClanNode4", "Sinai"),
        ("SolNode740", "The Ropalolyst"),
        ("SolNode10", "Thebe"),
        ("SolNode53", "Themisto"),
    ] },
    Planet { name: "Europa", gate: Some("JupiterToEuropaJunction"), junctions: &[], nodes: &[
        ("SolNode203", "Abaddon"),
        ("SolNode204", "Armaros"),
        ("SolNode205", "Baal"),
        ("ClanNode7", "Cholistan"),
        ("SolNode220", "Kokabiel"),
        ("ClanNode6", "Larzac"),
        ("SolNode209", "Morax"),
        ("SolNode210", "Naamah"),
        ("SolNode217", "Orias"),
        ("SolNode211", "Ose"),
        ("SolNode212", "Paimon"),
        ("SolNode214", "Sorath"),
        ("SolNode215", "Valac"),
        ("SolNode216", "Valefor"),
    ] },
    Planet { name: "Saturn", gate: Some("JupiterToSaturnJunction"), junctions: &[
        ("SaturnToUranusJunction", "Uranus Junction"),
    ], nodes: &[
        ("SolNode31", "Anthe"),
        ("SolNode82", "Calypso"),
        ("ClanNode12", "Caracol"),
        ("SolNode70", "Cassini"),
        ("SolNode67", "Dione"),
        ("SolNode19", "Enceladus"),
        ("SolNode42", "Helene"),
        ("SolNode93", "Keeler"),
        ("SolNode50", "Numa"),
        ("SolNode906", "Pandora"),
        ("ClanNode13", "Piscinas"),
        ("SolNode18", "Rhea"),
        ("SolNode20", "Telesto"),
        ("SolNode32", "Tethys"),
        ("SolNode96", "Titan"),
    ] },
    Planet { name: "Uranus", gate: Some("SaturnToUranusJunction"), junctions: &[
        ("UranusToNeptuneJunction", "Neptune Junction"),
    ], nodes: &[
        ("SolNode33", "Ariel"),
        ("ClanNode17", "Assur"),
        ("SolNode723", "Brutus"),
        ("SolNode907", "Caelus"),
        ("SolNode60", "Caliban"),
        ("SolNode83", "Cressida"),
        ("SolNode98", "Desdemona"),
        ("SolNode69", "Ophelia"),
        ("SolNode114", "Puck"),
        ("SolNode9", "Rosalind"),
        ("SolNode122", "Stephano"),
        ("SolNode34", "Sycorax"),
        ("SolNode105", "Titania"),
        ("SolNode64", "Umbriel"),
        ("ClanNode16", "Ur"),
    ] },
    Planet { name: "Neptune", gate: Some("UranusToNeptuneJunction"), junctions: &[
        ("NeptuneToPlutoJunction", "Pluto Junction"),
    ], nodes: &[
        ("SolNode6", "Despina"),
        ("SolNode1", "Galatea"),
        ("ClanNode21", "Kelashin"),
        ("SolNode118", "Laomedeia"),
        ("SolNode49", "Larissa"),
        ("SolNode84", "Nereid"),
        ("SolNode62", "Neso"),
        ("SolNode17", "Proteus"),
        ("SolNode127", "Psamathe"),
        ("SolNode908", "Salacia"),
        ("SolNode57", "Sao"),
        ("SolNode78", "Triton"),
        ("ClanNode20", "Yursa"),
    ] },
    Planet { name: "Pluto", gate: Some("NeptuneToPlutoJunction"), junctions: &[
        ("PlutoToErisJunction", "Eris Junction"),
        ("ErisToSednaJunction", "Sedna Junction"),
    ], nodes: &[
        ("SolNode4", "Acheron"),
        ("SolNode43", "Cerberus"),
        ("SolNode56", "Cypress"),
        ("SolNode51", "Hades"),
        ("ClanNode25", "Hieracon"),
        ("SolNode76", "Hydra"),
        ("SolNode38", "Minthe"),
        ("SolNode21", "Narcissus"),
        ("SolNode102", "Oceanum"),
        ("SolNode72", "Outer Terminus"),
        ("SolNode81", "Palus"),
        ("SolNode48", "Regna"),
        ("ClanNode24", "Sechura"),
    ] },
    Planet { name: "Eris", gate: Some("PlutoToErisJunction"), junctions: &[], nodes: &[
        ("ClanNode18", "Akkad"),
        ("SolNode153", "Brugia"),
        ("SolNode162", "Isos"),
        ("SolNode164", "Kala-azar"),
        ("SolNode175", "Naeglar"),
        ("SolNode166", "Nimus"),
        ("SolNode167", "Oestrus"),
        ("SolNode171", "Saxis"),
        ("SolNode173", "Solium"),
        ("SolNode172", "Xini"),
        ("ClanNode19", "Zabala"),
    ] },
    Planet { name: "Sedna", gate: Some("ErisToSednaJunction"), junctions: &[], nodes: &[
        ("SolNode181", "Adaro"),
        ("ClanNode14", "Amarna"),
        ("SolNode185", "Berehynia"),
        ("SolNode196", "Charybdis"),
        ("SolNode195", "Hydron"),
        ("SolNode177", "Kappa"),
        ("SolNode188", "Kelpie"),
        ("SolNode191", "Marid"),
        ("SolNode193", "Merrow"),
        ("SolNode189", "Naga"),
        ("SolNode190", "Nakki"),
        ("SolNode184", "Rusalka"),
        ("ClanNode15", "Sangeru"),
        ("SolNode187", "Selkie"),
        ("SolNode183", "Vodyanoi"),
        ("SolNode199", "Yam"),
    ] },
    Planet { name: "Void", gate: None, junctions: &[], nodes: &[
        ("SolNode405", "Ani"),
        ("SolNode410", "Aten"),
        ("SolNode408", "Belenus"),
        ("SolNode401", "Hepit"),
        ("SolNode411", "Marduk"),
        ("SolNode412", "Mithra"),
        ("SolNode409", "Mot"),
        ("SolNode407", "Oxomoco"),
        ("SolNode404", "Stribog"),
        ("SolNode402", "Taranis"),
        ("SolNode400", "Teshub"),
        ("SolNode403", "Tiwaz"),
        ("SolNode406", "Ukko"),
    ] },
    Planet { name: "Kuva Fortress", gate: None, junctions: &[], nodes: &[
        ("SolNode746", "Dakata"),
        ("SolNode748", "Garus"),
        ("SolNode741", "Koro"),
        ("SolNode742", "Nabuk"),
        ("SolNode747", "Pago"),
        ("SolNode743", "Rotuma"),
        ("SolNode745", "Tamu"),
        ("SolNode744", "Taveuni"),
    ] },
    Planet { name: "Zariman", gate: None, junctions: &[], nodes: &[
        ("SolNode230", "Everview Arc"),
        ("SolNode231", "Halako Perimeter"),
        ("SolNode233", "Oro Works"),
        ("SolNode235", "The Greenway"),
        ("SolNode232", "Tuvul Commons"),
    ] },
    Planet { name: "Duviri", gate: None, junctions: &[], nodes: &[
        ("SolNode238", "The Circuit"),
        ("SolNode236", "The Duviri Experience"),
        ("SolNode237", "The Lone Story"),
    ] },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_chart_has_thirteen_junctions_and_no_duplicate_key() {
        let nodes: Vec<Node> = all().collect();
        assert_eq!(nodes.len(), 265);
        assert_eq!(nodes.iter().filter(|n| n.junction).count(), 13);
        let keys: HashSet<&str> = nodes.iter().map(|n| n.key).collect();
        assert_eq!(keys.len(), nodes.len());
        for planet in PLANETS {
            if let Some(gate) = planet.gate {
                assert!(keys.contains(gate), "{}: gate {gate} is a listed junction", planet.name);
            }
        }
        let sedna = nodes.iter().find(|n| n.key == "ErisToSednaJunction").expect("listed");
        assert_eq!((sedna.planet.name, sedna.name, sedna.junction), ("Pluto", "Sedna Junction", true));
    }
}
