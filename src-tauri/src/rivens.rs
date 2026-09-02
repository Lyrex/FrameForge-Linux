use std::collections::HashMap;
use tracing::warn;
use std::sync::atomic::Ordering;
use tauri::{Emitter, State};
use crate::app_state::AppState;
use crate::monitor::append_to_file;
use crate::{cache, memory_scanner, ocr, paths};

pub(crate) fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ─── Riven database ───────────────────────────────────────────────────────────

static RIVEN_ABBREVIATIONS: &[(&str, &str)] = &[
    ("CD",    "Critical Damage"),
    ("CC",    "Critical Chance"),
    ("MS",    "Multishot"),
    ("DMG",   "Base Damage"),
    ("FR",    "Fire Rate"),
    ("SC",    "Status Chance"),
    ("TOX",   "Toxicity"),
    ("HEAT",  "Heat"),
    ("ELEC",  "Electricity"),
    ("COLD",  "Cold"),
    ("PT",    "Punch Through"),
    ("RLS",   "Reload Speed"),
    ("MAG",   "Magazine Size"),
    ("AMMO",  "Ammo Maximum"),
    ("ZOOM",  "Zoom"),
    ("REC",   "Recoil"),
    ("SLASH", "Slash"),
    ("PUNC",  "Puncture"),
    ("IMP",   "Impact"),
    ("PFS",   "Projectile Flight Speed"),
    ("SD",    "Status Duration"),
    ("DTI",   "Damage to Infested"),
    ("DTG",   "Damage to Grineer"),
    ("DTC",   "Damage to Corpus"),
    ("RLS",   "Reload Speed"),
    ("AS",    "Attack Speed"),
    ("RANGE", "Range"),
    ("IC",    "Initial Combo"),
    ("CC",    "Combo Count Chance"),
    ("EFF",   "Heavy Attack Efficiency"),
    ("SLIDE", "Slide Critical Chance"),
    ("FIN",   "Finisher Damage"),
    ("HA",    "Heavy Attack Damage"),
    ("SLAM",  "Slam Attack"),
];

/// Expand all-caps abbreviations in a notes string using the abbreviations table.
/// "PUNC gives 5%CC" → "Puncture gives 5% Critical Chance"
fn expand_abbrevs_in_notes(notes: &str) -> String {
    let bytes = notes.as_bytes();
    let mut result = String::with_capacity(notes.len() * 2);
    let mut last = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_uppercase() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_uppercase() {
                i += 1;
            }
            // Only expand if surrounded by non-alphabetic chars (word boundary)
            let prev_alpha = start > 0 && bytes[start - 1].is_ascii_alphabetic();
            let next_alpha = i < bytes.len() && bytes[i].is_ascii_alphabetic();
            if !prev_alpha && !next_alpha {
                let word = &notes[start..i];
                if let Some((_, full)) = RIVEN_ABBREVIATIONS.iter().find(|(a, _)| *a == word) {
                    result.push_str(&notes[last..start]);
                    result.push_str(full);
                    last = i;
                }
            }
        } else {
            i += 1;
        }
    }
    result.push_str(&notes[last..]);
    result
}

fn riven_abbrev_to_full(abbrev: &str) -> String {
    let up = abbrev.trim().to_uppercase();
    RIVEN_ABBREVIATIONS.iter()
        .find(|(a, _)| *a == up.as_str())
        .map(|(_, f)| f.to_string())
        .unwrap_or_else(|| abbrev.to_string())
}

/// Parse spreadsheet stat string into alternatives, each containing slot groups.
/// "or" = completely separate valid build paths — scored independently.
/// Space-separated = each token is its own required slot.
/// Slash-separated = any one of these fills that slot.
///
/// "TOX DTC or TOX DTG or CD MS/TOX/FR" →
///   [ [[TOX],[DTC]], [[TOX],[DTG]], [[CD],[MS,TOX,FR]] ]
fn parse_stat_alternatives(s: &str) -> Vec<Vec<Vec<String>>> {
    let without_note = s.split('(').next().unwrap_or(s);
    let mut alternatives: Vec<Vec<Vec<String>>> = Vec::new();
    for alt in without_note.split(" or ") {
        let mut groups: Vec<Vec<String>> = Vec::new();
        for token in alt.split_whitespace() {
            let options: Vec<String> = token.split('/')
                .filter_map(|t| { let t = t.trim(); if t.is_empty() { None } else { Some(riven_abbrev_to_full(t)) } })
                .collect();
            if !options.is_empty() { groups.push(options); }
        }
        if !groups.is_empty() { alternatives.push(groups); }
    }
    if alternatives.is_empty() { alternatives.push(vec![]); }
    alternatives
}

/// Flat list helper — kept for the wanted display (unique stat names across all alternatives)
fn parse_stat_groups(s: &str) -> Vec<Vec<String>> {
    let alts = parse_stat_alternatives(s);
    let mut all: Vec<Vec<String>> = Vec::new();
    for alt in alts {
        for group in alt {
            if !all.iter().any(|g| g == &group) { all.push(group); }
        }
    }
    all
}

/// Whether `text` (already lowercased) carries the riven screen's "FITS IN"
/// panel label. The label is small enough on a 4K frame that an engine can close
/// the word gap and report "FITSIN", so both sides are compared with spaces
/// removed.
fn says_fits_in(text: &str) -> bool {
    text.replace(' ', "").contains("fitsin")
}

/// Weapon-name candidates from the "FITS IN" panel's OCR, top to bottom.
///
/// The panel is mostly icon and border debris (single glyphs, punctuation)
/// with the weapon name and the panel's own buttons as the only real words, so a
/// candidate is a line of at least four letters that is not one of those
/// buttons. The name sits below the "FITS IN" label and above "SHOW RANKED",
/// which is why callers take the last candidate rather than the first.
fn panel_weapon_candidates(panel: &str) -> Vec<String> {
    panel
        .lines()
        .map(|l| l.trim().to_lowercase())
        .filter(|l| {
            l.chars().filter(|c| c.is_alphabetic()).count() >= 4
                && !says_fits_in(l)
                && !l.contains("show ranked")
                && !l.contains("close")
                && !l.contains("cancel")
        })
        .collect()
}

