use regex::Regex;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ParsedReward {
    pub item_name: String,
    pub quantity: i64,
    pub raw_line: String,
}

pub struct LogParser {
    patterns: Vec<Regex>,
}

impl LogParser {
    pub fn new() -> Self {
        let pattern_strings = vec![
            // "received ItemName x2"
            r"(?i)\breceived\s+([A-Za-z][A-Za-z0-9\s'\-]+?)\s+[xX]\s*(\d+)",
            // "reward: ItemName x2"
            r"(?i)\brewards?\s*[:\s]+([A-Za-z][A-Za-z0-9\s'\-]+?)\s+[xX]\s*(\d+)",
            // "Adding item: /path/ItemName x1"
            r"(?i)adding item.*?/([A-Za-z][A-Za-z0-9\s'\-]+?)\s+[xX]\s*(\d+)",
            // "ItemName x2" after mission/fissure keyword
            r"(?i)(?:mission|fissure|syndicate|foundry)[^\n]*?([A-Za-z][A-Za-z0-9\s'\-]{3,40}?)\s+[xX]\s*(\d+)",
            // "You received: ItemName x1"
            r"(?i)you received[:\s]+([A-Za-z][A-Za-z0-9\s'\-]+?)\s+[xX]\s*(\d+)",
        ];

        let patterns = pattern_strings
            .iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();

        Self { patterns }
    }

    pub fn parse_line(&self, line: &str) -> Option<ParsedReward> {
        for pattern in &self.patterns {
            if let Some(caps) = pattern.captures(line) {
                if let (Some(name), Some(qty_str)) = (caps.get(1), caps.get(2)) {
                    let item_name = name.as_str().trim().to_string();
                    let quantity: i64 = qty_str.as_str().parse().unwrap_or(1);

                    if item_name.len() < 3 || item_name.len() > 80 {
                        continue;
                    }

                    return Some(ParsedReward {
                        item_name,
                        quantity,
                        raw_line: line.to_string(),
                    });
                }
            }
        }
        None
    }

    pub fn parse_file_from_offset(&self, path: &Path, offset: u64) -> (Vec<ParsedReward>, u64) {
        let file = match File::open(path) {
            Ok(f) => f,
            Err(_) => return (vec![], offset),
        };

        let file_size = file.metadata().map(|m| m.len()).unwrap_or(0);

        // File was rotated (Warframe restarted) — start from beginning
        let actual_offset = if offset > file_size { 0 } else { offset };

        let mut reader = BufReader::new(file);
        if reader.seek(SeekFrom::Start(actual_offset)).is_err() {
            return (vec![], actual_offset);
        }

        let mut rewards = vec![];
        let mut new_offset = actual_offset;

        for line in reader.lines() {
            match line {
                Ok(l) => {
                    new_offset += l.len() as u64 + 1;
                    if let Some(reward) = self.parse_line(&l) {
                        rewards.push(reward);
                    }
                }
                Err(_) => break,
            }
        }

        (rewards, new_offset)
    }
}

/// Where detection says EE.log lives, whether or not Warframe has written it
/// yet. The player's `eeLogPath` override is not consulted; anything that
/// actually watches the file must go through [`watched_log_path`] instead.
pub fn default_log_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|data_dir| {
        #[cfg(target_os = "linux")]
        {
            linux_log_path(&data_dir)
        }
        #[cfg(not(target_os = "linux"))]
        {
            data_dir.join("Warframe").join("EE.log")
        }
    })
}

/// Resolved once at startup so the log watcher and the monitor always tail
/// the same file; a changed `eeLogPath` override therefore needs a restart,
/// which the settings UI says. Lives here rather than in `AppState` to keep
/// the fork's footprint out of upstream's struct.
static WATCHED_LOG_PATH: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

pub fn init_watched_log_path(settings_path: &Path) {
    let resolved = resolve_log_path(settings_path);
    match &resolved {
        Some(path) => eprintln!("EE.log resolved to {}", path.display()),
        None => eprintln!("warning: no EE.log path could be resolved"),
    }
    let _ = WATCHED_LOG_PATH.set(resolved);
}

