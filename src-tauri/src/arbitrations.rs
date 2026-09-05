//! The arbitration rotation, from the community feed at browse.wf.
//!
//! The feed is a plain `epoch,node` list of every rotation for years ahead.
//! It is cached whole and windowed here, so a refresh failure still leaves a
//! full schedule to show.

use std::time::Duration;

use serde::Serialize;

use crate::cache;

const FEED_URL: &str = "https://browse.wf/arbys.txt";
const FEED_CACHE: &str = "arbitrations-v1.json";
const FEED_TTL: Duration = Duration::from_secs(3600);
const ROTATION_SECS: u64 = 3600;
/// How far ahead the browser shows. The feed runs years further.
const HORIZON_SECS: u64 = 3 * 24 * 3600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rotation {
    pub start: u64,
    pub node_id: String,
}

/// Lines that do not parse are dropped rather than failing the whole feed:
/// one bad line at the top of a year-long list must not blank the schedule.
pub fn parse_feed(text: &str) -> Vec<Rotation> {
    let mut rotations: Vec<Rotation> = text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(',');
            let start = fields.next()?.trim().parse().ok()?;
            let node_id = fields.next()?.trim();
            (!node_id.is_empty()).then(|| Rotation { start, node_id: node_id.to_string() })
        })
        .collect();
    rotations.sort_by_key(|r| r.start);
    rotations
}

/// The Arbitration Goons' rating of how well a node farms Vitus Essence,
/// vendored from `calamity-inc/browse.wf` (`supplemental-data/arbyTiers.js`).
/// Refresh by hand when that file changes: it moves once in a blue moon, and
/// a runtime fetch would mean parsing JavaScript to learn a letter.
///
/// A node the rating does not cover is Unrated, which is not the same as bad:
/// the list only covers nodes the rotation actually visits.
pub fn tier(node_id: &str) -> Option<&'static str> {
    Some(match node_id {
        "SolNode450" | "SolNode106" | "SolNode25" | "SolNode719" | "SolNode64" => "S",
        "SolNode147" | "SolNode23" | "SolNode172" => "A",
        "SolNode167" | "ClanNode24" | "SolNode149" | "ClanNode22" | "ClanNode18" | "SolNode164"
        | "SolNode707" | "SolNode211" | "SolNode42" | "SolNode195" | "SolNode408"
        | "SolNode402" => "B",
        "SolNode412" | "ClanNode2" | "SolNode46" | "ClanNode8" | "SolNode212" | "SolNode22"
        | "SolNode224" | "SolNode26" | "ClanNode6" | "SolNode122" | "SolNode72" => "C",
        "SolNode130" | "ClanNode15" | "SolNode85" | "SolNode18" | "SolNode305" | "ClanNode4"
        | "SolNode125" => "D",
        _ => return None,
    })
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ScheduleEntry {
    pub start: u64,
    pub end: u64,
    pub node_id: String,
    pub node: String,
    pub region: String,
    pub mission_type: String,
    pub faction: String,
    pub tier: Option<&'static str>,
}

/// A rotation lasts until the next one in the feed, or one hour when the
/// feed has no successor, so a gap in the feed shows as a gap and not as an
/// arbitration that never ends.
pub fn within_horizon(rotations: &[Rotation], now: u64, horizon: u64) -> Vec<(u64, u64, &str)> {
    rotations
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let end = rotations
                .get(i + 1)
                .map(|next| next.start)
                .unwrap_or(r.start + ROTATION_SECS)
                .min(r.start + ROTATION_SECS);
            (r.start, end, r.node_id.as_str())
        })
        .filter(|(start, end, _)| *end > now && *start < now + horizon)
        .collect()
}

fn entry(start: u64, end: u64, node_id: &str) -> ScheduleEntry {
    // The sol-node data has no separate region field; the display name
    // carries it in parentheses ("Hyf (Deimos)"). An id the data does not
    // know resolves to itself, which is the whole fallback: the hour still
    // shows, with blank region, type and faction.
    let display = crate::resolve_node(node_id);
    let (node, region) = display
        .rsplit_once(" (")
        .map(|(n, r)| (n.to_string(), r.trim_end_matches(')').to_string()))
        .unwrap_or((display.clone(), String::new()));
    ScheduleEntry {
        start,
        end,
        node_id: node_id.to_string(),
        node,
        region,
        mission_type: crate::node_mission_type(node_id),
        faction: crate::node_enemy(node_id),
        tier: tier(node_id),
    }
}