/// Rejoin a riven card's OCR text into one line per stat.
///
/// A stat starts with `+<digit>`, `-<digit>` or `x<digit>`; the digit matters
/// because the card's dividers arrive as bare signs. Long names wrap onto a
/// second line ("+22.2% Magazine" / "Capacity"), so a following line is normally
/// the tail of the stat above it.
///
/// The border, rank pips and element icons also arrive as short punctuation
/// (`_`, `;`, `==`, `¢ Y`). Gluing those into a name breaks the lookup
/// ("Magazine _ Capacity"), so a continuation has to read as a word: three or
/// more letters, which also excludes the "MR11" rank label. Trailing debris is
/// left alone, since the lookup matches on substrings.
fn join_wrapped_stat_lines(text: &str) -> Vec<String> {
    let mut joined: Vec<String> = Vec::new();
    let mut pending: Option<String> = None;
    for line in text.lines() {
        // Artwork bleed puts stray glyphs in front of a sign ("v & -34.3%
        // Critical Chance"), hiding it so the stat joins upward and two are lost.
        // Trim only a short prefix carrying no word of its own: "Re-1oad Speed"
        // is a wrapped name, and "MR-1" would become a stat the card never had.
        // Counted in chars, not bytes, since this is where multi-byte glyphs land.
        let l = line.trim();
        let l = match l.find(['+', '-', 'x', 'X']) {
            Some(at) if at > 0
                && l[..at].chars().count() <= 4
                && !l[..at].ends_with(|c: char| c.is_alphanumeric())
                && l[at + 1..].starts_with(|c: char| c.is_ascii_digit())
                && l[..at].chars().filter(|c| c.is_alphabetic()).count() <= 2
                => &l[at..],
            _ => l,
        };
        if l.is_empty() { continue; }
        let ll = l.to_lowercase();
        // OCR sometimes misreads '+' as '•', '·', or similar bullet chars
        let first_char = l.chars().next().unwrap_or(' ');
        let is_ocr_plus = "•·○●◦".contains(first_char)
            && l.len() > 1
            && l.chars().nth(1).map_or(false, |c| c.is_ascii_digit());
        // A sign alone is not a stat: dividers come through as bare "-" lines,
        // which invented a negative stat on every card. Require a digit behind it.
        let is_signed_value = (l.starts_with('+') || l.starts_with('-'))
            && l[1..].trim_start().starts_with(|c: char| c.is_ascii_digit());
        let is_stat_start = is_signed_value
            || (ll.starts_with('x') && l.len() > 2 && l.chars().nth(1).map_or(false, |c| c.is_ascii_digit()))
            || is_ocr_plus;
        // "Damage to Grineer/Corpus/Infested" arrives unprefixed when the OCR
        // drops the leading "x0.88" multiplier.
        let is_orphan_stat = ll.starts_with("damage to grineer")
            || ll.starts_with("damage to corpus")
            || ll.starts_with("damage to infested");
        // "kuva" comes off the weapon-name filter below but stays here: a reroll
        // comparison screen stacks two cards, so the lower card's title follows
        // the upper card's stats with nothing between, and a title reads as a word.
        // TODO: only Kuva titles are caught. "Boltor Conci-" still glues, which
        // needs a title recognised as a title rather than another word on a list.
        let is_ui_noise = ll.contains("fits in") || ll.starts_with("mr ")
            || ll.contains("inventory") || ll.contains("cycle")
            || ll.contains("kuva") || ll.contains("remaining")
            || ll.contains("show ranked") || ll.contains("cancel");
        let reads_as_a_word = l.chars().filter(|c| c.is_alphabetic()).count() >= 3;
        if is_stat_start {
            if let Some(prev) = pending.take() { joined.push(prev); }
            pending = Some(l.to_string());
        } else if is_orphan_stat {
            if let Some(prev) = pending.take() { joined.push(prev); }
            joined.push(format!("+?% {}", l));
        } else if is_ui_noise {
            if let Some(prev) = pending.take() { joined.push(prev); }
        } else if reads_as_a_word {
            if let Some(ref mut prev) = pending {
                prev.push(' ');
                prev.push_str(l);
            }
        }
    }
    if let Some(prev) = pending { joined.push(prev); }
    joined
}

/// Flat dedup list of all stats across all groups — kept for backwards compat where needed.
fn parse_riven_stat_str(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    for group in parse_stat_groups(s) {
        for stat in group {
            if !result.contains(&stat) { result.push(stat); }
        }
    }
    result
}

fn csv_split_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for ch in line.chars() {
        match ch {
            '"' => in_q = !in_q,
            ',' if !in_q => { fields.push(cur.trim().to_string()); cur = String::new(); }
            c => cur.push(c),
        }
    }
    fields.push(cur.trim().to_string());
    fields
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct RivenEntry {
    pub weapon: String,
    /// Outer Vec = "or" alternatives (each is a completely separate valid build).
    /// Middle Vec = slot groups within that alternative.
    /// Inner Vec  = options for that slot (slash-separated).
    /// "TOX DTC or TOX DTG" → [[[TOX],[DTC]], [[TOX],[DTG]]]
    pub stat_alternatives: Vec<Vec<Vec<String>>>,
    /// Flat dedup list for backwards-compat display (unique groups across all alternatives)
    pub stat_groups: Vec<Vec<String>>,
    pub safe_negatives: Vec<String>,
    pub notes: String,
}

#[derive(serde::Serialize, Clone)]
pub struct AlternativeResult {
    pub label: String,        // "Option 1", "Option 2", etc.
    pub matched: Vec<String>,
    pub missing: Vec<String>,
    pub score: f32,
    pub verdict: String,
}

#[derive(serde::Serialize)]
pub struct RivenAnalysis {
    pub weapon: String,
    pub matched_positives: Vec<String>,   // best alternative
    pub missing_positives: Vec<String>,   // best alternative
    pub safe_negatives_present: Vec<String>,
    pub harmful_negatives: Vec<String>,
    pub total_wanted: usize,
    pub score: f32,
    pub verdict: String,
    pub notes: String,
    pub alternatives: Vec<AlternativeResult>, // one per "or" path
}

/// `None` until the first lookup fills it. A failed first load leaves an empty
/// map rather than a permanent one, so the next refresh can still succeed.
static RIVEN_DB: std::sync::RwLock<Option<HashMap<String, RivenEntry>>> =
    std::sync::RwLock::new(None);