/// True for the line Warframe writes once it has ingested an inventory refresh:
///
/// ```text
/// 1234.567 Sys [Info]: OnInventoryResults completed in 339ms
/// ```
///
/// The earlier `OnInventoryResults, body size=1335235` line is written on
/// receipt, tens to hundreds of milliseconds before the client finishes
/// applying the response, so gating a memory read on it risks reading a
/// half-written buffer. This matches the completion line only.
pub fn is_inventory_sync_line(line: &str) -> bool {
    line.contains("OnInventoryResults completed in")
}

// A mismatch here degrades to plain interval polling, which is hard to tell
// from working correctly, so the marker is pinned against verbatim lines.
#[cfg(test)]
mod inventory_sync_tests {
    use super::is_inventory_sync_line;

    #[test]
    fn matches_the_completion_line_only() {
        assert!(is_inventory_sync_line(
            "19761.848 Sys [Info]: OnInventoryResults completed in 339ms"
        ));
        assert!(!is_inventory_sync_line(
            "19761.509 Sys [Info]: OnInventoryResults, body size=1335235"
        ));
        assert!(!is_inventory_sync_line("19760.121 Sys [Info]: SyncInventoryFromDB"));
    }
}

pub fn watched_log_path() -> Option<PathBuf> {
    WATCHED_LOG_PATH
        .get()
        .cloned()
        .unwrap_or_else(default_log_path)
}

pub fn resolve_log_path(settings_path: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        resolve_with_override(
            log_path_override(settings_path).as_deref(),
            dirs::data_local_dir(),
        )
    }
    // Windows and macOS put the log in one fixed place, so there is nothing
    // for an override to rescue.
    #[cfg(not(target_os = "linux"))]
    {
        let _ = settings_path;
        default_log_path()
    }
}

#[cfg(target_os = "linux")]
fn log_path_override(settings_path: &Path) -> Option<String> {
    // A missing settings file just means first launch; a file that exists but
    // does not parse means the player's override is about to be ignored, which
    // is worth a trace.
    let json = std::fs::read_to_string(settings_path).ok()?;
    let settings: serde_json::Value = match serde_json::from_str(&json) {
        Ok(value) => value,
        Err(e) => {
            eprintln!(
                "warning: {} is not valid JSON ({e}); ignoring any eeLogPath override",
                settings_path.display()
            );
            return None;
        }
    };

    settings
        .get("eeLogPath")?
        .as_str()
        .map(|path| path.to_string())
}

#[derive(serde::Serialize)]
pub struct EeLogStatus {
    detected: Option<String>,
    exists: bool,
}

/// Checks the path the player is looking at rather than the saved one, so a
/// typo shows up before they restart on it.
///
/// `async` so the detection sweep's file I/O runs off the main thread.
/// `is_file` rather than `exists`: a directory at the path would pass
/// `exists()` while the watcher fails on it forever.
#[tauri::command]
pub async fn get_ee_log_status(path: Option<String>) -> EeLogStatus {
    match effective_override(path.as_deref()) {
        // With an override set, the detected path is hidden behind the typed
        // text anyway, so the (filesystem-sweeping) detection can be skipped.
        Some(entered) => EeLogStatus {
            detected: None,
            exists: entered.is_file(),
        },
        None => {
            let detected = default_log_path();
            EeLogStatus {
                exists: detected.as_deref().is_some_and(|p| p.is_file()),
                detected: detected.map(|p| p.to_string_lossy().to_string()),
            }
        }
    }
}

/// The one definition of what counts as an override: the startup resolver and
/// the settings-UI status check must agree on it, or the UI validates a
/// different path than the watcher tails.
pub fn effective_override(entered: Option<&str>) -> Option<PathBuf> {
    entered
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(normalize_override)
}

/// The path as the player typed it, turned into the file to watch: `~` becomes
/// the home directory, and a path naming the prefix's `Warframe` *directory*
/// (the likeliest paste) gets `EE.log` appended.
pub fn normalize_override(entered: &str) -> PathBuf {
    normalize_override_in(entered, dirs::home_dir())
}

fn normalize_override_in(entered: &str, home: Option<PathBuf>) -> PathBuf {
    let expanded = match entered.strip_prefix("~") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => match home {
            Some(home) => home.join(rest.trim_start_matches('/')),
            None => PathBuf::from(entered),
        },
        _ => PathBuf::from(entered),
    };

    if expanded.is_dir() {
        expanded.join("EE.log")
    } else {
        expanded
    }
}