#[derive(Debug, Serialize)]
pub struct Schedule {
    pub entries: Vec<ScheduleEntry>,
    pub source: cache::Source,
    pub warning: Option<String>,
}

fn fetch_feed(force: bool) -> (Option<String>, cache::Source, Option<String>) {
    let ttl = if force { Duration::ZERO } else { FEED_TTL };
    cache::get_or_refresh(FEED_CACHE, ttl, |etag| cache::get_conditional(FEED_URL, etag))
}

pub(crate) fn refresh_feed(_app: &tauri::AppHandle, force: bool) -> Result<(), String> {
    match fetch_feed(force) {
        (_, _, Some(warning)) => Err(warning),
        _ => Ok(()),
    }
}

#[tauri::command]
pub async fn fetch_arbitration_schedule() -> Result<Schedule, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let (text, source, warning) = fetch_feed(false);
        let Some(text) = text else {
            return Err(warning.unwrap_or_else(|| "arbitration feed unavailable".to_string()));
        };
        let entries = within_horizon(&parse_feed(&text), cache::now_unix(), HORIZON_SECS)
            .into_iter()
            .map(|(start, end, node_id)| entry(start, end, node_id))
            .collect();
        Ok(Schedule { entries, source, warning })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rotation(start: u64, node_id: &str) -> Rotation {
        Rotation { start, node_id: node_id.to_string() }
    }

    #[test]
    fn feed_lines_become_rotations_in_start_order() {
        let text = "7200,SolNode2\n3600,SolNode1\r\n10800,ClanNode10\n";
        assert_eq!(
            parse_feed(text),
            vec![rotation(3600, "SolNode1"), rotation(7200, "SolNode2"), rotation(10800, "ClanNode10")]
        );
    }

    #[test]
    fn malformed_lines_are_skipped_not_fatal() {
        let text = "\n3600,SolNode1\ngarbage\n,SolNode9\nnotanumber,SolNode3\n7200,\n  10800 , SolNode4 \n7200,SolNode2,extra";
        assert_eq!(
            parse_feed(text),
            vec![rotation(3600, "SolNode1"), rotation(7200, "SolNode2"), rotation(10800, "SolNode4")]
        );
    }

    #[test]
    fn empty_feed_is_an_empty_schedule() {
        assert!(parse_feed("").is_empty());
        assert!(within_horizon(&[], 1000, HORIZON_SECS).is_empty());
    }

    #[test]
    fn window_keeps_the_current_rotation_and_drops_finished_ones() {
        let feed = [rotation(0, "a"), rotation(3600, "b"), rotation(7200, "c"), rotation(10800, "d")];
        let got = within_horizon(&feed, 5000, 3600);
        assert_eq!(got, vec![(3600, 7200, "b"), (7200, 10800, "c")]);
    }

    #[test]
    fn a_gap_in_the_feed_ends_the_rotation_after_an_hour() {
        let feed = [rotation(0, "a"), rotation(36000, "b")];
        assert_eq!(within_horizon(&feed, 100, 100_000), vec![(0, 3600, "a"), (36000, 39600, "b")]);
    }

    #[test]
    fn rated_nodes_carry_their_tier_and_the_rest_carry_none() {
        assert_eq!(tier("SolNode450"), Some("S"));
        assert_eq!(tier("ClanNode24"), Some("B"));
        assert_eq!(tier("SolNode125"), Some("D"));
        assert_eq!(tier("SolNode1"), None);
        assert_eq!(tier(""), None);
    }

    #[test]
    fn horizon_is_exclusive_of_rotations_starting_at_its_edge() {
        let feed = [rotation(0, "a"), rotation(3600, "b")];
        assert_eq!(within_horizon(&feed, 0, 3600), vec![(0, 3600, "a")]);
    }
}