/// Returns a map of weapon unique_name → riven disposition (omegaAttenuation).
/// Data comes from All.json (fetched during item load) — no extra HTTP request.
#[tauri::command]
pub(crate) fn get_weapon_dispositions(state: State<AppState>) -> HashMap<String, f32> {
    state.weapon_dispositions.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Guards against concurrent scans: only one get_wfm_top_items scan runs at a time.
/// Concurrent callers wait (polling the cache) rather than starting a second scan.
/// Scan orchestration is the command's concern; the cached result it produces
/// lives in `Wfm`.
static WFM_SCAN_RUNNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Holds the one scan slot; dropping it releases the slot. Tying the release to
/// `Drop` frees it on every exit — error, panic, or a cancelled future — where
/// a manual `store(false)` after an `.await` would leak the slot for the rest
/// of the session and time out every later scan.
pub(crate) struct WfmScanSlot;

impl WfmScanSlot {
    pub(crate) fn claim() -> Option<Self> {
        WFM_SCAN_RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
            .then_some(WfmScanSlot)
    }
}

impl Drop for WfmScanSlot {
    fn drop(&mut self) {
        WFM_SCAN_RUNNING.store(false, Ordering::SeqCst);
    }
}

/// Cache: (warframe_pid, Option<flag_va>). None inner = scanned this PID, pattern not found.
/// Re-scanned only when PID changes (game restart). Prevents 200ms re-scan storm.
static RIVEN_FLAG_VA: std::sync::OnceLock<std::sync::Mutex<Option<(u32, Option<usize>)>>> =
    std::sync::OnceLock::new();

/// Guard: prevents spawning multiple watcher threads if start_riven_memory_watcher is called again.
static RIVEN_WATCHER_RUNNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

const RIVEN_DB_CACHE: &str = "riven-db-v1.json";

/// The community sheet is edited by hand and rarely more than once a day.
const RIVEN_DB_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Run `f` against the riven database, loading it first if this is the first
/// lookup of the session.
fn with_riven_db<R>(f: impl FnOnce(&HashMap<String, RivenEntry>) -> R) -> R {
    {
        let guard = RIVEN_DB.read().unwrap_or_else(|e| e.into_inner());
        if let Some(db) = guard.as_ref() {
            return f(db);
        }
    }

    // ponytail: two first-lookups racing here both fetch; the loser's result is
    // dropped. Take the write lock across the fetch if that ever matters.
    let (fresh, _, warning) = fetch_riven_db(false);
    if let Some(w) = warning {
        warn!("{w}");
    }
    let mut guard = RIVEN_DB.write().unwrap_or_else(|e| e.into_inner());
    // A failed load must not be latched: leaving the slot empty is what makes
    // the next lookup fetch again instead of serving nothing for the rest of
    // the session. A concurrent load that won the race is likewise kept.
    if guard.is_none() && !fresh.is_empty() {
        *guard = Some(fresh);
    }
    match guard.as_ref() {
        Some(db) => f(db),
        None => f(&HashMap::new()),
    }
}

/// Walk the cache ladder for the riven database: a fresh copy on disk, else the
/// sheet, else the stale copy, else nothing. `force` skips the first rung.
fn fetch_riven_db(force: bool) -> (HashMap<String, RivenEntry>, cache::Source, Option<String>) {
    let ttl = if force { std::time::Duration::ZERO } else { RIVEN_DB_TTL };
    let (data, source, warning) = cache::get_or_refresh(RIVEN_DB_CACHE, ttl, |_etag| {
        // Google's CSV export carries no usable validator, so every refresh
        // pulls all five tabs.
        load_riven_csv_from_url().map(|db| cache::Fetched::New(db, None))
    });
    (data.unwrap_or_default(), source, warning)
}

/// Background-refresh entry point. A refetch that came back empty leaves the
/// database that is already loaded in place.
pub(crate) fn refresh_riven_db_task(_app: &tauri::AppHandle, force: bool) -> Result<(), String> {
    let (fresh, _, warning) = fetch_riven_db(force);
    if !fresh.is_empty() {
        *RIVEN_DB.write().unwrap_or_else(|e| e.into_inner()) = Some(fresh);
    }
    match warning {
        Some(w) => Err(w),
        None => Ok(()),
    }
}

const RIVEN_SHEET_ID: &str = "1zbaeJBuBn44cbVKzJins_E3hTDpnmvOk8heYN-G8yy8";
// Tabs: 0=primary, 1505239276=secondary, 1413904270=melee, 289737427=archwing, 965095749=other
// 1687910063 is the legend/info page — skip it
const RIVEN_SHEET_GIDS: &[u64] = &[0, 1505239276, 1413904270, 289737427, 965095749];

fn load_riven_csv_from_url() -> Result<HashMap<String, RivenEntry>, String> {
    let mut combined = HashMap::new();
    for &gid in RIVEN_SHEET_GIDS {
        let url = format!(
            "https://docs.google.com/spreadsheets/d/{}/export?format=csv&gid={}",
            RIVEN_SHEET_ID, gid
        );
        match ureq::get(&url)
            .set("User-Agent", "FrameForge/3.2.0")
            .call().map_err(|e| e.to_string())
            .and_then(|r| r.into_string().map_err(|e| e.to_string()))
        {
            Ok(csv) => { combined.extend(parse_riven_csv(&csv)); }
            Err(e) => { warn!(gid, error = %e, "failed to load riven sheet tab"); }
        }
    }
    if combined.is_empty() {
        return Err("No riven data loaded from any sheet tab".into());
    }
    Ok(combined)
}

fn parse_riven_csv(csv: &str) -> HashMap<String, RivenEntry> {
    let mut map = HashMap::new();
    let mut lines = csv.lines();

    // Read header to find which column holds "NEGATIVE STATS:" — it varies by tab
    let header = match lines.next() { Some(h) => h, None => return map };
    let hf = csv_split_line(header);
    let neg_col = hf.iter().position(|c| c.trim().to_lowercase().contains("negative")).unwrap_or(5);
    let notes_col = hf.iter().position(|c| c.trim().to_lowercase().contains("note")).unwrap_or(8);

    for line in lines {
        let f = csv_split_line(line);
        if f.len() < neg_col + 1 { continue; }
        let weapon = f[0].trim().to_lowercase();
        if weapon.is_empty() { continue; }
        let stat_alternatives = parse_stat_alternatives(&f[1]);
        let stat_groups = parse_stat_groups(&f[1]);
        let safe_neg    = parse_riven_stat_str(&f[neg_col]);
        let raw_notes   = f.get(notes_col).map(|s| s.trim().trim_matches('"').to_string()).unwrap_or_default();
        let notes       = expand_abbrevs_in_notes(&raw_notes);
        map.insert(weapon.clone(), RivenEntry { weapon, stat_alternatives, stat_groups, safe_negatives: safe_neg, notes });
    }
    map
}

/// Like ocr_stat_to_full but first tries the full conditional name, then strips "for X" and retries.
/// "Critical Chance for Slide Attack" → "Slide Critical Chance" (full wins)
/// "Critical Damage for Slide Attack" → stripped → "Critical Damage" (full doesn't match, fallback)
fn ocr_stat_to_full_with_condition(ocr_name: &str) -> String {
    let full_try = ocr_stat_to_full(ocr_name);
    if full_try != ocr_name {
        return full_try; // matched on full name
    }
    // Strip "for <condition>" and try again
    let stripped = ocr_name.split(" for ").next().unwrap_or(ocr_name).trim();
    if stripped != ocr_name {
        let stripped_try = ocr_stat_to_full(stripped);
        if stripped_try != stripped {
            return stripped_try;
        }
    }
    full_try // return best effort even if unrecognized
}

/// In-game stat names → database full names (handles abbreviations and element icons stripped by OCR)
fn ocr_stat_to_full(ocr_name: &str) -> String {
    // Strip leading OCR artifacts from element icons (e.g. "61-leat" → "leat" from 🔥Heat,
    // "ld" from ❄Cold, etc.) before pattern matching.
    let stripped = ocr_name.trim().trim_start_matches(|c: char| !c.is_alphabetic());
    let n = stripped.to_lowercase();
    match n.as_str() {
        // Conditional melee stats — checked FIRST so "critical chance for slide attack" wins
        // over the generic "critical chance" pattern below
        s if s.contains("critical chance") && (s.contains("slide") || s.contains("slide attack")) => "Slide Critical Chance",
        s if s.contains("critical chance") && s.contains("aerial") => "Aerial Critical Chance",
        s if s.contains("critical chance") && s.contains("wall") => "Wall Critical Chance",
        s if s.contains("critical damage") || s.contains("crit. damage") || s.contains("crit damage") => "Critical Damage",
        s if s.contains("critical chance") || s.contains("crit. chance") || s.contains("crit chance") => "Critical Chance",
        s if s.contains("multishot") => "Multishot",
        s if s.contains("fire rate") => "Fire Rate",
        s if s.contains("status chance") => "Status Chance",
        s if s.contains("base damage") || (s.contains("damage") && !s.contains("critical") && !s.contains("infested") && !s.contains("grineer") && !s.contains("corpus")) => "Base Damage",
        // Toxin — icon may eat 'T', leaving "oxin" or "oxicity"
        s if s.contains("toxin") || s.contains("toxicity") || s.starts_with("oxin") => "Toxicity",
        // Heat — fire icon may eat 'H', leaving "eat" or "leat"
        s if s.contains("heat") || s.contains("fire damage")
            || s == "eat" || s == "leat" || (s.ends_with("eat") && s.len() <= 7) => "Heat",
        // Electricity — icon may eat 'E', leaving "lectricity" etc.
        s if s.contains("electricity") || s.contains("electric") || s.starts_with("lectr") => "Electricity",
        // Cold — ice icon may eat 'C', leaving "old"
        s if s.contains("cold") || s.contains("freeze") || s == "old" => "Cold",
        s if s.contains("punch through") => "Punch Through",
        s if s.contains("reload speed") || s.contains("reload") => "Reload Speed",
        s if s.contains("magazine size") || s.contains("magazine") || s.contains("mag size") => "Magazine Size",
        s if s.contains("ammo max") || s.contains("ammo maximum") => "Ammo Maximum",
        s if s.contains("zoom") => "Zoom",
        s if s.contains("recoil") => "Recoil",
        s if s.contains("slash") => "Slash",
        s if s.contains("puncture") => "Puncture",
        s if s.contains("impact") => "Impact",
        s if s.contains("flight speed") || s.contains("proj. flight") || s.contains("projectile") => "Projectile Flight Speed",
        s if s.contains("status duration") => "Status Duration",
        s if s.contains("infested") => "Damage to Infested",
        s if s.contains("grineer") => "Damage to Grineer",
        s if s.contains("corpus") => "Damage to Corpus",
        // Melee-specific stats
        s if s.contains("attack speed") || s.contains("attack spd") => "Attack Speed",
        s if s.contains("combo duration") => "Combo Duration",
        s if s.contains("combo count") => "Combo Count Chance",
        s if s.contains("heavy attack") && s.contains("efficiency") => "Heavy Attack Efficiency",
        s if s.contains("heavy attack") => "Heavy Attack Damage",
        s if s.contains("slam") => "Slam Attack",
        s if s.contains("slide") && s.contains("crit") => "Slide Critical Chance",
        s if s.contains("range") => "Range",
        _ => return ocr_name.to_string(),
    }.to_string()
}

/// Parse stat lines from a card's OCR text, returning rolled_stats JSON array.
fn parse_original_stats(text: Option<&str>) -> Vec<serde_json::Value> {
    let Some(text) = text else { return vec![]; };
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.to_lowercase().starts_with('x') && l.len() > 2 && l.chars().nth(1).map_or(false, |c| c.is_ascii_digit() || c == ' ') {
            let alpha_start = l.find(|c: char| c.is_alphabetic() && c != 'x').unwrap_or(l.len());
            let val = l[..alpha_start].split_whitespace().collect::<Vec<_>>().join("");
            let name_part = l[alpha_start..].trim().split(" (").next().unwrap_or("").trim();
            if !name_part.is_empty() {
                out.push(serde_json::json!({"name": ocr_stat_to_full_with_condition(name_part), "value": val, "positive": true}));
            }
            continue;
        }
        let fc = l.chars().next().unwrap_or(' ');
        let (is_pos, part) = if l.starts_with('+') { (true, l.trim_start_matches('+')) }
                             else if l.starts_with('-') { (false, l.trim_start_matches('-')) }
                             else if "•·○●◦".contains(fc) { (true, l.trim_start_matches(|c: char| "•·○●◦".contains(c))) }
                             else { continue; };
        let val = if part.contains('%') {
            let n = part.split('%').next().unwrap_or("").trim();
            format!("{}{}%", if is_pos { "+" } else { "-" }, n)
        } else {
            let e = part.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(part.len());
            format!("{}{}%", if is_pos { "+" } else { "-" }, &part[..e])
        };
        let sname: &str = if let Some(a) = part.splitn(2, '%').nth(1) { a.trim() }
                          else { let e = part.find(|c: char| c.is_alphabetic()).unwrap_or(0);
                                 part[e..].trim_start_matches(|c: char| !c.is_alphabetic()) };
        if sname.is_empty() { continue; }
        let sname = sname.trim_start_matches(|c: char| !c.is_alphabetic());
        let sname = sname.split(" (").next().unwrap_or(sname).trim();
        out.push(serde_json::json!({"name": ocr_stat_to_full_with_condition(sname), "value": val, "positive": is_pos}));
    }
    out
}