/// An override must win even when there is no data directory to search, so a
/// missing `data_dir` only matters on the fallback path.
#[cfg(target_os = "linux")]
fn resolve_with_override(log_path: Option<&str>, data_dir: Option<PathBuf>) -> Option<PathBuf> {
    match effective_override(log_path) {
        // Deliberately not checked for existence: falling back to a guessed
        // path would leave the player's setting looking ignored.
        Some(path) => Some(path),
        None => Some(linux_log_path(&data_dir?)),
    }
}

// ==============================================================================
// Linux: locating EE.log across Steam libraries and Wine prefixes
// ==============================================================================
//
// On Linux, Warframe runs under Proton, which keeps its Windows prefix beside
// the game rather than in one fixed place. Nothing about the location is fixed:
//
//   * The prefix lives in whichever Steam *library folder* holds the game, so
//     an install on a second drive is nowhere near the Steam install. Steam
//     indexes its libraries in `libraryfolders.vdf`, which we read rather than
//     guessing at mount points.
//   * Steam itself sits in one of three places, depending on how it was
//     packaged — native, the legacy `~/.steam` layout, or Flatpak. Each has its
//     own library index, so all three are read; a Flatpak Steam with games on a
//     second drive needs its own vdf just as the native one does.
//   * The username *inside* the prefix is `steamuser` under Proton, but an
//     older Proton or a hand-made prefix uses the Linux login name.
//
// The result is a list of candidate paths rather than one computed path, which
// is why this is a search and not a `join`.

/// Warframe's Steam application ID, which names its Proton prefix directory.
#[cfg(target_os = "linux")]
const WARFRAME_APPID: &str = "230410";

/// The username Proton creates inside a prefix, regardless of the Linux login.
#[cfg(target_os = "linux")]
const PROTON_WIN_USER: &str = "steamuser";

/// The Windows-side walk from a prefix's `drive_c` down to the log.
#[cfg(target_os = "linux")]
fn log_within_drive_c(drive_c: &Path, win_user: &str) -> PathBuf {
    drive_c
        .join("users")
        .join(win_user)
        .join("AppData/Local/Warframe/EE.log")
}

/// Warframe's Proton prefix inside a given Steam library root.
///
/// Steam creates this at install time, before the game has ever run.
#[cfg(target_os = "linux")]
fn prefix_within(library: &Path) -> PathBuf {
    library.join("steamapps/compatdata").join(WARFRAME_APPID)
}

/// Where EE.log sits inside a given Steam library root, for one prefix user.
#[cfg(target_os = "linux")]
fn ee_log_within(library: &Path, win_user: &str) -> PathBuf {
    log_within_drive_c(&prefix_within(library).join("pfx/drive_c"), win_user)
}

/// One place EE.log could be, and how to recognise its install without it.
#[cfg(target_os = "linux")]
struct Candidate {
    log: PathBuf,
    /// The Proton prefix this log belongs to, when the candidate is a Steam
    /// install. Steam creates the prefix at install time, so its presence
    /// identifies the right library before the game has written any log at
    /// all. A plain Wine prefix has no equivalent marker, hence `None`.
    prefix: Option<PathBuf>,
}