/// Capture the riven reroll screen and OCR the stats + weapon name.
/// Returns (weapon_name, positives, negatives).
#[tauri::command]
pub(crate) async fn ocr_riven_screen() -> Result<serde_json::Value, String> {
    let riven_log = paths::state_dir().join("frameforge_riven_session.txt");
    let ts1 = chrono::Local::now().format("%H:%M:%S%.3f").to_string();

    let _ = append_to_file(&riven_log, &format!(
        "[STEP 2] OCR STARTED — {}\n\
         ├─ Capture region : y 0%–75% (header + card + FITS IN panel)\n\
         └─ Validating: expects \"INVENTORY/MODS\" at top + \"FITS IN\" on right\n",
        ts1
    ));

    // Capture y 0–0.75: includes the "INVENTORY / MODS" header at the top and the
    // "FITS IN" weapon panel on the right. We retry until both markers are visible —
    // this filters out false EE.log triggers and handles slow screen transitions.
    const MAX_ATTEMPTS: u32 = 6;
    const RETRY_MS: u64 = 350;

    let mut text = String::new();
    let mut full_text_for_fallback = String::new();
    let mut panel_for_weapon = String::new();
    let mut confirmed = false;

    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(RETRY_MS)).await;
        }

        let riven_log2 = riven_log.clone();
        // One PrintWindow capture; two OCR passes from the same pixels:
        //   • Full width (0–100%) for validation markers ("INVENTORY/MODS" + "FITS IN")
        //   • Card column only (20–65%) for stat parsing — excludes the right panel whose
        //     "FITS IN" / weapon label text can interfere with reading the card's bottom stats.
        let attempt_result = tokio::task::spawn_blocking(move || {
            let ts = chrono::Local::now().format("%H:%M:%S%.3f").to_string();
            let px = ocr::capture_warframe_pixels().map_err(|e| format!("Capture: {}", e))?;
            let (pixels, w, h) = px;
            let full_text = ocr::ocr_pixels_rect(&pixels, w, h, 0.0, 1.0, 0.0, 0.82)
                .unwrap_or_default();
            let card_text = ocr::ocr_pixels_rect(&pixels, w, h, 0.20, 0.65, 0.28, 0.82)
                .unwrap_or_default();
            let panel_text = ocr::ocr_pixels_rect_raw(&pixels, w, h, 0.73, 1.0, 0.30, 0.95)
                .unwrap_or_default();
            let _ = append_to_file(&riven_log2, &format!(
                "[STEP 2] OCR attempt {} — {}\n├─ Full text:\n{}\n├─ Panel text:\n{}\n└─ Card text:\n{}\n\n",
                attempt + 1, ts, full_text, panel_text, card_text
            ));
            Ok::<_, String>((full_text, panel_text, card_text))
        }).await.map_err(|e| format!("Task: {}", e))??;

        let (full_text, panel_text, card_text) = attempt_result;
        let lower = full_text.to_lowercase();
        let has_header  = lower.contains("inventory") || lower.contains("mods");
        let has_fits_in = says_fits_in(&lower) || says_fits_in(&panel_text.to_lowercase());

        let _ = append_to_file(&riven_log, &format!(
            "[STEP 2] attempt {} — header={} fits_in={}\n",
            attempt + 1, has_header, has_fits_in
        ));

        // Count stat lines in card_text — 5+ means comparison mode (two cards visible).
        // In comparison mode the "FITS IN" panel shifts and may not OCR correctly.
        // Accept header-only confirmation when we already see enough stat lines.
        let stat_count = card_text.lines()
            .filter(|l| { let t = l.trim(); t.starts_with('+') || t.starts_with('-') })
            .count();
        let comparison_likely = stat_count >= 5;

        if (has_header && has_fits_in) || (has_header && comparison_likely) {
            text = card_text;
            full_text_for_fallback = full_text;
            panel_for_weapon = panel_text;
            confirmed = true;
            if comparison_likely && !has_fits_in {
                let _ = append_to_file(&riven_log, &format!(
                    "[STEP 2] Comparison mode early-confirm ({} stat lines, no FITS IN)\n", stat_count
                ));
            }
            break;
        }
        text = card_text;
        full_text_for_fallback = full_text;
        panel_for_weapon = panel_text;
    }

    if !confirmed {
        let _ = append_to_file(&riven_log, "[STEP 2] Screen markers not confirmed after all attempts — proceeding with last OCR result anyway\n\n");
    }

    // Detect comparison mode: >4 stat lines means two cards are visible (3–4 stats each).
    // A riven can have at most 4 stats (3 pos + 1 neg), so 5+ total implies 2 cards.
    let stat_line_count = text.lines()
        .filter(|l| { let t = l.trim(); t.starts_with('+') || t.starts_with('-') })
        .count();
    let is_comparison = stat_line_count > 4;

    if is_comparison {
        let _ = append_to_file(&riven_log, &format!(
            "[STEP 2] COMPARISON MODE detected ({} stat lines) — capturing card columns separately\n", stat_line_count
        ));
    }

    // In comparison mode: one PrintWindow capture, OCR left and right card columns.
    // Original card is ALWAYS on the left; new roll is always on the right.
    // Card area x 20–65% is split roughly in half: left=20–42%, right=42–65%.
    let (left_text, right_text) = if is_comparison {
        let riven_log3 = riven_log.clone();
        let cols = tokio::task::spawn_blocking(move || {
            match ocr::capture_warframe_pixels() {
                Ok((px, w, h)) => {
                    // Wider y range to catch element-icon stat lines near card bottom
                    let left  = ocr::ocr_pixels_rect(&px, w, h, 0.18, 0.44, 0.25, 0.84).unwrap_or_default();
                    let right = ocr::ocr_pixels_rect(&px, w, h, 0.44, 0.68, 0.25, 0.84).unwrap_or_default();
                    let _ = append_to_file(&riven_log3, &format!(
                        "[STEP 2] Original (left):\n{}\n\nNew roll (right):\n{}\n\n", left, right
                    ));
                    (left, right)
                }
                Err(e) => {
                    let _ = append_to_file(&riven_log3, &format!("[STEP 2] Column capture failed: {}\n", e));
                    (String::new(), String::new())
                }
            }
        }).await.map_err(|e| format!("Task: {}", e))?;
        cols
    } else {
        (String::new(), String::new())
    };

    // Which text to parse for the new roll:
    // - Comparison mode: right column = new roll, left column = original
    // - Single card mode: card column text; fall back to full text if card column had no stats
    let card_has_stats = text.lines().any(|l| { let t = l.trim(); t.starts_with('+') || t.starts_with('-') });
    let parse_text = if is_comparison && !right_text.is_empty() {
        &right_text
    } else if !card_has_stats && !full_text_for_fallback.is_empty() {
        // Card column empty — fall back to the full-width validated text
        let _ = append_to_file(&riven_log, "[STEP 2] Card column had no stats — using full-width text as fallback\n");
        &full_text_for_fallback
    } else {
        &text
    };
    let original_parse_text = if is_comparison && !left_text.is_empty() { Some(left_text.as_str()) } else { None };

    // Parse weapon name.
    // In the unveil screen "FITS IN" appears on its own line, weapon name on the next line.
    // In the reroll screen the mod name is "WeaponName RivenIdentifier" (e.g. "Hirudo Geli-plecinus").
    let lines: Vec<&str> = parse_text.lines().collect();

    // Helper: try to match a candidate string against the riven DB, trying word-prefix
    // substrings from longest to shortest (handles "Dual Cleavers Cronitron" → "dual cleavers").
    let find_in_db = |candidate: &str| -> Option<String> {
        with_riven_db(|db| {
            let words: Vec<&str> = candidate.split_whitespace().collect();
            for len in (1..=words.len().min(4)).rev() {
                let prefix = words[..len].join(" ");
                if db.contains_key(&prefix) {
                    return Some(prefix);
                }
            }
            None
        })
    };

    // The "FITS IN" panel is the only place the game states the weapon outright,
    // and it states the real one: a Kuva Nukor riven is titled "Nukor Crita-
    // hexapha" above the card, which resolves to the ordinary Nukor and its
    // different disposition.
    //
    // The grading sheet is a curated list, not a weapon index. It carries
    // "kuva bramma" but not "kuva nukor", so a panel name it does not know is
    // still the right answer. Reporting it unmatched costs the roll analysis
    // (analyze_riven returns nothing for an unknown weapon, which the UI
    // already handles) and buys not silently grading a Kuva Nukor as the base
    // Nukor it is titled after, on a different disposition.
    let panel_candidates = panel_weapon_candidates(&panel_for_weapon);
    let weapon = panel_candidates.iter()
        .find_map(|l| find_in_db(l))
        .or_else(|| panel_candidates.last().cloned())
        .or_else(|| lines.iter().enumerate()
            .find(|(_, l)| says_fits_in(&l.to_lowercase()))
            .and_then(|(i, _)| lines.get(i + 1))
            .and_then(|l| {
                let lc = l.trim().to_lowercase();
                find_in_db(&lc).or(Some(lc))
            }))
        // Fallback: first non-stat, non-UI line is the mod name "WeaponName RivenId".
        // Only accept if it matches a weapon in the DB — avoids returning currency values
        // like "D '5,598" (Endo count) that pass the basic filter.
        .or_else(|| {
            lines.iter()
                .find_map(|l| {
                    let lt = l.trim().to_lowercase();
                    if lt.is_empty() { return None; }
                    // Skip UI noise. "kuva" is deliberately absent: it prefixes a
                    // whole weapon family, so skipping it lost the name of every
                    // Kuva riven. "Remaining Kuva 102,773" is already caught by
                    // "remaining" and the currency-value rules below.
                    if lt.contains("fits in") || lt.contains("cycle")
                    || lt.contains("mr ") || lt.contains("inventory") || lt.contains("mods")
                    || lt.contains("remaining") || lt.contains("show ranked") || lt.contains("cancel")
                    || lt.starts_with('+') || lt.starts_with('-') || lt.starts_with('x')
                    || lt.chars().next().map_or(false, |c| c.is_ascii_digit())
                    // Skip lines that look like currency values (contain digit+comma or digit+apostrophe)
                    || (lt.contains(',') && lt.chars().any(|c| c.is_ascii_digit()))
                    || (lt.contains('\'') && lt.chars().any(|c| c.is_ascii_digit()))
                    {
                        return None;
                    }
                    find_in_db(&lt) // only return if it's actually in the DB
                })
        })
        .unwrap_or_default();

    let joined = join_wrapped_stat_lines(parse_text);

    // Parse stat lines and collect rolled_stats (name + formatted value for display).
    let mut positives: Vec<String> = Vec::new();
    let mut negatives: Vec<String> = Vec::new();
    // Each entry: { "name": "Combo Count Chance", "value": "+47.2%", "positive": true }
    let mut rolled_stats: Vec<serde_json::Value> = Vec::new();

    for line in &joined {
        let l = line.trim();

        // Handle multiplier format "x1.62 Damage to Corpus"
        // OCR may insert spaces inside the number ("x1 .62"), so collect everything
        // before the first alphabetic char and join to remove those spaces.
        if l.to_lowercase().starts_with('x') && l.len() > 2 && l.chars().nth(1).map_or(false, |c| c.is_ascii_digit() || c == ' ') {
            let alpha_start = l.find(|c: char| c.is_alphabetic() && c != 'x').unwrap_or(l.len());
            let val_str = l[..alpha_start].split_whitespace().collect::<Vec<_>>().join(""); // e.g. "x1.62"
            let stat_name = l[alpha_start..].trim();
            let stat_name = stat_name.split(" (").next().unwrap_or(stat_name).trim();
            if !stat_name.is_empty() {
                let full = ocr_stat_to_full_with_condition(stat_name);
                rolled_stats.push(serde_json::json!({"name": full, "value": val_str, "positive": true}));
                positives.push(full);
            }
            continue;
        }

        let first_l = l.chars().next().unwrap_or(' ');
        let (is_pos, stat_part) = if l.starts_with('+') {
            (true, l.trim_start_matches('+'))
        } else if l.starts_with('-') {
            (false, l.trim_start_matches('-'))
        } else if "•·○●◦".contains(first_l) {
            // OCR misread '+' as a bullet/dot character — treat as positive stat
            (true, l.trim_start_matches(|c: char| "•·○●◦".contains(c)))
        } else { continue; };

        // Extract the numeric value string.
        // Must explicitly check for '%' first — split('%').next() returns Some(whole_string)
        // even when no '%' is present, which would produce "+51 'Toxin%" for element stats.
        let pct_val = if stat_part.starts_with("?%") {
            // Synthesised from orphan stat — OCR dropped the x-multiplier value
            "x?".to_string()
        } else if stat_part.contains('%') {
            let n = stat_part.split('%').next().unwrap_or("").trim();
            format!("{}{}%", if is_pos { "+" } else { "-" }, n)
        } else {
            // No % sign (element stats, OCR dropped it) — extract leading digits only
            let num_end = stat_part.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(stat_part.len());
            format!("{}{}%", if is_pos { "+" } else { "-" }, &stat_part[..num_end])
        };

        // Extract stat name
        let stat_name: &str = if let Some(after_pct) = stat_part.splitn(2, '%').nth(1) {
            after_pct.trim()
        } else {
            let num_end = stat_part.find(|c: char| c.is_alphabetic()).unwrap_or(0);
            stat_part[num_end..].trim_start_matches(|c: char| !c.is_alphabetic())
        };
        if stat_name.is_empty() { continue; }

        // Strip leading OCR icon artifacts: "61-leat" → "leat", " 🔥Heat" → "Heat"
        let stat_name = stat_name.trim_start_matches(|c: char| !c.is_alphabetic());
        if stat_name.is_empty() { continue; }

        // Strip parenthetical qualifiers: "Critical Chance (x2 for Heavy Attacks)" → "Critical Chance"
        let stat_name = stat_name.split(" (").next().unwrap_or(stat_name).trim();

        // Try to match with the full conditional name first so "Critical Chance for Slide Attack"
        // maps to "Slide Critical Chance" (not just "Critical Chance"). Fall back to stripped form.
        let full = ocr_stat_to_full_with_condition(stat_name);
        rolled_stats.push(serde_json::json!({"name": full, "value": pct_val, "positive": is_pos}));
        if is_pos { positives.push(full); } else { negatives.push(full); }
    }

    let ts3 = chrono::Local::now().format("%H:%M:%S%.3f").to_string();
    let _ = append_to_file(&riven_log, &format!(
        "[STEP 3] PARSE RESULT — {}\n\
         ├─ Weapon    : \"{}\"\n\
         ├─ Positives : {:?}\n\
         └─ Negatives : {:?}\n\n",
        ts3, weapon, positives, negatives
    ));

    Ok(serde_json::json!({
        "weapon": weapon,
        "positives": positives,
        "negatives": negatives,
        "rolled_stats": rolled_stats,
        "is_comparison": is_comparison,
        "original_rolled_stats": parse_original_stats(original_parse_text),
        "raw": text,
    }))
}

/// 3-state riven screen status:
///  "open"    = inventory header visible + "FITS IN" on right panel
///  "closed"  = inventory header visible + "FITS IN" gone (user exited riven screen)
///  "unknown" = inventory header not visible (alt-tabbed, or left inventory entirely)
#[tauri::command]
pub(crate) fn riven_screen_status() -> String {
    let riven_log = paths::state_dir().join("frameforge_riven_session.txt");
    let ts = chrono::Local::now().format("%H:%M:%S%.3f").to_string();

    let Ok((pixels, w, h)) = ocr::capture_warframe_pixels() else {
        let _ = append_to_file(&riven_log, &format!("[POLL {}] capture failed → unknown\n", ts));
        return "unknown".into();
    };

    let header = ocr::ocr_pixels_rect_raw(&pixels, w, h, 0.0, 0.55, 0.0, 0.10)
        .unwrap_or_default();
    let in_inventory = header.to_lowercase().contains("inventory");

    if !in_inventory {
        let _ = append_to_file(&riven_log, &format!("[POLL {}] no inventory header → unknown\n", ts));
        return "unknown".into();
    }

    let right = ocr::ocr_pixels_rect_raw(&pixels, w, h, 0.73, 1.0, 0.30, 0.80)
        .unwrap_or_default();
    let rl = right.to_lowercase();
    // In comparison mode "FITS IN" may be partially cut off, reading as "SIN", "IN", "TS IN" etc.
    // Accept any fragment that is a suffix of "FITS IN".
    let fits_in = rl.contains("fits in") || rl.contains("fits") || rl.contains("ts in")
        || rl.contains("its in") || (rl.trim() == "in") || (rl.trim() == "sin");
    let preview = right.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join(" | ");

    let status = if fits_in { "open" } else { "closed" };
    let _ = append_to_file(&riven_log, &format!(
        "[POLL {}] inventory=true fits_in={} ocr=\"{}\" → {}\n",
        ts, fits_in, truncate_chars(&preview, 80), status
    ));
    status.into()
}