/// Steam installations to search, most conventional first.
///
/// `~/.steam/steam` is usually a symlink into the native path and Flatpak's is
/// genuinely separate; probing all three costs a few `stat` calls and spares us
/// having to tell which packaging is in use.
#[cfg(target_os = "linux")]
fn steam_roots(data_dir: &Path, home: &Path) -> Vec<PathBuf> {
    vec![
        data_dir.join("Steam"),
        home.join(".steam/steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ]
}

/// Every library folder belonging to one Steam install: the root itself, plus
/// whatever its own index lists.
///
/// A missing or unreadable index is not an error — it just means this Steam
/// root has no extra libraries, or is not the packaging in use on this machine.
#[cfg(target_os = "linux")]
fn libraries_under(root: &Path) -> Vec<PathBuf> {
    let mut libraries = vec![root.to_path_buf()];

    if let Ok(vdf) = std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
        // Steam lists the root among its own libraries, so this usually
        // repeats the entry above; the caller de-duplicates.
        libraries.extend(library_paths(&vdf));
    }

    libraries
}

/// Lutris names a prefix after the game as the player typed it, so no fixed
/// name can be trusted; instead, every prefix under `~/Games` is a candidate.
/// The listing is sorted so the pick is stable when several prefixes hold a
/// log.
#[cfg(target_os = "linux")]
fn wine_prefix_roots(home: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = std::fs::read_dir(home.join("Games"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("drive_c").is_dir())
        .collect();
    roots.sort();

    roots.push(home.join(".wine"));
    roots
}

/// Every path EE.log could occupy on this machine, best-guess first.
#[cfg(target_os = "linux")]
fn ee_log_candidates(data_dir: &Path, home: &Path, win_users: &[String]) -> Vec<Candidate> {
    let mut candidates = Vec::new();

    for root in steam_roots(data_dir, home) {
        for library in libraries_under(&root) {
            for win_user in win_users {
                candidates.push(Candidate {
                    log: ee_log_within(&library, win_user),
                    prefix: Some(prefix_within(&library)),
                });
            }
        }
    }

    // Wine prefixes for an install Steam knows nothing about. None of them has
    // a compatdata directory to recognise the install by, so they can only be
    // found by the log already being there.
    for prefix_root in wine_prefix_roots(home) {
        for win_user in win_users {
            candidates.push(Candidate {
                log: log_within_drive_c(&prefix_root.join("drive_c"), win_user),
                prefix: None,
            });
        }
    }

    // Roots overlap in the common case — the native root is listed in its own
    // vdf, and `~/.steam/steam` usually points at it — so the same log path can
    // be reached several ways. Keep the first occurrence of each.
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.log.clone()));

    candidates
}

/// The best candidate that something on disk actually corroborates.
#[cfg(target_os = "linux")]
fn resolve_candidate(candidates: &[Candidate]) -> Option<PathBuf> {
    candidates
        .iter()
        // An existing EE.log is the strongest signal: that is a prefix
        // Warframe actually ran from.
        .filter(|candidate| candidate.log.exists())
        // More than one install can have run at some point, and the enumeration
        // order is by install layout, which says nothing about which client the
        // player uses now. Warframe rewrites its log on every launch, so the
        // newest one belongs to the session in progress; an abandoned prefix
        // keeps whatever timestamp it was left with. Reverse rather than `max`
        // so an exact tie keeps enumeration order.
        .min_by_key(|candidate| {
            std::cmp::Reverse(
                candidate
                    .log
                    .metadata()
                    .and_then(|data| data.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            )
        })
        .or_else(|| {
            // No log yet, which is the normal state before the game's first
            // run. An existing prefix still says which install will hold it.
            candidates.iter().find(|candidate| {
                candidate
                    .prefix
                    .as_deref()
                    .is_some_and(|prefix| prefix.is_dir())
            })
        })
        .map(|candidate| candidate.log.clone())
}

/// Windows usernames to try inside a prefix, in order of likelihood.
///
/// Takes the Linux login name rather than reading the environment itself, so
/// the "which names are worth trying" rule can be tested without the
/// process-wide environment mutation that would race other tests.
#[cfg(target_os = "linux")]
fn windows_users(login: Option<&str>) -> Vec<String> {
    let mut users = vec![PROTON_WIN_USER.to_string()];

    // An empty login is what a stripped environment yields; pushing it would
    // build a path with an empty component that can never match anything.
    if let Some(login) = login.filter(|user| !user.is_empty() && *user != PROTON_WIN_USER) {
        users.push(login.to_string());
    }

    users
}

/// The library roots listed in a `libraryfolders.vdf`, in file order.
///
/// Deliberately not a real VDF parser. The only field we need is the `"path"`
/// of each entry, and every way the file can be malformed degrades to "no
/// matches" — which the caller already handles by falling back to the default
/// library. A parser would add a dependency to gain nothing but stricter
/// failure.
///
/// TODO: VDF escapes backslashes in Windows-style paths (`\\`). Library paths
/// written by Steam on Linux are POSIX and contain none, so unescaping is
/// omitted.
#[cfg(target_os = "linux")]
fn library_paths(vdf: &str) -> Vec<PathBuf> {
    let entry = match Regex::new(r#""path"\s*"([^"]+)""#) {
        Ok(regex) => regex,
        Err(_) => return vec![],
    };

    entry
        .captures_iter(vdf)
        .filter_map(|caps| caps.get(1))
        .map(|path| PathBuf::from(path.as_str()))
        .collect()
}

/// The search proper, with its two environment inputs passed in so tests can
/// point it at a fake layout instead of the machine it runs on.
#[cfg(target_os = "linux")]
fn linux_log_path_in(data_dir: &Path, home: &Path, win_users: &[String]) -> PathBuf {
    resolve_candidate(&ee_log_candidates(data_dir, home, win_users))
        // Nothing on disk corroborates any candidate: Warframe is not
        // installed, or lives somewhere none of the above covers. The watchers
        // start before the game does and need a path regardless, so name where
        // a stock Proton install would put it.
        .unwrap_or_else(|| ee_log_within(&data_dir.join("Steam"), PROTON_WIN_USER))
}

#[cfg(target_os = "linux")]
fn linux_log_path(data_dir: &Path) -> PathBuf {
    // Without a home directory the Flatpak, legacy and Wine candidates are all
    // unreachable; `data_dir` still yields the native Steam root, which is the
    // one that matters.
    let home = dirs::home_dir().unwrap_or_else(|| data_dir.to_path_buf());

    let login = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok();

    linux_log_path_in(data_dir, &home, &windows_users(login.as_deref()))
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;
    use std::fs;

    /// A throwaway directory for one test's fake Steam layout; the drop guard
    /// means a failing assertion cannot leak it.
    ///
    /// Cheaper than a `tempfile` dependency for the handful of tests here, and
    /// the name is unique per test because each call site passes its own tag.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    impl std::ops::Deref for Scratch {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    fn scratch_dir(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("ff-eelog-{}-{}", std::process::id(), tag));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir is writable in a test environment");
        Scratch(dir)
    }

    /// Write a `libraryfolders.vdf` listing `libraries`, in Steam's own format.
    fn write_vdf(steam_root: &Path, libraries: &[&Path]) {
        let mut vdf = String::from("\"libraryfolders\"\n{\n");
        for (index, library) in libraries.iter().enumerate() {
            vdf.push_str(&format!(
                "\t\"{}\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t\t\"label\"\t\t\"\"\n\t}}\n",
                index,
                library.display()
            ));
        }
        vdf.push_str("}\n");

        let steamapps = steam_root.join("steamapps");
        fs::create_dir_all(&steamapps).expect("scratch dir was just created");
        fs::write(steamapps.join("libraryfolders.vdf"), vdf).expect("scratch dir is writable");
    }

    /// Create an empty file at `log`, along with the directories above it.
    fn plant(log: PathBuf) -> PathBuf {
        fs::create_dir_all(log.parent().expect("EE.log always has a parent directory"))
            .expect("scratch dir is writable");
        fs::write(&log, "").expect("scratch dir is writable");
        log
    }

    /// Backdate a planted log. Both files are written moments apart otherwise,
    /// and which install ran most recently is what resolution has to decide.
    fn age(log: &Path, seconds: u64) {
        let file = fs::File::options().write(true).open(log).expect("planted log is writable");
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(seconds);
        file.set_modified(when).expect("scratch filesystem records mtimes");
    }

    /// Create an empty EE.log in `library`, as a stock Proton install would.
    fn plant_ee_log(library: &Path) -> PathBuf {
        plant(ee_log_within(library, PROTON_WIN_USER))
    }

    /// The usernames a stock Proton machine would search.
    fn stock_users() -> Vec<String> {
        vec![PROTON_WIN_USER.to_string()]
    }

    /// Run the search against a fake layout, with no home-directory sources.
    ///
    /// Tests that care about `~/.steam`, Flatpak or Wine pass their own home.
    fn search(data_dir: &Path) -> PathBuf {
        linux_log_path_in(data_dir, Path::new("/nonexistent-home"), &stock_users())
    }

    #[test]
    fn builds_proton_log_path_from_linux_data_dir() {
        let path = search(Path::new("/home/player/.local/share"));
        assert_eq!(
            path,
            Path::new("/home/player/.local/share/Steam/steamapps/compatdata/230410/pfx/drive_c/users/steamuser/AppData/Local/Warframe/EE.log")
        );
    }

    #[test]
    fn reads_every_library_root_listed_in_the_vdf() {
        let vdf = "\"libraryfolders\"\n{\n\
                   \t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/player/.local/share/Steam\"\n\t}\n\
                   \t\"1\"\n\t{\n\t\t\"path\"\t\t\"/mnt/games/SteamLibrary\"\n\t}\n\
                   }\n";

        assert_eq!(
            library_paths(vdf),
            vec![
                PathBuf::from("/home/player/.local/share/Steam"),
                PathBuf::from("/mnt/games/SteamLibrary"),
            ]
        );
    }

    #[test]
    fn a_malformed_vdf_yields_no_libraries_rather_than_an_error() {
        // Truncated mid-entry, which is what a vdf looks like if Steam was
        // killed while rewriting it.
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"pa";

        assert!(library_paths(vdf).is_empty());
    }

    #[test]
    fn finds_ee_log_in_a_secondary_library() {
        let scratch = scratch_dir("secondary");
        let data_dir = scratch.join("local-share");
        let secondary = scratch.join("mnt-games-SteamLibrary");

        write_vdf(
            &data_dir.join("Steam"),
            &[&data_dir.join("Steam"), &secondary],
        );
        let planted = plant_ee_log(&secondary);

        assert_eq!(search(&data_dir), planted);
    }

    #[test]
    fn points_at_the_library_holding_the_prefix_before_the_game_has_run() {
        let scratch = scratch_dir("prefix-only");
        let data_dir = scratch.join("local-share");
        let secondary = scratch.join("mnt-games-SteamLibrary");

        write_vdf(
            &data_dir.join("Steam"),
            &[&data_dir.join("Steam"), &secondary],
        );

        // Warframe is installed to the secondary library — Steam created the
        // Proton prefix — but it has never been launched, so there is no
        // EE.log to find yet.
        fs::create_dir_all(prefix_within(&secondary).join("pfx")).expect("scratch dir is writable");

        assert_eq!(
            search(&data_dir),
            ee_log_within(&secondary, PROTON_WIN_USER)
        );
    }

    #[test]
    fn falls_back_to_the_default_library_when_the_appid_is_in_no_library() {
        let scratch = scratch_dir("missing-appid");
        let data_dir = scratch.join("local-share");
        let secondary = scratch.join("mnt-games-SteamLibrary");

        // Both libraries exist and are indexed, but neither holds Warframe.
        write_vdf(
            &data_dir.join("Steam"),
            &[&data_dir.join("Steam"), &secondary],
        );

        // The watchers start before the game does, so we still owe them the
        // path EE.log *would* occupy in the default library.
        assert_eq!(
            search(&data_dir),
            ee_log_within(&data_dir.join("Steam"), PROTON_WIN_USER)
        );
    }

    #[test]
    fn finds_ee_log_under_flatpak_steam() {
        let scratch = scratch_dir("flatpak");
        let data_dir = scratch.join("local-share");
        let home = scratch.join("home");

        // Nothing is installed natively; Steam came from Flathub.
        let flatpak = home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
        let planted = plant_ee_log(&flatpak);

        assert_eq!(linux_log_path_in(&data_dir, &home, &stock_users()), planted);
    }

    #[test]
    fn finds_ee_log_under_the_legacy_steam_root() {
        let scratch = scratch_dir("legacy-root");
        let data_dir = scratch.join("local-share");
        let home = scratch.join("home");

        let planted = plant_ee_log(&home.join(".steam/steam"));

        assert_eq!(linux_log_path_in(&data_dir, &home, &stock_users()), planted);
    }

    #[test]
    fn reads_the_library_index_of_each_steam_root_not_only_the_native_one() {
        let scratch = scratch_dir("flatpak-secondary");
        let data_dir = scratch.join("local-share");
        let home = scratch.join("home");
        let secondary = scratch.join("mnt-games-SteamLibrary");

        // Flatpak Steam, with the game itself on a second drive: the only
        // index naming that drive belongs to the Flatpak install, so a search
        // that reads just the native root's vdf would never see it.
        let flatpak = home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
        write_vdf(&flatpak, &[&flatpak, &secondary]);
        let planted = plant_ee_log(&secondary);

        assert_eq!(linux_log_path_in(&data_dir, &home, &stock_users()), planted);
    }

    #[test]
    fn finds_ee_log_in_a_plain_wine_prefix() {
        let scratch = scratch_dir("wine");
        let data_dir = scratch.join("local-share");
        let home = scratch.join("home");

        // A non-Steam install, so the prefix carries the Linux login name and
        // there is no compatdata directory anywhere.
        let users = vec![PROTON_WIN_USER.to_string(), "player".to_string()];
        let planted = plant(log_within_drive_c(&home.join(".wine/drive_c"), "player"));

        assert_eq!(linux_log_path_in(&data_dir, &home, &users), planted);
    }

    #[test]
    fn finds_ee_log_when_the_prefix_uses_the_linux_login_name() {
        let scratch = scratch_dir("login-name");
        let data_dir = scratch.join("local-share");
        let home = scratch.join("home");

        // An older Proton, or a prefix made by hand: same Steam layout, but
        // the user directory inside it is not `steamuser`.
        let users = vec![PROTON_WIN_USER.to_string(), "player".to_string()];
        let planted = plant(ee_log_within(&data_dir.join("Steam"), "player"));

        assert_eq!(linux_log_path_in(&data_dir, &home, &users), planted);
    }

    #[test]
    fn tries_the_linux_login_name_only_when_it_adds_something() {
        assert_eq!(windows_users(Some("player")), vec!["steamuser", "player"]);

        // Nothing to add: no login, an empty one, or one that already matches
        // what Proton would have created.
        assert_eq!(windows_users(None), vec!["steamuser"]);
        assert_eq!(windows_users(Some("")), vec!["steamuser"]);
        assert_eq!(windows_users(Some("steamuser")), vec!["steamuser"]);
    }

    #[test]
    fn an_override_wins_whatever_the_search_would_say() {
        for data_dir in [Some(PathBuf::from("/nonexistent")), None] {
            assert_eq!(
                resolve_with_override(Some("/mnt/games/wf/EE.log"), data_dir),
                Some(PathBuf::from("/mnt/games/wf/EE.log"))
            );
        }
    }

    #[test]
    fn no_override_falls_through_to_the_search() {
        let scratch = scratch_dir("override-absent");
        let data_dir = scratch.join("local-share");
        let planted = plant_ee_log(&data_dir.join("Steam"));

        for unset in [None, Some(""), Some("   ")] {
            assert_eq!(
                resolve_with_override(unset, Some(data_dir.clone())),
                Some(planted.clone()),
                "{unset:?} should not count as an override"
            );
        }
    }

    #[test]
    fn reads_the_override_from_the_settings_file() {
        let scratch = scratch_dir("settings");
        let settings = scratch.join("settings.json");
        fs::write(&settings, r#"{"overlayEnabled":true,"eeLogPath":"/mnt/wf/EE.log"}"#)
            .expect("scratch dir is writable");

        assert_eq!(
            log_path_override(&settings),
            Some("/mnt/wf/EE.log".to_string())
        );

        fs::write(&settings, r#"{"overlayEnabled":true}"#).expect("scratch dir is writable");
        assert_eq!(log_path_override(&settings), None);
        assert_eq!(log_path_override(&scratch.join("missing.json")), None);
    }

    #[test]
    fn a_non_string_override_is_no_override() {
        let scratch = scratch_dir("settings-nonstring");
        let settings = scratch.join("settings.json");

        for json in [r#"{"eeLogPath":null}"#, r#"{"eeLogPath":5}"#] {
            fs::write(&settings, json).expect("scratch dir is writable");
            assert_eq!(log_path_override(&settings), None, "{json}");
        }
    }

    #[test]
    fn resolve_log_path_returns_the_override_from_the_settings_file() {
        let scratch = scratch_dir("resolve-end-to-end");
        let settings = scratch.join("settings.json");
        fs::write(&settings, r#"{"eeLogPath":"/mnt/wf/EE.log"}"#)
            .expect("scratch dir is writable");

        assert_eq!(
            resolve_log_path(&settings),
            Some(PathBuf::from("/mnt/wf/EE.log"))
        );
    }

    #[test]
    fn a_tilde_in_the_override_means_the_home_directory() {
        assert_eq!(
            normalize_override_in("~/wf/EE.log", Some(PathBuf::from("/home/player"))),
            PathBuf::from("/home/player/wf/EE.log")
        );

        // No home to expand against: better the literal path (and its "no file
        // there" warning) than a silently different one.
        assert_eq!(
            normalize_override_in("~/wf/EE.log", None),
            PathBuf::from("~/wf/EE.log")
        );

        assert_eq!(
            normalize_override_in("/mnt/~backup/EE.log", Some(PathBuf::from("/home/player"))),
            PathBuf::from("/mnt/~backup/EE.log")
        );
    }

    #[test]
    fn an_override_naming_a_directory_means_the_log_inside_it() {
        let scratch = scratch_dir("dir-override");
        let dir = scratch.join("Warframe");
        fs::create_dir_all(&dir).expect("scratch dir is writable");

        assert_eq!(
            normalize_override_in(dir.to_str().expect("scratch paths are UTF-8"), None),
            dir.join("EE.log")
        );
    }

    #[test]
    fn finds_ee_log_in_a_lutris_prefix_however_the_player_named_it() {
        let scratch = scratch_dir("lutris-renamed");
        let data_dir = scratch.join("local-share");
        let home = scratch.join("home");

        let planted = plant(log_within_drive_c(
            &home.join("Games/wf (proton-ge)/drive_c"),
            PROTON_WIN_USER,
        ));

        assert_eq!(linux_log_path_in(&data_dir, &home, &stock_users()), planted);
    }

    #[test]
    fn finds_a_lutris_prefix_named_after_the_linux_login() {
        let scratch = scratch_dir("lutris-login");
        let data_dir = scratch.join("local-share");
        let home = scratch.join("home");

        let users = vec![PROTON_WIN_USER.to_string(), "player".to_string()];
        let planted = plant(log_within_drive_c(
            &home.join("Games/warframe/drive_c"),
            "player",
        ));

        assert_eq!(linux_log_path_in(&data_dir, &home, &users), planted);
    }

    /// Warframe rewrites its log on every launch, so with two installs that
    /// have both run, the newest log belongs to the current session.
    #[test]
    fn prefers_a_steam_install_over_a_leftover_lutris_prefix() {
        let scratch = scratch_dir("lutris-vs-steam");
        let data_dir = scratch.join("local-share");
        let home = scratch.join("home");

        let lutris = plant(log_within_drive_c(
            &home.join("Games/warframe/drive_c"),
            PROTON_WIN_USER,
        ));
        let steam = plant_ee_log(&data_dir.join("Steam"));
        age(&lutris, 3600);

        assert_eq!(linux_log_path_in(&data_dir, &home, &stock_users()), steam);
    }

    /// The reported case: both prefixes have run, Steam is enumerated first,
    /// but the player now launches through Lutris. Picking by enumeration order
    /// keeps reading the stale log.
    #[test]
    fn prefers_a_live_lutris_prefix_over_a_stale_steam_install() {
        let scratch = scratch_dir("lutris-live");
        let data_dir = scratch.join("local-share");
        let home = scratch.join("home");

        let steam = plant_ee_log(&data_dir.join("Steam"));
        let lutris = plant(log_within_drive_c(
            &home.join("Games/warframe/drive_c"),
            PROTON_WIN_USER,
        ));
        age(&steam, 3600);

        assert_eq!(linux_log_path_in(&data_dir, &home, &stock_users()), lutris);
    }

    #[test]
    fn offers_each_candidate_path_only_once() {
        let scratch = scratch_dir("dedup");
        let data_dir = scratch.join("local-share");
        let home = scratch.join("home");

        // Steam names its own root in its index, which is the usual case and
        // would otherwise yield the same candidate twice.
        let native = data_dir.join("Steam");
        write_vdf(&native, &[&native]);

        let candidates = ee_log_candidates(&data_dir, &home, &stock_users());
        let mut logs: Vec<_> = candidates.iter().map(|c| c.log.clone()).collect();
        let before = logs.len();
        logs.sort();
        logs.dedup();

        assert_eq!(logs.len(), before, "candidate list repeats a path");
    }
}