/// Is the riven reroll screen still open?
/// Checks for "FITS IN" text on the right panel using RAW OCR (no preprocessing).
/// "FITS IN" is white text on dark — readable without grayscale conversion.
/// Only closes the overlay when Warframe is still focused (INVENTORY/MODS header present)
/// AND "FITS IN" is gone — so alt-tabbing away doesn't trigger a false close.
#[tauri::command]
pub(crate) fn riven_screen_visible() -> bool {
    let riven_log = paths::state_dir().join("frameforge_riven_session.txt");
    let ts = chrono::Local::now().format("%H:%M:%S%.3f").to_string();

    let Ok((pixels, w, h)) = ocr::capture_warframe_pixels() else {
        let _ = append_to_file(&riven_log, &format!("[POLL {}] capture failed → true (assume open)\n", ts));
        return true; // can't capture = can't confirm closed
    };

    // Check header (x 0–55%, y 0–10%) for "INVENTORY" — confirms Warframe is focused
    // and we're in the mods screen. If header is absent, user alt-tabbed; keep overlay.
    let header = ocr::ocr_pixels_rect_raw(&pixels, w, h, 0.0, 0.55, 0.0, 0.10)
        .unwrap_or_default();
    let in_inventory = header.to_lowercase().contains("inventory");

    if !in_inventory {
        let _ = append_to_file(&riven_log, &format!(
            "[POLL {}] no inventory header → true (alt-tabbed or different screen)\n", ts
        ));
        return true; // Warframe not in focus or wrong screen — don't close
    }

    // Check right panel (x 73–100%, y 30–80%) for "FITS IN"
    let right = ocr::ocr_pixels_rect_raw(&pixels, w, h, 0.73, 1.0, 0.30, 0.80)
        .unwrap_or_default();
    let fits_in_visible = right.to_lowercase().contains("fits");
    let right_preview = right.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join(" | ");

    let _ = append_to_file(&riven_log, &format!(
        "[POLL {}] inventory=true fits_in={} ocr=\"{}\"\n",
        ts, fits_in_visible, truncate_chars(&right_preview, 120)
    ));

    fits_in_visible
}

/// Read the single validity-flag byte that Overwolf GEP uses to track the riven reroll screen.
/// Non-zero = screen open; 0 = closed. Returns true on any error (fail-open avoids false closes).
/// The VA is found once via Pattern D-2 and cached; re-scanned only when the game restarts.
#[tauri::command]
/// Read the riven validity flag byte. Returns None if Warframe is not running.
/// Returns Some(true) = screen open, Some(false) = screen closed.
/// Fails open (Some(true)) on read errors so the overlay is never falsely dismissed.
///
/// Takes the PID from the caller: enumerating processes costs a `/proc` walk on
/// Linux and a snapshot on Windows, and the only caller polls five times a
/// second, so looking it up again here would double that for no new
/// information.
fn read_riven_flag_byte(pid: u32) -> Option<bool> {
    let cache = RIVEN_FLAG_VA.get_or_init(|| std::sync::Mutex::new(None));
    let mut cached = cache.lock().unwrap_or_else(|e| e.into_inner());
    if cached.map_or(true, |(p, _)| p != pid) {
        // Scan once per PID. Store (pid, None) if pattern not found so we don't re-scan every 200ms.
        let va = memory_scanner::find_riven_validity_va(pid);
        *cached = Some((pid, va));
    }
    let flag_va = match *cached {
        Some((_, Some(va))) => va,
        // Pattern not found for this PID — return None so the watcher ignores this tick.
        // Do NOT fail-open here: that would fire a false open event on every app start.
        Some((_, None)) | None => { return None; }
    };
    drop(cached);

    // Read failure means the mapping moved or access was lost, not that the
    // screen closed — fail open so an active overlay is never dismissed.
    Some(memory_scanner::read_process_byte(pid, flag_va).map_or(true, |byte| byte != 0))
}

/// Background thread: polls the riven validity flag every 200 ms and emits
/// riven-screen-open-mem / riven-screen-close-mem on state transitions.
/// Open fires on the first non-zero reading (fast). Close requires 2 consecutive
/// zero readings (400 ms) to avoid false dismissals.
#[tauri::command]
pub(crate) fn start_riven_memory_watcher(app: tauri::AppHandle) {
    use std::sync::atomic::Ordering;
    if RIVEN_WATCHER_RUNNING.swap(true, Ordering::SeqCst) {
        return; // already running — don't spawn a second thread
    }
    std::thread::spawn(move || {
        let mut prev_open = false;
        let mut close_streak: u8 = 0;
        let mut warframe_was_running = false;

        loop {
            std::thread::sleep(std::time::Duration::from_millis(200));

            let pid = memory_scanner::find_warframe_pid_pub();
            let Some(pid) = pid else {
                // Warframe not running — reset state
                if warframe_was_running {
                    prev_open = false;
                    close_streak = 0;
                    warframe_was_running = false;
                }
                continue;
            };
            warframe_was_running = true;

            match read_riven_flag_byte(pid) {
                None => {
                    // Warframe running but pattern VA not found yet — don't change state,
                    // just wait. This avoids a false open event on app start.
                }
                Some(true) => {
                    close_streak = 0;
                    if !prev_open {
                        prev_open = true;
                        let _ = app.emit("riven-screen-open-mem", ());
                    }
                }
                Some(false) => {
                    if prev_open {
                        close_streak += 1;
                        if close_streak >= 2 {
                            prev_open = false;
                            close_streak = 0;
                            let _ = app.emit("riven-screen-close-mem", ());
                        }
                    } else {
                        close_streak = 0;
                    }
                }
            }
        }
    });
}

/// Write an error into the riven session log (called from TypeScript when OCR command fails).
#[tauri::command]
pub(crate) fn ocr_riven_log_error(error: String) {
    let path = paths::state_dir().join("frameforge_riven_session.txt");
    let ts = chrono::Local::now().format("%H:%M:%S%.3f").to_string();
    let _ = append_to_file(&path, &format!(
        "[STEP 2] OCR COMMAND FAILED — {}\n└─ Error: {}\n\n", ts, error
    ));
}

// ── Saved rivens commands ─────────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn save_riven_roll(
    state: tauri::State<'_, AppState>,
    weapon: String, label: String, stats_json: String,
    verdict: String, score: f64,
) -> Result<String, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let count = crate::db::count_saved_rivens(&conn).unwrap_or(0);
    if count >= 50 {
        return Err("Maximum of 50 saved rivens reached. Delete some to save more.".into());
    }
    let id = format!("{:x}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    let saved_at = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
    let riven = crate::db::SavedRiven { id: id.clone(), weapon, label, stats_json, verdict, score, saved_at };
    crate::db::save_riven(&conn, &riven).map_err(|e| e.to_string())?;
    Ok(id)
}

#[tauri::command]
pub(crate) fn get_saved_riven_rolls(state: tauri::State<'_, AppState>) -> Result<Vec<crate::db::SavedRiven>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    crate::db::get_saved_rivens(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn delete_saved_riven_roll(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    crate::db::delete_saved_riven(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn rename_saved_riven_roll(state: tauri::State<'_, AppState>, id: String, label: String) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    crate::db::rename_saved_riven(&conn, &id, &label).map_err(|e| e.to_string())
}

/// Return all weapon names that have riven data.
#[tauri::command]
pub(crate) fn get_riven_weapons() -> Vec<String> {
    let mut weapons: Vec<String> = with_riven_db(|db| db.keys().cloned().collect());
    weapons.sort();
    weapons
}

/// Reload the riven database from the Google Sheet.
#[tauri::command]
pub(crate) fn reload_riven_database() -> Result<usize, String> {
    let (fresh, source, warning) = fetch_riven_db(true);
    let count = fresh.len();
    if count > 0 {
        *RIVEN_DB.write().unwrap_or_else(|e| e.into_inner()) = Some(fresh);
    }
    // The user asked for the current sheet, so a stale copy is not an answer
    // even though it is still worth showing.
    if source != cache::Source::Refreshed {
        return Err(warning.unwrap_or_else(|| "Failed to load riven database.".to_string()));
    }
    Ok(count)
}

/// Analyse a riven roll for a given weapon.
/// positives / negatives are full stat names (e.g. "Critical Damage", "Zoom").
#[tauri::command]
pub(crate) fn analyze_riven(weapon: String, positives: Vec<String>, negatives: Vec<String>) -> Option<RivenAnalysis> {
    let key = weapon.to_lowercase();
    let entry = with_riven_db(|db| db.get(&key).cloned())?;

    let normalize = |s: &str| s.to_lowercase();

    // Score every "or" alternative independently — collect all results, pick best.
    let make_verdict = |s: f32, neg_ok: bool| -> String {
        match (s, neg_ok) {
            (s, true)  if s >= 0.80 => "GREAT ROLL — Consider keeping".into(),
            (s, true)  if s >= 0.60 => "GOOD ROLL — Decent for selling".into(),
            (s, _)     if s >= 0.40 => "MEDIOCRE — Keep rolling".into(),
            _                        => "BAD ROLL — Keep rolling".into(),
        }
    };
    // neg_ok = no harmful negatives rolled (i.e. rolled negs are NOT in the bad list)
    let neg_ok_pre = negatives.iter().all(|neg| {
        !entry.safe_negatives.iter().any(|s| normalize(s) == normalize(neg))
    });

    let mut all_alternatives: Vec<AlternativeResult> = Vec::new();
    let mut best_matched: Vec<String> = Vec::new();
    let mut best_missing: Vec<String> = Vec::new();
    let mut best_score: f32 = -1.0_f32;

    for (idx, alternative) in entry.stat_alternatives.iter().enumerate() {
        if alternative.is_empty() { continue; }
        let mut m: Vec<String> = Vec::new();
        let mut ms: Vec<String> = Vec::new();
        for group in alternative {
            let hit = positives.iter().find(|p| group.iter().any(|g| normalize(g) == normalize(p)));
            if let Some(stat) = hit { m.push(stat.clone()); }
            else { ms.push(group.join(" / ")); }
        }
        let s = m.len() as f32 / alternative.len() as f32;
        let label = if entry.stat_alternatives.len() == 1 {
            "Build".to_string()
        } else {
            format!("Option {}", idx + 1)
        };
        all_alternatives.push(AlternativeResult {
            label, matched: m.clone(), missing: ms.clone(),
            score: s, verdict: make_verdict(s, neg_ok_pre),
        });
        let better = s > best_score || (s == best_score && m.len() > best_matched.len());
        if better { best_score = s; best_matched = m; best_missing = ms; }
    }

    let matched = best_matched;
    let missing = best_missing;
    let score   = if best_score < 0.0 { 0.0 } else { best_score };
    let total   = entry.stat_alternatives.iter().map(|a| a.len()).min().unwrap_or(1).max(1);

    // The spreadsheet "NEGATIVE STATS" column lists HARMFUL negatives to avoid.
    // Any negative NOT in that list is safe (doesn't matter for this weapon).
    let mut safe_present: Vec<String> = Vec::new();
    let mut harmful: Vec<String> = Vec::new();
    for neg in &negatives {
        if entry.safe_negatives.iter().any(|s| normalize(s) == normalize(neg)) {
            harmful.push(neg.clone());      // listed = BAD for this weapon
        } else {
            safe_present.push(neg.clone()); // not listed = safe/irrelevant
        }
    }
    let neg_ok = harmful.is_empty();

    let verdict = match (score, neg_ok) {
        (s, true)  if s >= 0.80 => "GREAT ROLL — Consider keeping".to_string(),
        (s, true)  if s >= 0.60 => "GOOD ROLL — Decent for selling".to_string(),
        (s, _)     if s >= 0.40 => "MEDIOCRE — Keep rolling".to_string(),
        _                        => "BAD ROLL — Keep rolling".to_string(),
    };

    Some(RivenAnalysis {
        weapon: entry.weapon.clone(),
        matched_positives: matched,
        missing_positives: missing,
        safe_negatives_present: safe_present,
        harmful_negatives: harmful,
        total_wanted: total,
        score,
        verdict,
        notes: entry.notes.clone(),
        alternatives: all_alternatives,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_chars_splits_on_characters_not_bytes() {
        assert_eq!(truncate_chars("éé", 3), "éé");
        assert_eq!(truncate_chars("éé", 1), "é");
        assert_eq!(truncate_chars("abc", 2), "ab");
    }

    /// Verbatim OCR for the right-hand card of a reroll comparison screen (Kuva
    /// Bramma, 3840×2160), border and rank pips included as punctuation.
    const KUVA_BRAMMA_CARD_OCR: &str = "\
Kuva Bramma Lexi-
==
fevatin
;
-
+1.4 Punch Through
;
+22.2% Magazine
_
Capacity
-
-
\"
+23.6% Reload Speed ¢ Y
MR11
K Y
S
e
";

    #[test]
    fn wrapped_stat_names_rejoin_without_the_card_border() {
        let joined = join_wrapped_stat_lines(KUVA_BRAMMA_CARD_OCR);
        assert_eq!(
            joined,
            vec![
                "+1.4 Punch Through",
                // Wrapped across two lines with a border fragment between the halves.
                "+22.2% Magazine Capacity",
                // Trailing "¢ Y" is part of the line, so it survives; "MR11" does not.
                "+23.6% Reload Speed ¢ Y",
            ]
        );

        // All three still resolve. Trailing debris is fine, debris *inside* a
        // name ("Magazine _ Capacity") is not.
        assert_eq!(ocr_stat_to_full_with_condition("Magazine Capacity"), "Magazine Size");
        assert_eq!(ocr_stat_to_full_with_condition("Reload Speed ¢ Y"), "Reload Speed");
        assert_eq!(ocr_stat_to_full_with_condition("Punch Through"), "Punch Through");
    }

    /// Both cards of a Kuva Nukor reroll screen. The left card's artwork puts
    /// stray glyphs in front of a sign, and its "Magazine Capacity" wraps.
    #[test]
    fn stats_survive_glyphs_in_front_of_the_sign() {
        let joined = join_wrapped_stat_lines("\
Nukor Mantitin
)
+30.9% Magazine
Capacity
x1.29 Damage to Corpus P
v & -34.3% Critical Chance
H
O\\
M
");
        assert_eq!(
            joined,
            vec![
                "+30.9% Magazine Capacity",
                "x1.29 Damage to Corpus P",
                // Without the prefix trim this line joined onto the multiplier
                // above it, losing both stats in one go.
                "-34.3% Critical Chance",
            ]
        );

        // The new roll, whose only oddity is the element icon read as "W".
        let joined = join_wrapped_stat_lines("\
\\ukor Crita-hexapha
+76.6% Critical Chance
;
+43.3% Status Chance
+39.9% W Heat
p
-74.7% Damage
g
MR13
X N,
");
        assert_eq!(
            joined,
            vec![
                "+76.6% Critical Chance",
                "+43.3% Status Chance",
                "+39.9% W Heat",
                "-74.7% Damage",
            ]
        );
        // Constructed, from the multi-byte glyphs this OCR emits elsewhere on the
        // card: four characters but six bytes, so a byte bound would leave it.
        assert_eq!(
            join_wrapped_stat_lines("x1.29 Damage to Corpus P\n¢ ¥ -34.3% Critical Chance\n"),
            vec!["x1.29 Damage to Corpus P", "-34.3% Critical Chance"]
        );

        assert_eq!(ocr_stat_to_full_with_condition("W Heat"), "Heat");
        assert_eq!(ocr_stat_to_full_with_condition("Damage to Corpus P"), "Damage to Corpus");
    }

    /// The trim that rescues a stat could also destroy one, so it is bounded from
    /// both sides: reach the multiplier in either case, stop at anything wordlike.
    #[test]
    fn debris_trimming_stops_at_a_word_boundary() {
        // The multiplier is matched case-insensitively elsewhere, so debris in
        // front of a capital "X" has to be trimmed too.
        assert_eq!(
            join_wrapped_stat_lines("+50% Critical Chance\nv & X1.29 Damage to Corpus\n"),
            vec!["+50% Critical Chance", "X1.29 Damage to Corpus"]
        );

        // A sign glued to a word is part of it: this is a name wrapping
        // mid-hyphen, and trimming would leave "-1oad Speed".
        assert_eq!(
            join_wrapped_stat_lines("+50% Critical Chance\nRe-1oad Speed\n"),
            vec!["+50% Critical Chance Re-1oad Speed"]
        );

        // The rank label would otherwise trim into "-1" and read as a curse.
        assert_eq!(
            join_wrapped_stat_lines("+50% Critical Chance\nMR-1\n"),
            vec!["+50% Critical Chance"]
        );
    }

    /// Verbatim panel OCR from three reroll screens. The weapon name has to
    /// survive whether or not the grading sheet lists it: "kuva nukor" is not in
    /// the sheet, and reporting the base Nukor in its place would grade the roll
    /// against a different weapon's disposition.
    #[test]
    fn the_panel_yields_the_weapon_name_over_its_own_chrome() {
        let nukor = "o\n=\n\\\n[\"\no\nIN\n\u{fb01} 'A l\u{2019}\u{2019})\n\u{2014}\nKuva Nukor\n";
        assert_eq!(panel_weapon_candidates(nukor).last().unwrap(), "kuva nukor");

        let bramma = "-\nD\n)\nA\n~\n3\n\u{00a5}\nFITSIN\ne\nKuva Bramma\nSHOW RANKED\n";
        assert_eq!(panel_weapon_candidates(bramma).last().unwrap(), "kuva bramma");

        // The single-card screen adds a CLOSE button below SHOW RANKED.
        let single = "\\\nE_ 3\n-\n-~\nFITSIN\n@\nKuva Bramma\nSHOW RANKED\nCLOSE\n";
        assert_eq!(panel_weapon_candidates(single).last().unwrap(), "kuva bramma");

        // A panel that read as nothing but debris must not name a weapon.
        assert!(panel_weapon_candidates("\u{201c} \\\\\n>~ \u{2018}\n").is_empty());
    }

    /// The label is small enough that an engine can close the word gap.
    #[test]
    fn the_fits_in_marker_is_matched_without_its_space() {
        assert!(says_fits_in("fitsin"));
        assert!(says_fits_in("fits in"));
        assert!(says_fits_in("e\nfitsin\nkuva bramma"));
        assert!(!says_fits_in("inventory/mods"));
    }

    /// Titles must not glue onto a stat, and a negative stat is a real curse
    /// rather than junk. The second title is the one that matters: it follows the
    /// card above with no blank line, and is what the "kuva" noise rule holds back.
    #[test]
    fn stat_joining_keeps_curses_and_drops_the_mod_name() {
        let joined = join_wrapped_stat_lines("\
Kuva Bramma Conci-
satitio
+50.3% Electricity
+57% Projectile Speed
+52.4% Multishot
-25.4% Ammo Maximum
Kuva Bramma Lexi-
MR 11
");
        assert_eq!(
            joined,
            vec![
                "+50.3% Electricity",
                "+57% Projectile Speed",
                "+52.4% Multishot",
                "-25.4% Ammo Maximum",
            ]
        );
    }
}
