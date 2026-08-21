use memchr::memmem;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use tracing::debug;
use tracing::{info, warn};

/// Every name below is the Linux arm of a function whose Windows arm is
/// defined in this file. A caller reaches the same name either way.
#[cfg(target_os = "linux")]
pub use crate::memory_scanner_linux::{
    capture_all_blobs, dump_inventory_regions, find_riven_validity_va, find_warframe_pid_pub,
    probe_tick, raw_scan_pass, read_process_byte, scan_api_url_strings,
    scan_warframe_credentials_process,
};

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ModCount {
    /// Total copies owned (all ranks combined)
    pub total: i64,
    /// rank (0 = unranked) → count at that rank
    pub by_rank: HashMap<u8, i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BlobRivenStat {
    pub tag:   String,
    pub value: i64,
}

/// Which stage of unlocking a riven is at. Matches warframe.market terminology.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RivenState {
    /// From RawUpgrades — only the weapon type (Rifle/Pistol/Melee…) is visible.
    Unrevealed,
    /// From Upgrades with a `challenge` fingerprint but no `compat` — challenge is visible
    /// but not yet completed; weapon has not been assigned.
    Revealed,
    /// From Upgrades with a `compat` — weapon assigned, stats fully visible.
    #[default]
    Unlocked,
}

/// One owned riven mod (unrevealed, revealed, or fully unlocked).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobRivenEntry {
    /// MongoDB ObjectId hex string (empty for unrevealed stacks).
    pub item_id:  String,
    /// Lotus path, e.g. /Lotus/Upgrades/Mods/Randomized/LotusMeleeRandomModRare
    pub item_type: String,
    /// Which stage this riven is at (unrevealed / revealed / unlocked).
    /// Old cache entries without this field default to Unlocked.
    #[serde(default)]
    pub riven_state: RivenState,
    /// Weapon unique_name from `compat` field. Only present for Unlocked rivens.
    pub compat:   Option<String>,
    /// Challenge path from fingerprint. Only present for Revealed rivens.
    /// e.g. "/Lotus/Types/Challenges/HighExterminationUndetected"
    #[serde(default)]
    pub challenge_type: Option<String>,
    /// Complication path. e.g. "/Lotus/Types/Challenges/Complications/SoloPlayer"
    #[serde(default)]
    pub challenge_complication: Option<String>,
    /// MR required to equip.
    pub lvl_req:  Option<u32>,
    /// Polarity slot (AP_ATTACK, AP_DEFENSE, etc.).
    pub polarity: Option<String>,
    pub buffs:    Vec<BlobRivenStat>,
    pub curses:   Vec<BlobRivenStat>,
    /// Current mod level (rank).
    pub mod_rank: u8,
    /// >1 for stacked unrevealed rivens of the same type.
    pub count:    u32,
    /// Number of times this riven has been re-rolled (Kuva spent). 0 = never rolled.
    #[serde(default)]
    pub rerolls:  u32,
    /// Generated riven mod name (e.g. "cronitron"). Computed from buffs at parse time.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mod_name: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct PendingRecipe {
    pub unique_name: String,
    /// Unix timestamp in milliseconds when the craft completes
    pub completion_ms: i64,
}

/// One Archon Shard socketed into a Warframe.
/// `upgrade_type` is the effect path (e.g. `.../ArchonCrystalUpgradeWarframeEnergyMax`).
/// `color` is the raw string value from the JSON (e.g. `"ACC_CRIMSON"`, `"ACC_AZURE_TAUFORGED"`).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ArchonShard {
    pub upgrade_type: String,
    pub color: String,
}

// ─── Blob inventory types ─────────────────────────────────────────────────────

/// Parsed representation of an Actual_inventory_FULL_ACCOUNT blob.
/// Single authoritative source for all inventory data.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BlobInventory {
    pub credits:         i64,
    pub endo:            i64,
    pub platinum:        i64,
    pub free_platinum:   i64,
    pub mastery_level:   u32,
    pub unique_items:    Vec<BlobUniqueEntry>,
    pub stackable_items: Vec<BlobStackableEntry>,
    /// Aggregated from RawUpgrades (unranked) + Upgrades (ranked).
    pub mods:            HashMap<String, ModCount>,
    /// FlavourItems — glyphs, palettes, emotes, titles, ship skins. Path → occurrence count.
    pub flavour_items:   HashMap<String, i64>,
    /// WeaponSkins — sigils and cosmetic weapon overlays. Path → occurrence count.
    pub weapon_skins:    HashMap<String, i64>,
    /// Path → mastery rank derived from XPInfo.
    pub mastery_data:    HashMap<String, u32>,
    pub pending_recipes: Vec<BlobPendingRecipe>,
    /// Warframe paths fed to Helminth (InfestedFoundry.ConsumedSuits).
    pub consumed_suits:  Vec<String>,
    /// All owned riven mods (veiled and revealed).
    pub rivens:          Vec<BlobRivenEntry>,
}

/// One owned unique item (warframe, weapon, companion, archwing, amp, mech).
/// Multiple entries with the same item_type = multiple owned copies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobUniqueEntry {
    pub item_type:     String,
    pub section:       String,
    pub polarized:     u32,
    /// Raw XP from the blob — used to compute rank via `xp_to_rank`.
    /// For gilded modular items (Amps, Kitguns, Zaws) XP resets to 0 on gilding,
    /// so this reflects post-gild progress.
    pub xp:            i64,
    /// Player-assigned name (set when an item is gilded in the Foundry).
    pub item_name:     Option<String>,
    pub pet_name:      Option<String>,
    pub focus_lens:    Option<String>,
    pub archon_shards: Vec<ArchonShard>,
    /// Component paths for modular items (Amps, Kitguns, Zaws).
    /// Populated from the blob's `ModularParts` array.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modular_parts: Vec<String>,
}

/// A stackable item: resource, blueprint, relic, Ayatan sculpture, etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobStackableEntry {
    pub item_type:  String,
    pub item_count: i64,
    /// Ayatan sockets (FusionTreasures only).
    pub sockets:    Option<i64>,
}

/// Active Foundry crafting job parsed from PendingRecipes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobPendingRecipe {
    pub item_type:     String,
    pub completion_ms: i64,
}

fn digits_end(data: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < data.len() && data[i].is_ascii_digit() { i += 1; }
    i
}

/// Convert raw affinity XP to item rank.
/// Formula from Warframe wiki: cumulative XP to reach rank N is 1000×N² for
/// Warframes/Sentinels/companions, 500×N² for all weapon types.
/// Invert: rank = floor(sqrt(xp / base)).
/// No upper cap — some weapons (e.g. Paracesis) can exceed rank 30.
pub fn xp_to_rank(xp: i64, path: &str) -> u32 {
    let base = if path.contains("/Powersuits/")
        || path.contains("/SentinelPowersuits/")
        || path.contains("/Types/Friendly/")
        || path.contains("/Types/Game/KubrowPet/")
        || path.contains("/Types/Game/CatbrowPet/")
    { 1000.0f64 } else { 500.0f64 };
    (xp as f64 / base).sqrt().floor() as u32
}

// ─── Auth credentials scan ───────────────────────────────────────────────────
//
// When Warframe is running and logged in, the game stores the session credentials
// in memory as URL-encoded strings: accountId=<id>&nonce=<nonce>
// We scan for these to authenticate with the Warframe companion API.

pub fn scan_auth_credentials(data: &[u8]) -> Option<(String, String)> {
    // The Warframe game receives a login response JSON from DE's servers containing:
    //   {"id":"<24-char-hex-accountId>","Nonce":<large-integer>,...}
    // We search for this pattern. The Nonce is typically 9-13 digits.
    // We also try URL-encoded form: accountId=<id>&nonce=<nonce>
    //
    // Key insight from devtools: accountId=594144e63ade7f2f2091c48e (24ch), nonce len=9
    // The 24-char hex accountId is a MongoDB ObjectId — correct format.
    // The 9-digit nonce IS valid — it's a server-issued integer session token.

    // Search for "id":"<24hexchars>" near "Nonce":<digits>
    let id_key = b"\"id\":\"";
    let nonce_key = b"\"Nonce\":";
    for next in memmem::find_iter(data, id_key) {
        let id_start = next + id_key.len();
        // accountId is exactly 24 lowercase hex chars
        let id_slice = &data[id_start..id_start.saturating_add(26).min(data.len())];
        let close = id_slice.iter().position(|&b| b == b'"').unwrap_or(0);
        if close != 24 { continue; }
        let id_bytes = &id_slice[..24];
        if !id_bytes.iter().all(|&b| b.is_ascii_hexdigit()) { continue; }
        let account_id = std::str::from_utf8(id_bytes).unwrap_or("").to_string();

        let nonce_search_end = (id_start + 2048).min(data.len());
        if let Some(rel) = memmem::find(&data[id_start..nonce_search_end], nonce_key) {
            let ns = id_start + rel + nonce_key.len();
            let ne = digits_end(data, ns);
            if ne > ns && ne - ns >= 5 {
                if let Ok(nonce) = std::str::from_utf8(&data[ns..ne]) {
                    return Some((account_id, nonce.to_string()));
                }
            }
        }
    }

    // URL-encoded: accountId=<24hexchars>&nonce=<10digits>&ct=STM
    let ak = b"accountId=";
    let nk = b"nonce=";
    for next in memmem::find_iter(data, ak) {
        let id_start = next + ak.len();
        let id_end = data[id_start..].iter().position(|&b| !b.is_ascii_hexdigit()).map(|p| id_start + p).unwrap_or(data.len());
        if id_end - id_start != 24 { continue; }
        let account_id = std::str::from_utf8(&data[id_start..id_end]).unwrap_or("").to_string();
        let nonce_search_end = (id_end + 512).min(data.len());
        if let Some(rel) = memmem::find(&data[id_end..nonce_search_end], nk) {
            let ns = id_end + rel + nk.len();
            let ne = digits_end(data, ns);
            if ne > ns && ne - ns >= 5 {
                if let Ok(nonce) = std::str::from_utf8(&data[ns..ne]) {
                    return Some((account_id, nonce.to_string()));
                }
            }
        }
    }
    None
}

/// Also extract steamId from memory (found near accountId/nonce in URL params).
pub fn scan_steam_id(data: &[u8]) -> Option<String> {
    let key = b"steamId=";
    for next in memmem::find_iter(data, key) {
        let id_start = next + key.len();
        let id_end = data[id_start..].iter().position(|&b| !b.is_ascii_digit()).map(|p| id_start + p).unwrap_or(data.len());
        if id_end - id_start >= 15 && id_end - id_start <= 20 {
            if let Ok(sid) = std::str::from_utf8(&data[id_start..id_end]) {
                return Some(sid.to_string());
            }
        }
    }
    None
}

// ─── Public helpers ──────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
pub fn find_warframe_pid_pub() -> Option<u32> { find_warframe_pid() }

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn find_warframe_pid_pub() -> Option<u32> { None }

#[cfg(target_os = "windows")]
#[tracing::instrument(level = "info", skip_all)]
pub fn scan_warframe_credentials_process() -> Result<(String, String, String), String> {
    use std::ffi::c_void;
    use std::mem;
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::{
            Diagnostics::Debug::ReadProcessMemory,
            Memory::{VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_NOACCESS},
            Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ},
        },
    };

    let pid = find_warframe_pid_pub().ok_or("Warframe is not running")?;

    unsafe {
        let process = OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, 0, pid);
        if process == 0 { return Err("Cannot open Warframe process".into()); }

        let mut address: usize = 0x10000;
        let mbi_size = mem::size_of::<MEMORY_BASIC_INFORMATION>();

        loop {
            let mut mbi: MEMORY_BASIC_INFORMATION = mem::zeroed();
            if VirtualQueryEx(process, address as *const c_void, &mut mbi, mbi_size) == 0 { break; }
            let region_end = (mbi.BaseAddress as usize).saturating_add(mbi.RegionSize);
            if region_end <= address { break; }
            address = region_end;

            if mbi.State != MEM_COMMIT { continue; }
            let protection = mbi.Protect;
            if protection & PAGE_NOACCESS != 0 || protection & PAGE_GUARD != 0 { continue; }
            if protection == 0x10 || protection == 0x20 { continue; }
            if mbi.RegionSize > 128 * 1024 * 1024 { continue; }

            let mut buffer = vec![0u8; mbi.RegionSize];
            let mut bytes_read: usize = 0;
            let ok = ReadProcessMemory(
                process,
                mbi.BaseAddress as *const c_void,
                buffer.as_mut_ptr() as *mut c_void,
                mbi.RegionSize,
                &mut bytes_read,
            );
            if ok == 0 || bytes_read == 0 { continue; }

            let data = &buffer[..bytes_read];
            if let Some((id, nonce)) = scan_auth_credentials(data) {
                let steam_id = scan_steam_id(data).unwrap_or_default();
                CloseHandle(process);
                return Ok((id, nonce, steam_id));
            }
        }
        CloseHandle(process);
    }
    Err("Credentials not found in memory. Make sure you are in the orbiter (not loading screen) and Warframe has been running for a few minutes.".into())
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn scan_warframe_credentials_process() -> Result<(String, String, String), String> {
    Err("Only supported on Windows and Linux".into())
}

/// Read a single byte from the game process, used for the riven validity flag.
/// `None` means the process or the address is not readable.
#[cfg(target_os = "windows")]
pub fn read_process_byte(pid: u32, address: usize) -> Option<u8> {
    use std::ffi::c_void;
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::{
            Diagnostics::Debug::ReadProcessMemory,
            Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ},
        },
    };

    unsafe {
        let process = OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, 0, pid);
        if process == 0 {
            return None;
        }
        let mut byte: u8 = 0;
        let mut read = 0usize;
        let ok = ReadProcessMemory(
            process,
            address as *const c_void,
            &mut byte as *mut u8 as *mut c_void,
            1,
            &mut read,
        );
        CloseHandle(process);
        (ok != 0 && read == 1).then_some(byte)
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn read_process_byte(_pid: u32, _address: usize) -> Option<u8> { None }

// ─── Raw memory format probe ──────────────────────────────────────────────────
//
// Scans Warframe's memory and returns raw text context around every occurrence
// of a set of known strings.  Capped at max_hits total.  Used to reverse-engineer
// the actual JSON format for inventory items without any parsing assumptions.

#[cfg(target_os = "windows")]
#[tracing::instrument(level = "info", skip_all, fields(max_hits = max_hits))]
pub fn dump_inventory_regions(max_hits: usize) -> Vec<String> {
    use std::ffi::c_void;
    use std::mem;
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::{
            Diagnostics::Debug::ReadProcessMemory,
            Memory::{VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_NOACCESS},
            Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ},
        },
    };

    // Patterns to search for — ordered by diagnostic value.
    // "MiscItems":[{ marks the beginning of the actual inventory JSON array from DE's API
    // response (the most useful single needle for finding the real JSON blob).
    const NEEDLES: &[&[u8]] = &[
        b"\"MiscItems\":[{",      // inventory JSON array start — best diagnostic
        b"\"ItemCount\":",
        b"MiscItems",
        b"AlloyPlate",
        b"Circuits\"",
        b"/Lotus/Types/Items/MiscItems/",
    ];

    let pid = match find_warframe_pid() {
        Some(p) => p,
        None => return vec!["Warframe not running".to_string()],
    };

    let process = unsafe { OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, 0, pid) };
    if process == 0 { return vec!["OpenProcess failed".to_string()]; }

    let mut results: Vec<String> = Vec::new();
    let mut addr: usize = 0x10000;
    let mbi_size = mem::size_of::<MEMORY_BASIC_INFORMATION>();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);

    'outer: while std::time::Instant::now() < deadline && results.len() < max_hits {
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { mem::zeroed() };
        if unsafe { VirtualQueryEx(process, addr as *const c_void, &mut mbi, mbi_size) } == 0 { break; }
        let region_end = (mbi.BaseAddress as usize).saturating_add(mbi.RegionSize);
        if region_end <= addr { break; }
        addr = region_end;

        if mbi.State != MEM_COMMIT { continue; }
        let p = mbi.Protect;
        if p & PAGE_NOACCESS != 0 || p & PAGE_GUARD != 0 { continue; }
        if p == 0x10 || p == 0x20 { continue; }    // skip executable (code) pages
        // Skip tiny or enormous regions; read large regions in 64 MB chunks
        const MAX_REGION: usize = 256 * 1024 * 1024;
        const CHUNK_SIZE: usize =  64 * 1024 * 1024;
        if mbi.RegionSize < 4096 || mbi.RegionSize > MAX_REGION { continue; }

        let chunks = if mbi.RegionSize > CHUNK_SIZE {
            (mbi.RegionSize + CHUNK_SIZE - 1) / CHUNK_SIZE
        } else { 1 };

        'chunk: for chunk_idx in 0..chunks {
            if results.len() >= max_hits { break 'outer; }
            if std::time::Instant::now() >= deadline { break 'outer; }

            let chunk_offset = chunk_idx * CHUNK_SIZE;
            let read_size    = CHUNK_SIZE.min(mbi.RegionSize - chunk_offset);
            let chunk_addr   = mbi.BaseAddress as usize + chunk_offset;

            let mut buf = vec![0u8; read_size];
            let mut bytes_read = 0usize;
            let ok = unsafe {
                ReadProcessMemory(process, chunk_addr as *const c_void,
                    buf.as_mut_ptr() as *mut c_void, read_size, &mut bytes_read)
            };
            if ok == 0 || bytes_read < 8 { continue 'chunk; }
            let data = &buf[..bytes_read];

        for needle in NEEDLES {
            if results.len() >= max_hits { break 'outer; }
            if let Some(pos) = data.windows(needle.len()).position(|w| w == *needle) {
                let ctx_start = pos.saturating_sub(80);
                let ctx_end   = data.len().min(pos + 200);
                let snip: String = data[ctx_start..ctx_end].iter()
                    .map(|&b| if b >= 0x20 && b < 0x7f { b as char } else { '·' })
                    .collect();
                results.push(format!(
                    "0x{:012x}  needle=\"{}\"  ctx: {}",
                    chunk_addr + ctx_start,
                    String::from_utf8_lossy(needle),
                    snip
                ));
                // Also grab up to 2 more occurrences of the same needle in this chunk
                let mut search = pos + needle.len();
                let mut extra = 0;
                while extra < 2 && search + needle.len() <= data.len() {
                    if let Some(rel) = data[search..].windows(needle.len()).position(|w| w == *needle) {
                        let p2 = search + rel;
                        let s2 = p2.saturating_sub(80);
                        let e2 = data.len().min(p2 + 200);
                        let snip2: String = data[s2..e2].iter()
                            .map(|&b| if b >= 0x20 && b < 0x7f { b as char } else { '·' })
                            .collect();
                        results.push(format!(
                            "0x{:012x}  needle=\"{}\"  ctx: {}",
                            chunk_addr + s2,
                            String::from_utf8_lossy(needle),
                            snip2
                        ));
                        search = p2 + needle.len();
                        extra += 1;
                    } else { break; }
                }
            }
        }
        } // end 'chunk loop
    }

    unsafe { CloseHandle(process); }
    if results.is_empty() { results.push("No matches found".to_string()); }
    results
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn dump_inventory_regions(_max_hits: usize) -> Vec<String> {
    vec!["Only supported on Windows and Linux".to_string()]
}

// ─── Full-account blob parser ─────────────────────────────────────────────────

/// Find the end of the FULL_ACCOUNT blob by locating `"DeathSquadable":` and
/// the `}` that immediately follows its boolean value (true or false).
fn find_blob_end(raw: &[u8]) -> Option<usize> {
    const KEY: &[u8] = b"\"DeathSquadable\":";
    let key_pos = memmem::find(raw, KEY)?;
    let after   = key_pos + KEY.len();
    // Skip the boolean value and find the closing brace
    let brace = memchr::memchr(b'}', &raw[after..])?;
    Some(after + brace + 1)
}

const START_MARKER: &[u8] = b"\"SubscribedToEmails\"";
const ALT_STARTS: &[&[u8]] = &[
    b"\"MiscItems\":[{\"ItemType\":\"/Lotus/",
    b"\"Suits\":[{\"ItemType\":\"/Lotus/",
    b"\"RegularCredits\":",
];

/// Walk backward from `marker_off` counting braces; return the offset of the
/// outermost `{` that encloses the marker.  Returns `None` when the buffer is
/// inconsistent (freed/partial blob where the opening brace was overwritten).
fn enclosing_object_start(buf: &[u8], marker_off: usize) -> Option<usize> {
    let mut depth: i32 = 0;
    for i in (0..marker_off).rev() {
        match buf[i] {
            b'}' => depth += 1,
            b'{' => {
                depth -= 1;
                if depth < 0 { return Some(i); }
            }
            _ => {}
        }
    }
    None
}

/// Where a stitched buffer's blob begins: `(marker_at, seed_at)`.
///
/// `marker_at` is the marker occurrence the seed is anchored to; `seed_at` is
/// the brace enclosing it, or the marker itself when no occurrence has a
/// brace that reads as an object head. See `enclosing_object_start` for why
/// marker and brace are rarely the same offset.
///
/// With no primary marker anywhere the fallbacks keep both offsets in bounds,
/// down to the buffer's last byte when nothing matches at all. Such a seed is
/// junk and fails to parse, which is what the walk already does with a bad
/// start.
fn blob_seed_offsets(combined: &[u8]) -> (usize, usize) {
    // The heap can hold a stale copy of the blob ahead of the live one: its
    // marker survives but its opening brace was overwritten, so brace-matching
    // backward from it lands on a stray `{` in binary garbage. A live seed is
    // a JSON object head, so only accept a brace followed by a quote, and keep
    // trying later marker occurrences until one qualifies.
    let mut first_marker = None;
    let mut search_from = 0;
    while let Some(found) = memmem::find(&combined[search_from..], START_MARKER) {
        let marker_at = search_from + found;
        first_marker.get_or_insert(marker_at);
        if let Some(open) = enclosing_object_start(combined, marker_at) {
            if combined.get(open + 1) == Some(&b'"') {
                return (marker_at, open);
            }
        }
        search_from = marker_at + 1;
    }
    // Without a plausible brace anywhere, seed at the first marker: the
    // parser can rebuild the object head from a marker-anchored seed.
    let marker_at = first_marker
        .or_else(|| ALT_STARTS.iter().find_map(|a| memmem::find(combined, a)))
        .unwrap_or(combined.len().saturating_sub(1));
    (marker_at, first_marker.unwrap_or_else(||
        enclosing_object_start(combined, marker_at).unwrap_or(marker_at)))
}

/// Parse a FULL_ACCOUNT blob from raw memory bytes into structured inventory data.
///
/// Compute the deterministic riven mod name from its buff stats.
/// Mirrors the RIVEN_NAME_PARTS table in MarketHelper.tsx.
/// 1 buff  → coreSuffix       (buff's prefix word + buff's suffix word)
/// 2 buffs → coreSuffix       (higher's prefix + lower's suffix, no dash)
/// 3 buffs → prefix-coreSuffix (highest - second + lowest, with dash)
pub fn compute_riven_mod_name(buffs: &[BlobRivenStat]) -> String {
    fn parts(tag: &str) -> Option<(&'static str, &'static str)> {
        match tag {
            "WeaponMeleeComboBonusOnHitMod" | "WeaponMeleeComboPointsOnHitMod" => Some(("Laci",  "Nus"  )),
            "WeaponAmmoMaxMod"                                                  => Some(("Ampi",  "Bin"  )),
            "WeaponMeleeFactionDamageCorpus"   | "WeaponFactionDamageCorpus"   => Some(("Manti", "Tron" )),
            "WeaponMeleeFactionDamageGrineer"  | "WeaponFactionDamageGrineer"  => Some(("Argi",  "Con"  )),
            "WeaponMeleeFactionDamageInfested" | "WeaponFactionDamageInfested" => Some(("Pura",  "Ada"  )),
            "WeaponFreezeDamageMod"            => Some(("Geli",  "Do"   )),
            "ComboDurationMod"                 => Some(("Tempi", "Nem"  )),
            "WeaponCritChanceMod"              => Some(("Crita", "Cron" )),
            "SlideAttackCritChanceMod"         => Some(("Pleci", "Nent" )),
            "WeaponCritDamageMod"              => Some(("Acri",  "Tis"  )),
            "WeaponDamageAmountMod" | "WeaponMeleeDamageMod" => Some(("Visi", "Ata")),
            "WeaponElectricityDamageMod"       => Some(("Vexi",  "Tio"  )),
            "WeaponFireDamageMod"              => Some(("Igni",  "Pha"  )),
            "WeaponMeleeFinisherDamageMod"     => Some(("Exi",   "Cta"  )),
            "WeaponFireRateMod"                => Some(("Croni", "Dra"  )),
            "WeaponProjectileSpeedMod"         => Some(("Conci", "Nak"  )),
            "WeaponMeleeComboInitialBonusMod"  => Some(("Para",  "Um"   )),
            "WeaponImpactDamageMod"            => Some(("Magna", "Ton"  )),
            "WeaponClipMaxMod"                 => Some(("Arma",  "Tin"  )),
            "WeaponMeleeComboEfficiencyMod"    => Some(("Forti", "Us"   )),
            "WeaponFireIterationsMod"          => Some(("Sati",  "Can"  )),
            "WeaponToxinDamageMod"             => Some(("Toxi",  "Tox"  )),
            "WeaponPunctureDepthMod"           => Some(("Lexi",  "Nok"  )),
            "WeaponArmorPiercingDamageMod"     => Some(("Insi",  "Cak"  )),
            "WeaponReloadSpeedMod"             => Some(("Feva",  "Tak"  )),
            "WeaponMeleeRangeIncMod"           => Some(("Locti", "Tor"  )),
            "WeaponSlashDamageMod"             => Some(("Sci",   "Sus"  )),
            "WeaponStunChanceMod"              => Some(("Hexa",  "Dex"  )),
            "WeaponProcTimeMod"                => Some(("Deci",  "Des"  )),
            "WeaponRecoilReductionMod"         => Some(("Zeti",  "Mag"  )),
            "WeaponZoomFovMod"                 => Some(("Hera",  "Lis"  )),
            _ => None,
        }
    }
    if buffs.is_empty() { return String::new(); }
    let mut sorted: Vec<&BlobRivenStat> = buffs.iter().collect();
    sorted.sort_by(|a, b| b.value.cmp(&a.value));
    let Some((hi_p, _))  = parts(&sorted[0].tag)                   else { return String::new(); };
    let Some((_, lo_s))  = parts(&sorted[sorted.len() - 1].tag)    else { return String::new(); };
    if sorted.len() >= 3 {
        if let Some((mid_p, _)) = parts(&sorted[1].tag) {
            return format!("{}-{}{}", hi_p.to_lowercase(), mid_p.to_lowercase(), lo_s.to_lowercase());
        }
    }
    format!("{}{}", hi_p.to_lowercase(), lo_s.to_lowercase())
}

/// Cut the JSON object out of a stitched memory buffer.
///
/// A scan buffer is whole memory regions glued together, so the blob's last
/// brace is followed by whatever heap bytes happened to sit in the tail of the
/// final region — potentially tens of megabytes of noise. This trims to just
/// the valid JSON object so both the parser and the debug dump files see clean data.
pub fn extract_blob_json(raw: &[u8]) -> Option<Vec<u8>> {
    extract_blob_json_at(raw, find_blob_end(raw)?).map(Cow::into_owned)
}

/// Borrowing form taking an already-known `end_pos`, so callers that located the
/// blob end for their own purposes (e.g. the minimum-size check in
/// [`parse_full_account_blob`]) don't pay for a second scan. The common case
/// (buffer still starts with the original `{`) needs no copy at all; only the
/// fallback, where the opening brace was overwritten in memory and has to be
/// reinstated, allocates.
fn extract_blob_json_at(raw: &[u8], end_pos: usize) -> Option<Cow<'_, [u8]>> {
    if raw.first() == Some(&b'{') {
        Some(Cow::Borrowed(&raw[..end_pos]))
    } else {
        let start_pos = memmem::find(raw, START_MARKER)?;
        let mut v = Vec::with_capacity(end_pos - start_pos + 1);
        v.push(b'{');
        v.extend_from_slice(&raw[start_pos..end_pos]);
        Some(Cow::Owned(v))
    }
}

/// `raw` must span from the JSON opening `{` (or from `"SubscribedToEmails"`) through
/// `"DeathSquadable":`. Returns `None` if neither start can be located or JSON is malformed.
#[tracing::instrument(level = "debug", skip_all)]
pub fn parse_full_account_blob(raw: &[u8]) -> Option<BlobInventory> {
    let end_pos = find_blob_end(raw)?;

    // Real FULL_ACCOUNT blobs are several hundred KB — anything smaller is a
    // small false-positive fragment that matched the end marker by coincidence.
    const MIN_PARSE_BYTES: usize = 50_000;
    if end_pos < MIN_PARSE_BYTES {
        debug!(target: "frameforge::blob_parse", end_pos, min = MIN_PARSE_BYTES, "too small — skipping");
        return None;
    }

    // Section completeness check: a partial/mid-write blob may pass all marker
    // checks (SubscribedToEmails present, DeathSquadable present, size OK) yet be
    // missing MiscItems, RegularCredits, and other top-level sections entirely.
    // Reject such blobs before the expensive JSON parse — they would wipe the
    // displayed inventory even though prior state was valid.
    const REQUIRED_SECTIONS: &[&[u8]] = &[
        b"\"MiscItems\":",
        b"\"RegularCredits\":",
        b"\"Suits\":",
        b"\"XPInfo\":",
        b"\"FusionPoints\":",
    ];
    let search_range = &raw[..end_pos.min(raw.len())];
    for required in REQUIRED_SECTIONS {
        if memchr::memmem::find(search_range, required).is_none() {
            debug!(
                target: "frameforge::blob_parse",
                missing = %std::str::from_utf8(required).unwrap_or("?"),
                "incomplete blob — missing required section, skipping"
            );
            return None;
        }
    }

    let json_bytes = extract_blob_json_at(raw, end_pos)?;

    let json: serde_json::Value = serde_json::from_slice(&json_bytes)
        .map_err(|e| {
            let head: String = json_bytes[..json_bytes.len().min(48)]
                .iter().map(|&b| if b >= 0x20 && b < 0x7f { b as char } else { '.' }).collect();
            debug!(target: "frameforge::blob_parse", error = %e, head = ?head, "JSON error");
        })
        .ok()?;

    // Scalars
    let credits       = json["RegularCredits"].as_i64().unwrap_or(0);
    let endo          = json["FusionPoints"].as_i64().unwrap_or(0);
    let platinum      = json["PremiumCredits"].as_i64().unwrap_or(0);
    let free_platinum = json["PremiumCreditsFree"].as_i64().unwrap_or(0);
    let mastery_level = json["PlayerLevel"].as_u64().unwrap_or(0) as u32;

    // Unique item sections — each array entry = one owned copy
    const UNIQUE_SECS: &[&str] = &[
        "Suits", "LongGuns", "Pistols", "Melee",
        "SpaceSuits", "SpaceMelee", "SpaceGuns",
        "Sentinels", "SentinelWeapons", "KubrowPets",
        "OperatorAmps", "MechSuits",
    ];
    let mut unique_items = Vec::new();
    for &sec in UNIQUE_SECS {
        if let Some(arr) = json[sec].as_array() {
            for e in arr {
                let Some(it) = e["ItemType"].as_str() else { continue };
                if !it.starts_with("/Lotus/") { continue; }
                let archon_shards = e["ArchonCrystalUpgrades"].as_array()
                    .map(|a| a.iter().filter_map(|s| {
                        Some(ArchonShard {
                            color:        s["Color"].as_str()?.to_string(),
                            upgrade_type: s["UpgradeType"].as_str().unwrap_or("").to_string(),
                        })
                    }).collect())
                    .unwrap_or_default();
                let modular_parts = e["ModularParts"].as_array()
                    .map(|a| a.iter().filter_map(|p| p.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                unique_items.push(BlobUniqueEntry {
                    item_type:     it.to_string(),
                    section:       sec.to_string(),
                    polarized:     e["Polarized"].as_u64().unwrap_or(0) as u32,
                    xp:            e["XP"].as_i64().unwrap_or(0),
                    item_name:     e["ItemName"].as_str().map(String::from),
                    pet_name:      e["Details"]["Name"].as_str().map(String::from),
                    focus_lens:    e["FocusLens"].as_str().map(String::from),
                    archon_shards,
                    modular_parts,
                });
            }
        }
    }

    // Stackable item sections
    const STACK_SECS: &[(&str, bool)] = &[
        ("MiscItems",          false),
        ("Recipes",            false),
        ("FusionTreasures",    true),   // has Sockets
        ("CrewShipRawSalvage", false),
        ("ShipDecorations",    false),
    ];
    let mut stackable_items = Vec::new();
    for &(sec, has_sockets) in STACK_SECS {
        if let Some(arr) = json[sec].as_array() {
            for e in arr {
                let Some(it) = e["ItemType"].as_str() else { continue };
                if !it.starts_with("/Lotus/") { continue; }
                let count = e["ItemCount"].as_i64().unwrap_or(0);
                if count <= 0 { continue; }
                stackable_items.push(BlobStackableEntry {
                    item_type:  it.to_string(),
                    item_count: count,
                    sockets:    if has_sockets { e["Sockets"].as_i64() } else { None },
                });
            }
        }
    }

    // Rivens + Mods: RawUpgrades (unranked, ItemCount) + Upgrades (ranked, one entry = one copy).
    // Riven paths contain "RandomMod" — extract them separately and skip from mods map.
    let mut rivens: Vec<BlobRivenEntry> = Vec::new();
    let mut mods: HashMap<String, ModCount> = HashMap::new();
    if let Some(arr) = json["RawUpgrades"].as_array() {
        for e in arr {
            let Some(it) = e["ItemType"].as_str() else { continue };
            if !it.starts_with("/Lotus/") { continue; }
            let count = e["ItemCount"].as_i64().unwrap_or(0);
            if count <= 0 { continue; }
            if it.contains("RandomMod") {
                // Unrevealed riven: stacked in RawUpgrades, only type visible.
                rivens.push(BlobRivenEntry {
                    item_id:  String::new(),
                    item_type: it.to_string(),
                    riven_state: RivenState::Unrevealed,
                    compat: None, challenge_type: None, challenge_complication: None,
                    lvl_req: None, polarity: None,
                    buffs: vec![], curses: vec![],
                    mod_rank: 0, count: count as u32, rerolls: 0,
                    mod_name: String::new(),
                });
                continue;
            }
            let mc = blob_entry(&mut mods, it);
            *mc.by_rank.entry(0).or_insert(0) += count;
            mc.total += count;
        }
    }
    if let Some(arr) = json["Upgrades"].as_array() {
        for e in arr {
            let Some(it) = e["ItemType"].as_str() else { continue };
            if !it.starts_with("/Lotus/") { continue; }
            if it.contains("RandomMod") {
                let fp_str = e["UpgradeFingerprint"].as_str().unwrap_or("{}");
                if let Ok(fp) = serde_json::from_str::<serde_json::Value>(fp_str) {
                    let item_id = e["ItemId"]["$oid"].as_str().unwrap_or("").to_string();
                    if let Some(compat) = fp["compat"].as_str() {
                        // Unlocked riven: weapon assigned + stats visible.
                        let buffs: Vec<BlobRivenStat> = fp["buffs"].as_array()
                            .map(|a| a.iter().filter_map(|s| Some(BlobRivenStat {
                                tag:   s["Tag"].as_str()?.to_string(),
                                value: s["Value"].as_i64().unwrap_or(0),
                            })).collect())
                            .unwrap_or_default();
                        let curses: Vec<BlobRivenStat> = fp["curses"].as_array()
                            .map(|a| a.iter().filter_map(|s| Some(BlobRivenStat {
                                tag:   s["Tag"].as_str()?.to_string(),
                                value: s["Value"].as_i64().unwrap_or(0),
                            })).collect())
                            .unwrap_or_default();
                        let mod_name = compute_riven_mod_name(&buffs);
                        rivens.push(BlobRivenEntry {
                            item_id, item_type: it.to_string(),
                            riven_state: RivenState::Unlocked,
                            compat: Some(compat.to_string()),
                            challenge_type: None, challenge_complication: None,
                            lvl_req:  fp["lvlReq"].as_u64().map(|v| v as u32),
                            polarity: fp["pol"].as_str().map(String::from),
                            mod_rank: fp["lvl"].as_u64().map(|v| v as u8).unwrap_or(0),
                            count: 1,
                            rerolls: fp["rerolls"].as_u64().unwrap_or(0) as u32,
                            mod_name,
                            buffs,
                            curses,
                        });
                        continue;
                    } else if fp["challenge"].is_object() {
                        // Revealed riven: challenge assigned but not yet completed.
                        let challenge_type = fp["challenge"]["Type"].as_str().map(String::from);
                        let challenge_complication = fp["challenge"]["Complication"].as_str().map(String::from);
                        rivens.push(BlobRivenEntry {
                            item_id, item_type: it.to_string(),
                            riven_state: RivenState::Revealed,
                            compat: None, challenge_type, challenge_complication,
                            lvl_req: None, polarity: None,
                            buffs: vec![], curses: vec![],
                            mod_rank: 0, count: 1, rerolls: 0,
                            mod_name: String::new(),
                        });
                        continue;
                    }
                }
            }
            let rank = blob_extract_mod_rank(e["UpgradeFingerprint"].as_str());
            let mc = blob_entry(&mut mods, it);
            *mc.by_rank.entry(rank).or_insert(0) += 1;
            mc.total += 1;
        }
    }

    // FlavourItems (glyphs, palettes, emotes, titles, ship skins): each entry = one copy.
    let mut flavour_items: HashMap<String, i64> = HashMap::new();
    if let Some(arr) = json["FlavourItems"].as_array() {
        for e in arr {
            let Some(it) = e["ItemType"].as_str() else { continue };
            if !it.starts_with("/Lotus/") { continue; }
            *blob_entry(&mut flavour_items, it) += 1;
        }
    }

    // WeaponSkins (sigils, cosmetic skins): each array entry = one owned copy,
    // count occurrences of the same ItemType.
    let mut weapon_skins: HashMap<String, i64> = HashMap::new();
    if let Some(arr) = json["WeaponSkins"].as_array() {
        for e in arr {
            let Some(it) = e["ItemType"].as_str() else { continue };
            if !it.starts_with("/Lotus/") { continue; }
            *blob_entry(&mut weapon_skins, it) += 1;
        }
    }

    // XPInfo → mastery ranks (covers items no longer owned)
    let mut mastery_data: HashMap<String, u32> = HashMap::new();
    if let Some(arr) = json["XPInfo"].as_array() {
        for e in arr {
            let Some(it) = e["ItemType"].as_str() else { continue };
            if let Some(xp) = e["XP"].as_i64() {
                let rank = xp_to_rank(xp, it);
                if rank > 0 { mastery_data.insert(it.to_string(), rank); }
            }
        }
    }

    // PendingRecipes (Foundry)
    let pending_recipes: Vec<BlobPendingRecipe> = json["PendingRecipes"].as_array()
        .map(|a| a.iter().filter_map(|e| {
            let it = e["ItemType"].as_str()?.to_string();
            let ms = e["CompletionDate"]["$date"]["$numberLong"]
                .as_str().and_then(|s| s.parse::<i64>().ok())
                .or_else(|| e["CompletionDate"]["$date"]["$numberLong"].as_i64())
                .unwrap_or(0);
            Some(BlobPendingRecipe { item_type: it, completion_ms: ms })
        }).collect())
        .unwrap_or_default();

    // Helminth consumed suits
    let consumed_suits: Vec<String> = json["InfestedFoundry"]["ConsumedSuits"].as_array()
        .map(|a| a.iter().filter_map(|e| e["s"].as_str().map(String::from)).collect())
        .unwrap_or_default();

    // Every valid FULL_ACCOUNT blob at the orbiter has at least one Warframe in Suits.
    // An empty unique_items means we captured an incomplete blob (game is mid-write,
    // returning from mission, or the blob sections were partially stitched out of order).
    if unique_items.is_empty() {
        debug!(target: "frameforge::blob_parse", "blob has no warframes/weapons — incomplete blob, rejecting");
        return None;
    }

    Some(BlobInventory {
        credits, endo, platinum, free_platinum, mastery_level,
        unique_items, stackable_items, mods,
        flavour_items, weapon_skins, mastery_data, pending_recipes, consumed_suits,
        rivens,
    })
}

/// Duplicate ItemTypes dominate every map the blob parse builds, so the lookup
/// checks `get_mut` first rather than paying for `entry()`'s unconditional
/// `to_string()` on each of the tens of thousands of repeats.
fn blob_entry<'a, V: Default>(map: &'a mut HashMap<String, V>, key: &str) -> &'a mut V {
    // The double lookup is what the borrow checker costs here; it is still
    // cheaper than allocating a key per occurrence.
    if map.contains_key(key) {
        map.get_mut(key).expect("just checked the key is present")
    } else {
        map.entry(key.to_string()).or_default()
    }
}

/// Extract the `lvl` field from a mod UpgradeFingerprint JSON string.
/// Returns 0 for unranked or missing fingerprint.
fn blob_extract_mod_rank(fingerprint: Option<&str>) -> u8 {
    fingerprint
        .and_then(|fp| {
            let pos = fp.find("\"lvl\":")?;
            let after = fp[pos + 6..].trim_start();
            let end = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
            after[..end].parse::<u8>().ok()
        })
        .unwrap_or(0)
}

// ─── Blob capture ─────────────────────────────────────────────────────────────

// Cache: remember the region address where the blob was last successfully found.
// On the next cycle we probe that address first — if the blob is still there we
// finish in milliseconds instead of walking the full address space.
pub(crate) static LAST_BLOB_REGION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

// Digest of the last blob whose bytes were about to be parsed. Inventory
// changes maybe once per mission, so most 10s scan cycles find byte-identical
// JSON — hashing a few MB is far cheaper than rebuilding BlobInventory's
// HashMaps/Vecs from scratch every cycle.
static LAST_BLOB_DIGEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Outcome of probing the cached region address: either the bytes changed and
/// were reparsed, or they're identical to what the previous cycle already
/// sent and there's nothing new to do.
pub(crate) enum CachedBlobScan {
    Fresh(usize, BlobInventory),
    Unchanged,
}

/// Set once the probe has reported that nothing changed, cleared as soon as
/// anything does. Probes run every couple of seconds and nearly all of them
/// find byte-identical JSON, so logging each one drowns out the rest of the
/// log. Only the transition into that state is logged.
static STEADY_STATE_LOGGED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// True the first time the probe settles into "unchanged", false for every
/// repeat until [`blob_unchanged`] sees bytes that differ.
fn steady_state_notice_due() -> bool {
    !STEADY_STATE_LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed)
}

pub fn has_cached_blob() -> bool {
    LAST_BLOB_REGION.load(std::sync::atomic::Ordering::Relaxed) != 0
}

/// Clear the fast-path region cache. Call when Warframe's PID changes so the
/// next scan doesn't probe a stale address from the previous process instance.
pub fn reset_last_blob_region() {
    LAST_BLOB_REGION.store(0, std::sync::atomic::Ordering::Relaxed);
    LAST_BLOB_DIGEST.store(0, std::sync::atomic::Ordering::Relaxed);
    STEADY_STATE_LOGGED.store(false, std::sync::atomic::Ordering::Relaxed);
}

/// Discard the digest baseline so the next candidate is parsed no matter what
/// its bytes are. Call after a parse failure: skipping a re-parse is only safe
/// while the baseline names bytes that are known to parse, and `blob_unchanged`
/// records its argument before the parse outcome is known.
fn forget_blob_digest() {
    LAST_BLOB_DIGEST.store(0, std::sync::atomic::Ordering::Relaxed);
}

/// Checks `json` against the digest recorded by the previous call, then
/// records `json`'s digest as the new baseline — check-and-update in one
/// step, so a caller should invoke this exactly once per candidate blob,
/// right before deciding whether to parse it.
///
/// A caller that then fails to parse `json` must call `forget_blob_digest`:
/// the skip paths treat a match as "already parsed this successfully", and
/// unparseable bytes that persist across cycles would otherwise be mistaken
/// for a result and suppress the rest of the walk indefinitely.
///
/// Returns true when `json` is byte-identical to what the previous call saw.
fn blob_unchanged(json: &[u8]) -> bool {
    use std::hash::{DefaultHasher, Hash, Hasher};
    // Callers hand over a stitched buffer, not a trimmed blob. The stitch stops
    // at the mapping that closes the JSON, so everything past the closing brace
    // is whatever heap shared that mapping, tens of megabytes of it, rewritten
    // constantly by a running client. Hash that tail and the digest never
    // matches, so every probe reparses and the skip never happens. Bytes with no
    // blob end are hashed whole; they only need to compare equal to themselves.
    let blob = &json[..find_blob_end(json).unwrap_or(json.len())];
    let mut hasher = DefaultHasher::new();
    blob.hash(&mut hasher);
    // OR in a set bit so a hashed digest can never equal the 0 sentinel that
    // reset_last_blob_region stores — that sentinel must always compare as
    // "changed" to force a re-parse after a PID change.
    let digest = hasher.finish() | 1;
    let unchanged = LAST_BLOB_DIGEST.swap(digest, std::sync::atomic::Ordering::Relaxed) == digest;
    if !unchanged {
        // Bytes moved, so the next settle into the steady state is worth
        // saying out loud again.
        STEADY_STATE_LOGGED.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    unchanged
}

// ─── Shared constants ─────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
const MAX_READ: usize = 64 * 1024 * 1024;
// Shared by the cold walk and the cached-region fast path, so a region
// rejected as a mission delta or a scan dropped for growing past the cap
// means the same thing in either place.
pub(crate) const MAX_SCAN: usize = 20 * 1024 * 1024;
const MISSION_DELTA: &[u8] = b"\"InventoryChanges\":";
const LOTUS_KEY: &[u8] = b"/Lotus/";
/// The blob's last field. Finding it is not finding the blob's end: the
/// closing brace can still be a region away. It only gates the
/// `find_blob_end` call that answers that question.
const END_MARKER: &[u8] = b"\"DeathSquadable\":";
const ANCHORS: &[&[u8]] = &[
    b"\"SubscribedToEmails\"",
    b"\"MiscItems\":[",
    b"\"Suits\":[",
    b"\"LongGuns\":[",
    b"\"Melee\":[",
    b"\"Pistols\":[",
];

/// Re-read the blob straight from the address the last successful scan found
/// it at, stitching forward through following regions until the JSON closes.
///
/// The game rarely moves the blob between cycles, and the full walk reads
/// gigabytes to reach it. Probing the remembered address first turns the
/// common case into a few megabytes.
///
/// Returns `None` whenever anything looks different from last time, which puts
/// the caller back on the full walk rather than reporting a stale inventory.
#[tracing::instrument(level = "debug", skip_all)]
pub(crate) fn scan_cached_blob(
    src: &dyn crate::mem_regions::RegionSource,
) -> Option<CachedBlobScan> {
    let cached_addr = LAST_BLOB_REGION.load(std::sync::atomic::Ordering::Relaxed) as usize;
    if cached_addr == 0 {
        return None;
    }

    let (mut next_addr, first_bytes) = src.read_at(cached_addr)?;
    if first_bytes.len() < 8 {
        debug!(addr = format_args!("0x{cached_addr:012x}"), "fast-path miss — unreadable");
        return None;
    }

    let is_mission = memmem::find(&first_bytes, MISSION_DELTA).is_some();
    let has_anchor = ANCHORS.iter().any(|a| memmem::find(&first_bytes, a).is_some());
    let has_lotus  = memmem::find(&first_bytes, LOTUS_KEY).is_some();
    // The walk seeds from the blob's opening brace, or from the start marker
    // itself when the brace sits in a region it could not stitch. Both shapes
    // are accepted. Anything else reads as a stale address. Field order varies
    // by account, so a region counts as blob data on its anchors even when
    // SubscribedToEmails appears further along.
    let seeds_a_blob = first_bytes.starts_with(b"{\"") || first_bytes.starts_with(START_MARKER);
    if is_mission || !(has_anchor || has_lotus) || !seeds_a_blob {
        debug!(addr = format_args!("0x{cached_addr:012x}"), "fast-path miss — not blob data");
        return None;
    }

    let mut stitched = first_bytes;
    // The end-marker search covers only the newly appended bytes, backed off by
    // one marker length so a copy split across a region boundary is still
    // caught. It latches once seen, so a marker flush against a boundary is not
    // left behind the search window.
    let mut search_from = 0;
    let mut end_seen = false;
    loop {
        if !end_seen {
            let scan_from = search_from;
            search_from = stitched.len().saturating_sub(END_MARKER.len() - 1);
            end_seen = memmem::find(&stitched[scan_from..], END_MARKER).is_some();
        }
        if end_seen && find_blob_end(&stitched).is_some() {
            break;
        }
        if stitched.len() >= MAX_SCAN {
            break;
        }
        let Some((end, bytes)) = src.read_at(next_addr) else { break };
        next_addr = end;
        // An unreadable mapping ends the stitch: the blob is contiguous, so a
        // gap means the address no longer holds what it held last cycle.
        if bytes.is_empty() {
            break;
        }
        let fits = bytes.len().min(MAX_SCAN - stitched.len());
        stitched.extend_from_slice(&bytes[..fits]);
    }

    if blob_unchanged(&stitched) {
        if steady_state_notice_due() {
            debug!(addr = format_args!("0x{cached_addr:012x}"), "fast-path hit: unchanged since last scan; quiet until it changes");
        }
        return Some(CachedBlobScan::Unchanged);
    }
    match parse_full_account_blob(&stitched) {
        Some(inventory) => Some(CachedBlobScan::Fresh(cached_addr, inventory)),
        None => {
            forget_blob_digest();
            None
        }
    }
}

/// Scans Warframe process memory for the FULL_ACCOUNT inventory blob and sends it
/// through `blob_tx` for the monitor loop to apply.
///
/// Multi-scan strategy: the blob may span many memory regions and multiple copies
/// can exist at different addresses. We track every potential start point as a
/// separate in-flight scan and stitch them all in parallel as the region walk
/// advances. The first scan that produces a valid JSON blob wins; all others are
/// dropped. This is far more robust than the old single-start approach when the
/// blob is large or when the first start hit leads to a truncated region.
///
/// Algorithm:
///   1. Walk every committed readable region.
///   2. If a region has START_MARKER ("SubscribedToEmails") and is NOT a mission
///      delta ("InventoryChanges"), open a new ActiveScan seeded with that region's
///      data from the START_MARKER offset onwards.
///   3. Every readable region is appended to ALL active scans (stitching).
///   4. After each append, check every scan for the end marker. If found, parse it.
///      On success send the inventory to the monitor loop. On failure drop the scan.
///      The walk always continues through all of memory — every blob start is found.
///   5. Drop any scan that grows past MAX_SCAN_BYTES without finding the end.
///
/// When `save=true` also writes the raw text to `blob_dir` for debugging.
/// Returns the number of files written (always 0 when `save=false`).
#[cfg(target_os = "windows")]
#[tracing::instrument(level = "debug", skip_all, fields(save = save))]
pub fn capture_all_blobs(blob_dir: &std::path::Path, ts: &str, blob_tx: std::sync::mpsc::Sender<BlobInventory>, save: bool) -> usize {
    const MIN_REGION: usize = 64_000;

    let pid = match find_warframe_pid_pub() { Some(p) => p, None => return 0 };
    let mut src = match crate::mem_regions::WindowsRegionSource::open(pid, MIN_REGION, MAX_READ) {
        Some(s) => s,
        None => return 0,
    };

    // No fast path here on purpose. The monitor already ran that scan in
    // `probe_tick` and escalated on the result. Repeating it here would answer
    // the same and skip the walk the monitor asked for.
    let saved = stitch_blobs(&mut src, blob_dir, ts, blob_tx, save).unwrap_or(0);
    let (regions_skipped, vquery_ms, read_ms) = src.stats();
    debug!(
        target: "frameforge::blob_capture",
        regions_skipped,
        vquery_ms,
        read_ms,
        "source stats"
    );
    saved
}

/// Walk all memory regions via `src`, stitch blobs, parse and send them.
///
/// `None` means no blob completed at all. `Some` carries the number of blob
/// files written, always 0 when `save=false`. A walk that stops early on a full
/// inventory writes and sends nothing, so the count alone cannot say whether
/// the walk found anything.
pub(crate) fn stitch_blobs(
    src: &mut dyn crate::mem_regions::RegionSource,
    blob_dir: &std::path::Path,
    ts: &str,
    blob_tx: std::sync::mpsc::Sender<BlobInventory>,
    save: bool,
) -> Option<usize> {
    const MAX_BLOBS: usize = 25;

    struct ActiveScan {
        data: Vec<u8>,
        id: usize,
        /// Absolute address of the JSON opening brace this scan was seeded at
        /// (mid-region, not a region base). Cached in LAST_BLOB_REGION on success.
        seed_addr: usize,
        /// Minimum offset at which the end-marker search should start next append.
        /// Avoids rescanning already-checked data on every region append (O(n²) → O(n)).
        search_from: usize,
        /// Once set, `search_from` stops advancing, so a marker flush against
        /// a region edge is not left behind the search window.
        end_seen: bool,
    }
    let mut scans: Vec<ActiveScan> = Vec::new();
    let mut next_scan_id = 0usize;

    // Pre-buffer: rolling window of recent regions that have Lotus paths / anchor keys
    // but no SubscribedToEmails yet.  Used to recover the true JSON start when the
    // outer `{` of the FULL_ACCOUNT blob lives in a region that precedes the region
    // containing SubscribedToEmails (field order varies by account).
    struct PreChunk { addr: usize, end_addr: usize, data: Vec<u8> }
    let mut pre_buf: std::collections::VecDeque<PreChunk> = std::collections::VecDeque::new();
    const PRE_BUF_BYTES: usize = 8 * 1024 * 1024; // keep ≤8 MB of prefix history

    let mut saved = 0usize;
    let mut regions_read    = 0usize;
    let mut starts_found    = 0usize;
    let mut t_search = std::time::Duration::ZERO;
    let mut bytes_read: u64 = 0;
    // Once we have at least one successful parse we stop opening new scans.
    // Active scans already in progress are still stitched to completion (or dropped).
    // The loop exits as soon as all active scans are gone.
    let mut found_result = false;

    loop {
        if saved >= MAX_BLOBS { break; }
        // Early exit: we have a result and no active scans left to finish.
        if found_result && scans.is_empty() && !save { break; }

        let (region_addr, buf) = match src.next_region() {
            Some(r) => r,
            None => break,
        };
        let n = buf.len();
        bytes_read += n as u64;
        let chunk = &buf[..];
        regions_read += 1;

        // ── Step 1: append this chunk to every active scan and check for completion ──
        // search_from tracks where we left off so we only scan newly-appended bytes
        // (plus a small overlap for markers that straddle a region boundary).
        scans.retain_mut(|scan| {
            // A previous scan in this same retain_mut pass already succeeded.
            // Drop this one immediately — applying a second blob overwrites correct data
            // with a stale/parallel copy from a different memory region.
            if found_result && !save { return false; }
            // The append is capped at what is left of the budget rather than
            // dropped for overrunning it. A blob that closes on the very last
            // byte the budget allows still parses.
            let remaining = MAX_SCAN.saturating_sub(scan.data.len());
            let exceeds_limit = chunk.len() > remaining;
            if !scan.end_seen {
                // Advance the search cursor before appending so the overlap catches split markers.
                let search_from = scan.search_from;
                scan.search_from = scan.data.len().saturating_sub(END_MARKER.len() - 1);
                scan.data.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                scan.end_seen =
                    memchr::memmem::find(&scan.data[search_from..], END_MARKER).is_some();
            } else {
                scan.data.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            let complete = scan.end_seen && find_blob_end(&scan.data).is_some();
            if !complete {
                if exceeds_limit {
                    warn!(scan_id = scan.id, max_mb = MAX_SCAN / 1024 / 1024, "scan exceeded size limit without end — dropped");
                    return false; // drop oversized scan
                }
                return true; // keep waiting for end
            }
            if !save && blob_unchanged(&scan.data) {
                debug!(scan_id = scan.id, "unchanged since last scan — skipping parse");
                LAST_BLOB_REGION.store(scan.seed_addr as u64, std::sync::atomic::Ordering::Relaxed);
                found_result = true;
                return false;
            }
            match parse_full_account_blob(&scan.data) {
                Some(inv) => {
                    info!(
                        scan_id = scan.id,
                        addr = format_args!("0x{:012x}", scan.seed_addr),
                        unique = inv.unique_items.len(),
                        stackable = inv.stackable_items.len(),
                        mods = inv.mods.len(),
                        "scan SUCCESS"
                    );
                    LAST_BLOB_REGION.store(scan.seed_addr as u64, std::sync::atomic::Ordering::Relaxed);
                    if save {
                        let name = format!("Actual_inventory_FULL_ACCOUNT_{}_{:02}.txt", ts, saved + 1);
                        let path = blob_dir.join(&name);
                        if let Some(json) = extract_blob_json(&scan.data) {
                            if std::fs::write(&path, &json).is_ok() { saved += 1; }
                        }
                    }
                    blob_tx.send(inv).ok();
                    found_result = true;
                }
                None => {
                    warn!(scan_id = scan.id, "end marker found but JSON parse failed — dropped");
                    forget_blob_digest();
                }
            }
            false // remove completed (or failed) scan
        });

        // ── Step 2: check if this chunk opens a new scan ──
        // Don't open new scans once we already have a result — drain the active ones then exit.
        if found_result { continue; }

        let t2 = std::time::Instant::now();
        // Every region pays for this one search, since it gates both
        // prefix-buffer eligibility and qualification. The rest run only once a
        // region already looks like blob data. `/Lotus/` leads because an
        // inventory region contains it within the first few hundred bytes,
        // which cuts the anchor scans short.
        let blob_shaped   = memchr::memmem::find(chunk, LOTUS_KEY).is_some()
            || ANCHORS.iter().any(|a| memchr::memmem::find(chunk, a).is_some());
        let has_start     = blob_shaped && memchr::memmem::find(chunk, START_MARKER).is_some();
        let has_alt_start = blob_shaped && !has_start
            && ALT_STARTS.iter().any(|a| memchr::memmem::find(chunk, a).is_some());
        let is_mission    = blob_shaped && memchr::memmem::find(chunk, MISSION_DELTA).is_some();
        let qualifies     = (has_start || has_alt_start) && !is_mission;
        t_search += t2.elapsed();

        // Accumulate regions with Lotus paths (no SubscribedToEmails, no mission delta) into
        // a pre-buffer.  When SubscribedToEmails is found in a later region, we prepend
        // contiguous pre-buffer regions so that the backward {"  search finds the true
        // outermost JSON opening rather than a nested {"$oid":…} inside the blob.
        if blob_shaped && !has_start && !is_mission {
            // Keep only the tail of large regions in the pre-buffer: a chunk
            // bigger than PRE_BUF_BYTES can never be fully kept, so trim it to
            // PRE_BUF_BYTES before enqueueing and skip the normal eviction loop.
            let chunk_data = if n > PRE_BUF_BYTES {
                chunk[n - PRE_BUF_BYTES..].to_vec()
            } else {
                while pre_buf.iter().map(|p| p.data.len()).sum::<usize>() + n > PRE_BUF_BYTES
                    && !pre_buf.is_empty()
                {
                    pre_buf.pop_front();
                }
                chunk.to_vec()
            };
            let stored_len = chunk_data.len();
            pre_buf.push_back(PreChunk {
                addr:     region_addr + (n - stored_len),
                end_addr: region_addr + n,
                data:     chunk_data,
            });
        }

        if qualifies {
            // Prepend any contiguous pre-buffer regions that immediately precede this one.
            // This recovers the full blob when the outer { lives in an earlier region and
            // SubscribedToEmails appears later (field order varies per account/build).
            let mut combined: Vec<u8> = Vec::new();
            let mut blob_start_addr = region_addr;
            {
                let mut expect_end = region_addr;
                let mut chain: Vec<usize> = Vec::new();
                for (i, pc) in pre_buf.iter().enumerate().rev() {
                    // Allow ≤4 KB alignment gap between regions.
                    if pc.end_addr <= expect_end && pc.end_addr + 4096 >= expect_end {
                        chain.push(i);
                        expect_end = pc.addr;
                    } else if pc.end_addr < expect_end.saturating_sub(4096) {
                        break;
                    }
                }
                chain.reverse();
                for &i in &chain {
                    let pc = &pre_buf[i];
                    if combined.is_empty() { blob_start_addr = pc.addr; }
                    combined.extend_from_slice(&pc.data);
                }
            }
            combined.extend_from_slice(chunk);

            let (start_off, json_open) = blob_seed_offsets(&combined);

            // Absolute memory address of the seed start (for LAST_BLOB_REGION cache).
            let seed_addr = blob_start_addr + json_open;

            let id = next_scan_id;
            next_scan_id += 1;
            starts_found += 1;
            let pre_bytes = combined.len() - n;
            debug!(
                scan_id = id,
                addr = format_args!("0x{region_addr:012x}"),
                start_off,
                json_open,
                seed = format_args!("0x{seed_addr:012x}"),
                pre_bytes,
                "scan started"
            );
            let seed = combined[json_open..].to_vec();

            let seed_ends = find_blob_end(&seed).is_some();
            if seed_ends && !save && blob_unchanged(&seed) {
                debug!(scan_id = id, "immediate hit: unchanged since last scan — skipping parse");
                LAST_BLOB_REGION.store(seed_addr as u64, std::sync::atomic::Ordering::Relaxed);
                found_result = true;
            } else if seed_ends {
                match parse_full_account_blob(&seed) {
                    Some(inv) => {
                        info!(
                            scan_id = id,
                            addr = format_args!("0x{region_addr:012x}"),
                            unique = inv.unique_items.len(),
                            stackable = inv.stackable_items.len(),
                            "scan immediate SUCCESS"
                        );
                        LAST_BLOB_REGION.store(seed_addr as u64, std::sync::atomic::Ordering::Relaxed);
                        if save {
                            let name = format!("Actual_inventory_FULL_ACCOUNT_{}_{:02}.txt", ts, saved + 1);
                            if let Some(json) = extract_blob_json(&seed) {
                                if std::fs::write(blob_dir.join(&name), &json).is_ok() { saved += 1; }
                            }
                        }
                        blob_tx.send(inv).ok();
                        found_result = true;
                    }
                    None => {
                        warn!(scan_id = id, "immediate end found but parse failed — dropping");
                        forget_blob_digest();
                    }
                }
            } else {
                scans.push(ActiveScan { data: seed, id, seed_addr, search_from: 0, end_seen: false });
            }
        }
    }

    debug!(
        target: "frameforge::blob_capture",
        regions_read,
        starts_found,
        saved,
        bytes_mb = bytes_read / 1_000_000,
        search_ms = t_search.as_secs_f64() * 1000.0,
        "capture done"
    );
    if starts_found == 0 {
        warn!(target: "frameforge::blob_capture", "no start-marker found — FULL_ACCOUNT not in memory (game in mission, on login screen, or Arsenal not open?)");
    }
    found_result.then_some(saved)
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn capture_all_blobs(_blob_dir: &std::path::Path, _ts: &str, _blob_tx: std::sync::mpsc::Sender<BlobInventory>, _save: bool) -> usize { 0 }

// ─── Cheap probe ──────────────────────────────────────────────────────────────

/// What a probe of the cached blob address concluded.
///
/// `Unchanged` and `Updated` are definitive answers obtained for a few
/// megabytes of reads. `CacheMiss` is not: the game may have reallocated the
/// blob because the inventory changed, or the address may be stale for some
/// unrelated reason, and telling those apart costs a full region walk.
#[derive(Debug, PartialEq, Eq)]
pub enum ScanOutcome {
    /// Cached address hit, bytes identical to the last cycle.
    Unchanged,
    /// Blob parsed and sent through `blob_tx`.
    Updated,
    /// Cached address absent, stale, or unparseable.
    CacheMiss,
}

/// Map a cached-region scan onto a probe outcome, sending any fresh inventory.
pub(crate) fn probe_outcome(
    scan: Option<CachedBlobScan>,
    blob_tx: &std::sync::mpsc::Sender<BlobInventory>,
) -> ScanOutcome {
    match scan {
        Some(CachedBlobScan::Fresh(address, inventory)) => {
            debug!(
                addr = format_args!("0x{address:012x}"),
                unique = inventory.unique_items.len(),
                stackable = inventory.stackable_items.len(),
                "probe hit"
            );
            blob_tx.send(inventory).ok();
            ScanOutcome::Updated
        }
        Some(CachedBlobScan::Unchanged) => ScanOutcome::Unchanged,
        None => ScanOutcome::CacheMiss,
    }
}

/// One monitor tick: re-read the blob from its remembered address, and check
/// whether the game has logged an inventory sync since the last tick.
///
/// Never falls back to a full region walk. `capture_all_blobs` does that, which
/// makes it unusable as a poll: probing at 1-2 Hz would mean walking memory at
/// 1-2 Hz for as long as the cached address stays stale. Splitting the two lets
/// the caller poll cheaply and decide for itself when a miss is worth the walk.
///
/// The marker is read first and every tick, because it is what tells the blob
/// scan it has something to look at. The scan itself runs only when `force` or
/// that marker says so; between syncs it can only ever conclude that nothing
/// moved. `None` means it was not scanned this tick, which is not the same as
/// a miss.
#[cfg(target_os = "windows")]
#[tracing::instrument(level = "debug", skip_all, fields(force = force))]
pub fn probe_tick(
    pid: u32,
    blob_tx: std::sync::mpsc::Sender<BlobInventory>,
    force: bool,
) -> (Option<ScanOutcome>, bool) {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FALSE},
        System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ},
    };

    let process = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, FALSE, pid) };
    if process == 0 {
        return (None, false);
    }
    let sync = sync_marker_is_new(windows_newest_sync_timestamp(process));
    unsafe { CloseHandle(process) };
    if !(force || sync) {
        return (None, sync);
    }
    // A minimum region size would only filter the cold walk, which this never runs.
    let Some(src) = crate::mem_regions::WindowsRegionSource::open(pid, 0, MAX_READ) else {
        return (None, sync);
    };
    (Some(probe_outcome(scan_cached_blob(&src), &blob_tx)), sync)
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn probe_tick(
    _pid: u32,
    _blob_tx: std::sync::mpsc::Sender<BlobInventory>,
    _force: bool,
) -> (Option<ScanOutcome>, bool) {
    (None, false)
}

// ─── Inventory-sync marker, read from memory rather than from EE.log ──────────
//
// Warframe composes its log lines in process memory long before they reach
// EE.log: the game buffers writes and flushes in bursts, and sampling the live
// client showed the newest in-memory line running 23 s ahead of the newest line
// on disk. Tailing the file therefore reports an inventory sync at an unknown,
// variable delay, and that delay lands on every capture gated behind it.
//
// The formatted lines are findable by content, so no pointer chain and no
// per-build offsets are involved:
//
//   19761.848 Sys [Info]: OnInventoryResults completed in 339ms
//
// Only the log text holds ` Sys [Info]: ` preceded by a seconds-since-launch
// timestamp, and once the buffer is found its address caches like
// LAST_BLOB_REGION.

/// The formatted marker. Shared with the file tail rather than re-spelled: a
/// mismatch between the two readers degrades to plain interval polling, which
/// is hard to tell from working correctly.
const SYNC_MARKER: &[u8] = crate::log_parser::INVENTORY_SYNC_MARKER.as_bytes();

/// Present on every log line, so it identifies the buffer regardless of what
/// the game happens to have logged recently.
pub(crate) const LOG_LINE_MARKER: &[u8] = b" Sys [Info]: ";

/// The candidate buffers are a few MB against several GB of readable mappings,
/// so anything larger is some other allocation that happens to quote a log line.
pub(crate) const MAX_LOG_REGION: usize = 16 * 1024 * 1024;

pub(crate) static LAST_LOG_REGION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Probes still to skip before the cold search may run again.
pub(crate) static LOG_SEARCH_BACKOFF: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whether the cold search is allowed to run on this probe. That search reads
/// every non-executable mapping under [`MAX_LOG_REGION`], hundreds of MB.
///
/// Without this, a client whose log buffers cannot be located pays a walk-sized
/// read on the monitor thread every couple of seconds for the whole session.
/// Backing off costs only latency: the marker is an optimisation, and the
/// EE.log tail reports the same syncs meanwhile.
pub(crate) fn cold_log_search_due() -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    LOG_SEARCH_BACKOFF
        .fetch_update(Relaxed, Relaxed, |left| Some(left.saturating_sub(1)))
        .is_ok_and(|left| left == 0)
}

/// Probes to sit out after a failed cold search, at the monitor's 2 s cadence.
pub(crate) const LOG_SEARCH_BACKOFF_PROBES: u64 = 30;

/// Game timestamp of the newest sync marker already reported, as `f64` bits.
static LAST_SYNC_TIMESTAMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Forget the log buffer's address and the marker baseline. Call alongside
/// [`reset_last_blob_region`] when the PID changes: the timestamps are seconds
/// since *that* client launched, so a baseline from the previous process would
/// swallow every marker the new one writes.
pub fn reset_log_region() {
    LAST_LOG_REGION.store(0, std::sync::atomic::Ordering::Relaxed);
    LAST_SYNC_TIMESTAMP.store(0, std::sync::atomic::Ordering::Relaxed);
    LOG_SEARCH_BACKOFF.store(0, std::sync::atomic::Ordering::Relaxed);
}

/// Seconds-since-launch stamp opening the line that `offset` falls inside, e.g.
/// `19761.848` from `19761.848 Sys [Info]: …`.
///
/// Both buffers hold complete formatted lines but end them differently: the
/// pending file-write buffer uses CRLF, the heap ring LF, so the search back
/// to the line start stops at either.
fn line_timestamp(chunk: &[u8], offset: usize) -> Option<f64> {
    let start = chunk[..offset]
        .iter()
        .rposition(|&byte| byte == b'\n' || byte == b'\r')
        .map_or(0, |index| index + 1);
    // `offset` lands on the space opening ` Sys [Info]: ` for one caller and
    // partway into the message for the other, so the stamp runs to whichever
    // comes first: the next space, or the marker itself.
    let line = &chunk[start..offset];
    let end = line.iter().position(|&byte| byte == b' ').unwrap_or(line.len());
    let stamp = std::str::from_utf8(&line[..end]).ok()?;
    // Reject anything that is not the timestamp shape: a bare integer or a
    // stray word would otherwise parse and then compare as a valid ordering.
    let (seconds, millis) = stamp.split_once('.')?;
    if seconds.is_empty()
        || !seconds.bytes().all(|byte| byte.is_ascii_digit())
        || millis.len() != 3
        || !millis.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    stamp.parse().ok()
}

/// True when `chunk` holds formatted log lines rather than, say, the `.rdata`
/// copy of the format string. The timestamp is what tells the two apart.
pub(crate) fn looks_like_log_buffer(chunk: &[u8]) -> bool {
    let mut from = 0;
    // A handful of probes is enough: the log text is dense with these, so a
    // buffer that fails several in a row is not it.
    for _ in 0..8 {
        let Some(hit) = memmem::find(&chunk[from..], LOG_LINE_MARKER) else { return false };
        let offset = from + hit;
        if line_timestamp(chunk, offset).is_some() {
            return true;
        }
        from = offset + LOG_LINE_MARKER.len();
    }
    false
}

/// Newest game timestamp among the sync markers in `chunk`.
///
/// Every match is examined rather than just the last, because the heap ring
/// wraps: the newest line is not necessarily the one at the highest address.
pub(crate) fn newest_sync_timestamp(chunk: &[u8]) -> Option<f64> {
    let mut newest: Option<f64> = None;
    let mut from = 0;
    while let Some(hit) = memmem::find(&chunk[from..], SYNC_MARKER) {
        let offset = from + hit;
        if let Some(stamp) = line_timestamp(chunk, offset) {
            newest = Some(newest.map_or(stamp, |best: f64| best.max(stamp)));
        }
        from = offset + SYNC_MARKER.len();
    }
    newest
}

/// Fold a freshly-observed marker timestamp into the baseline, reporting
/// whether it names a sync that has not been reported yet.
///
/// Any difference from the baseline counts, in both directions. The stamps are
/// seconds since the client launched, so the only way they run backwards is a
/// game restart, and the sync logged just after one is the login sync that
/// populates the inventory.
///
/// The first observation counts too, rather than being spent establishing a
/// baseline. A buffer that already holds markers at app start is reporting
/// history, but reporting it costs nothing, because the first capture walks
/// memory unconditionally and nothing reads the marker on that tick. Spending
/// the first observation would instead swallow the login sync after every
/// restart, since the PID change clears the baseline right before it arrives.
pub(crate) fn sync_marker_is_new(newest: Option<f64>) -> bool {
    let Some(newest) = newest else { return false };
    let previous = f64::from_bits(LAST_SYNC_TIMESTAMP.swap(newest.to_bits(), std::sync::atomic::Ordering::Relaxed));
    let is_new = newest != previous;
    if is_new {
        // Four in a 40-minute session, and the walk policy keys off them, so
        // they are logged rather than left to be inferred from the walks.
        info!(t = format_args!("{newest:.3}s"), "inventory sync marker");
    }
    is_new
}

#[cfg(target_os = "windows")]
fn windows_newest_sync_timestamp(process: windows_sys::Win32::Foundation::HANDLE) -> Option<f64> {
    use std::ffi::c_void;
    use std::mem;
    use windows_sys::Win32::System::{
        Diagnostics::Debug::ReadProcessMemory,
        Memory::{VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_NOACCESS},
    };

    // Executable pages hold the `.rdata` copy of the format string, never a
    // formatted line, so skipping them also skips the obvious false positive.
    const EXEC_MASK: u32 = 0x10 | 0x20 | 0x40 | 0x80;

    let mut buffer = Vec::new();
    let read_region = |address: usize, size: usize, buffer: &mut Vec<u8>| -> Option<usize> {
        buffer.resize(size.min(MAX_LOG_REGION), 0);
        let mut read = 0usize;
        let ok = unsafe { ReadProcessMemory(process, address as *const c_void,
            buffer.as_mut_ptr() as *mut c_void, buffer.len(), &mut read) } != 0;
        (ok && read > LOG_LINE_MARKER.len()).then_some(read)
    };
    let query = |address: usize| -> Option<MEMORY_BASIC_INFORMATION> {
        let mut mbi = unsafe { mem::zeroed::<MEMORY_BASIC_INFORMATION>() };
        let ok = unsafe { VirtualQueryEx(process, address as *const c_void, &mut mbi,
            mem::size_of::<MEMORY_BASIC_INFORMATION>()) } != 0;
        ok.then_some(mbi)
    };
    let readable = |mbi: &MEMORY_BASIC_INFORMATION| {
        mbi.State == MEM_COMMIT
            && mbi.Protect & PAGE_GUARD == 0
            && mbi.Protect & PAGE_NOACCESS == 0
            && mbi.Protect & EXEC_MASK == 0
            && mbi.RegionSize > 0
    };

    let cached = LAST_LOG_REGION.load(std::sync::atomic::Ordering::Relaxed) as usize;
    if cached != 0 {
        if let Some(mbi) = query(cached).filter(readable).filter(|mbi| mbi.BaseAddress as usize == cached) {
            if let Some(read) = read_region(cached, mbi.RegionSize, &mut buffer) {
                if looks_like_log_buffer(&buffer[..read]) {
                    return newest_sync_timestamp(&buffer[..read]);
                }
            }
        }
        // The mapping is gone or holds something else now; fall through and
        // look again rather than reporting a silent nothing from here on.
        LAST_LOG_REGION.store(0, std::sync::atomic::Ordering::Relaxed);
    }

    if !cold_log_search_due() {
        return None;
    }

    // Cold search. There are two copies of the log text: the pending
    // file-write buffer and a heap ring of recent lines. Which one is
    // further ahead depends on where the game is in its flush cycle, so both
    // are read and the newer marker wins.
    let mut newest: Option<f64> = None;
    let mut found = 0;
    let mut address = 0usize;
    while let Some(mbi) = query(address) {
        let base = mbi.BaseAddress as usize;
        let size = mbi.RegionSize;
        let Some(next) = base.checked_add(size).filter(|next| *next > address) else { break };
        address = next;
        if !readable(&mbi) || size > MAX_LOG_REGION {
            continue;
        }
        let Some(read) = read_region(base, size, &mut buffer) else { continue };
        let chunk = &buffer[..read];
        if !looks_like_log_buffer(chunk) {
            continue;
        }
        if found == 0 {
            debug!(addr = format_args!("0x{base:012x}"), kb = read / 1000, "sync-marker buffer");
            LAST_LOG_REGION.store(base as u64, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(stamp) = newest_sync_timestamp(chunk) {
            newest = Some(newest.map_or(stamp, |best: f64| best.max(stamp)));
        }
        found += 1;
        if found == 2 {
            break;
        }
    }
    if found == 0 {
        info!("no in-memory log buffer found; sync markers come from the EE.log tail only");
        LOG_SEARCH_BACKOFF.store(LOG_SEARCH_BACKOFF_PROBES, std::sync::atomic::Ordering::Relaxed);
    }
    newest
}

// ─── Continuous raw memory string dump ───────────────────────────────────────
//
// Scans every committed readable region in the Warframe process and extracts
// every run of 12+ consecutive printable ASCII bytes.  Each string is written
// to `out_file` as: `0xADDR  <string>\n`.  No needle filtering — everything.
//
// Designed to be called repeatedly from a loop: one call = one full pass.
// Returns the number of strings written this pass, or an error string.
//
// Large regions (>64 MB) are read in 64 MB chunks so the heap stays bounded.
// The caller is responsible for not holding the file lock across sleeps.

#[cfg(target_os = "windows")]
#[tracing::instrument(level = "info", skip_all)]
pub fn raw_scan_pass(out: &mut impl std::io::Write) -> Result<usize, String> {
    use std::ffi::c_void;
    use std::mem;
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::{
            Diagnostics::Debug::ReadProcessMemory,
            Memory::{VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_NOACCESS},
            Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ},
        },
    };

    const MIN_LEN:  usize = 8;
    const CHUNK:    usize = 64 * 1024 * 1024;
    const TIMEOUT:  u64   = 600; // 10 minutes — full coverage over full scan

    let pid = find_warframe_pid().ok_or("Warframe not running")?;
    let process = unsafe { OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, 0, pid) };
    if process == 0 { return Err("OpenProcess failed".into()); }

    let mut addr: usize = 0x10000;
    let mbi_size = mem::size_of::<MEMORY_BASIC_INFORMATION>();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(TIMEOUT);
    let mut count = 0usize;

    while std::time::Instant::now() < deadline {
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { mem::zeroed() };
        if unsafe { VirtualQueryEx(process, addr as *const c_void, &mut mbi, mbi_size) } == 0 { break; }
        let region_end = (mbi.BaseAddress as usize).saturating_add(mbi.RegionSize);
        if region_end <= addr { break; }
        addr = region_end;

        if mbi.State != MEM_COMMIT { continue; }
        let p = mbi.Protect;
        if p & PAGE_NOACCESS != 0 || p & PAGE_GUARD != 0 { continue; }
        // Only skip pure-execute (no read bit) — PAGE_EXECUTE_READ (0x20) is kept
        // because game DLL const-string sections use that protection.
        if p == 0x10 { continue; }

        let chunks = (mbi.RegionSize + CHUNK - 1) / CHUNK;
        for ci in 0..chunks {
            if std::time::Instant::now() >= deadline { break; }
            let off        = ci * CHUNK;
            let read_size  = CHUNK.min(mbi.RegionSize - off);
            let chunk_base = mbi.BaseAddress as usize + off;

            let mut buf = vec![0u8; read_size];
            let mut bytes_read = 0usize;
            let ok = unsafe {
                ReadProcessMemory(process, chunk_base as *const c_void,
                    buf.as_mut_ptr() as *mut c_void, read_size, &mut bytes_read)
            };
            if ok == 0 || bytes_read < MIN_LEN { continue; }

            // Extract printable ASCII runs of MIN_LEN+
            let data = &buf[..bytes_read];
            let mut run_start: Option<usize> = None;
            for (i, &b) in data.iter().enumerate() {
                let printable = b >= 0x20 && b < 0x7f;
                if printable {
                    if run_start.is_none() { run_start = Some(i); }
                } else {
                    if let Some(s) = run_start.take() {
                        let len = i - s;
                        if len >= MIN_LEN {
                            let s_str = std::str::from_utf8(&data[s..i]).unwrap_or("?");
                            let _ = writeln!(out, "0x{:012x}  {}", chunk_base + s, s_str);
                            count += 1;
                        }
                    }
                }
            }
            // flush any run that reaches end of chunk
            if let Some(s) = run_start {
                let len = bytes_read - s;
                if len >= MIN_LEN {
                    let s_str = std::str::from_utf8(&data[s..bytes_read]).unwrap_or("?");
                    let _ = writeln!(out, "0x{:012x}  {}", chunk_base + s, s_str);
                    count += 1;
                }
            }
        }
    }

    unsafe { CloseHandle(process); }
    Ok(count)
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn raw_scan_pass(_out: &mut impl std::io::Write) -> Result<usize, String> {
    Err("Only supported on Windows and Linux".into())
}

// ─── Riven validity flag scanner ──────────────────────────────────────────────
//
// GEP (gep_warframeext.dll) uses Pattern D-2 to locate a single byte in
// Warframe's .text section that acts as an open/closed flag for the riven
// reroll UI. The byte is non-zero while the screen is shown, zero when closed.
//
// Pattern D-2 (13 bytes):
//   80 3d ?? ?? ?? ?? 00  48 8b ?? ??  0f 85
//   CMP byte ptr [RIP+disp32], 0   MOV ...   JNZ ...
//
// Resolving the flag VA:
//   The CMP instruction is 7 bytes. RIP at execution = match_va + 7.
//   flag_va = (match_va + 7) + i32::from_le_bytes(bytes[2..6])

#[cfg(target_os = "windows")]
fn find_pattern_d2(data: &[u8], base_va: usize) -> Option<usize> {
    let len = data.len();
    if len < 13 { return None; }
    for i in 0..len - 13 {
        if data[i]    != 0x80 || data[i+1]  != 0x3d { continue; }
        if data[i+6]  != 0x00 { continue; }
        if data[i+7]  != 0x48 || data[i+8]  != 0x8b { continue; }
        if data[i+11] != 0x0f || data[i+12] != 0x85 { continue; }
        let disp = i32::from_le_bytes([data[i+2], data[i+3], data[i+4], data[i+5]]);
        let flag_va = (base_va + i + 7) as i64 + disp as i64;
        if flag_va > 0x10000 && flag_va < 0x7fff_ffff_ffff {
            return Some(flag_va as usize);
        }
    }
    None
}

/// Scan Warframe's executable image sections for the riven screen validity flag VA.
/// Returns the virtual address of the single byte: non-zero = screen open, 0 = closed.
/// Scans once; caller should cache the result and re-scan only on PID change.
#[cfg(target_os = "windows")]
pub fn find_riven_validity_va(pid: u32) -> Option<usize> {
    use std::ffi::c_void;
    use std::mem;
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::{
            Diagnostics::Debug::ReadProcessMemory,
            Memory::{VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT},
            Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ},
        },
    };

    let process = unsafe { OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, 0, pid) };
    if process == 0 { return None; }

    let mut result: Option<usize> = None;
    let mut addr: usize = 0x10000;
    let mbi_size = mem::size_of::<MEMORY_BASIC_INFORMATION>();
    let start_time = std::time::Instant::now();

    while start_time.elapsed().as_secs() < 60 && result.is_none() {
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { mem::zeroed() };
        if unsafe { VirtualQueryEx(process, addr as *const c_void, &mut mbi, mbi_size) } == 0 { break; }
        let region_end = (mbi.BaseAddress as usize).saturating_add(mbi.RegionSize);
        if region_end <= addr { break; }
        addr = region_end;

        // Only scan committed, executable, memory-mapped PE image regions (MEM_IMAGE = 0x1000000).
        // 0x20 = PAGE_EXECUTE_READ (normal .text), 0x40 = PAGE_EXECUTE_READWRITE (patched pages).
        let is_exec_image = mbi.State == MEM_COMMIT
            && matches!(mbi.Protect, 0x20 | 0x40)
            && mbi.Type == 0x1000000
            && mbi.RegionSize >= 13
            && mbi.RegionSize <= 64 * 1024 * 1024;

        if !is_exec_image { continue; }

        let mut buf = vec![0u8; mbi.RegionSize];
        let mut bytes_read = 0usize;
        let ok = unsafe {
            ReadProcessMemory(
                process, mbi.BaseAddress as *const c_void,
                buf.as_mut_ptr() as *mut c_void, mbi.RegionSize, &mut bytes_read,
            )
        };
        if ok == 0 || bytes_read < 13 { continue; }

        result = find_pattern_d2(&buf[..bytes_read], mbi.BaseAddress as usize);
    }

    unsafe { CloseHandle(process); }
    result
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn find_riven_validity_va(_pid: u32) -> Option<usize> { None }

#[cfg(target_os = "windows")]
fn find_warframe_pid() -> Option<u32> {
    use std::mem;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32First, Process32Next,
            PROCESSENTRY32, TH32CS_SNAPPROCESS,
        },
    };
    // CreateToolhelp32Snapshot gives process names without needing OpenProcess,
    // so EAC blocking read access on the game process doesn't prevent detection.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE { return None; }

        let mut entry: PROCESSENTRY32 = mem::zeroed();
        entry.dwSize = mem::size_of::<PROCESSENTRY32>() as u32;

        let mut found = None;
        if Process32First(snapshot, &mut entry) != 0 {
            loop {
                let name_len = entry.szExeFile.iter().position(|&b| b == 0).unwrap_or(260);
                let name = String::from_utf8_lossy(&entry.szExeFile[..name_len]).to_lowercase();
                if name.starts_with("warframe") && !name.contains("launcher") && !name.contains("companion") {
                    found = Some(entry.th32ProcessID);
                    break;
                }
                if Process32Next(snapshot, &mut entry) == 0 { break; }
            }
        }
        CloseHandle(snapshot);
        found
    }
}

#[cfg(test)]
mod seed_tests {
    use super::{enclosing_object_start, blob_seed_offsets, extract_blob_json, extract_blob_json_at};
    use std::borrow::Cow;

    #[test]
    fn enclosing_finds_outer_brace() {
        let buf = b"{\"SubscribedToEmails\":1}";
        let off = buf.windows(b"SubscribedToEmails".len())
            .position(|w| w == b"SubscribedToEmails").unwrap();
        assert_eq!(enclosing_object_start(buf, off), Some(0));
    }

    #[test]
    fn enclosing_skips_nested_braces() {
        let buf = b"{\"nested\":{\"a\":1},\"SubscribedToEmails\":1}";
        let off = buf.windows(b"SubscribedToEmails".len())
            .position(|w| w == b"SubscribedToEmails").unwrap();
        assert_eq!(enclosing_object_start(buf, off), Some(0));
    }

    #[test]
    fn enclosing_returns_none_when_no_open_brace() {
        // Stale/freed blob — the outer { was overwritten
        let buf = b"x\"SubscribedToEmails\":1}";
        assert_eq!(enclosing_object_start(buf, 0), None);
    }

    #[test]
    fn seed_offsets_with_start_marker() {
        let buf = b"{\"SubscribedToEmails\":1,\"RegularCredits\":100}";
        let (marker_off, json_open) = blob_seed_offsets(buf);
        assert_eq!(json_open, 0);
        // Both markers are present; the primary one must win over the alt-start.
        assert_eq!(marker_off, 1);
    }

    #[test]
    fn seed_offsets_with_alt_start_regular_credits() {
        // No SubscribedToEmails — falls through to RegularCredits alt-start
        let buf = b"{\"RegularCredits\":999}";
        let (marker_off, json_open) = blob_seed_offsets(buf);
        assert_eq!(json_open, 0);
        assert_eq!(marker_off, 1);
    }

    #[test]
    fn seed_offsets_stale_blob_falls_back_to_marker() {
        // The outer { was overwritten — json_open falls back to marker_off
        let buf = b"x\"SubscribedToEmails\":1}";
        let (marker_off, json_open) = blob_seed_offsets(buf);
        assert_eq!(json_open, marker_off);
    }

    /// A freed copy of the blob can sit ahead of the live one with its marker
    /// intact but its opening brace overwritten. Brace-matching backward from
    /// that copy lands on a stray `{` in binary garbage ("key must be a string
    /// at line 1 column 2"), so the stale occurrence has to be skipped in
    /// favour of the live one.
    #[test]
    fn a_stale_headless_copy_is_skipped_for_the_live_blob() {
        let mut combined = b"\x00{J>\x01\x02 garbage ".to_vec();
        combined.extend_from_slice(br#""SubscribedToEmails":0,"RegularCredits":1,"#);
        combined.extend_from_slice(b"\x03\x04 more garbage ");
        let live_at = combined.len();
        combined.extend_from_slice(br#"{"SubscribedToEmails":0,"RegularCredits":42}"#);

        let (marker_at, seed_at) = blob_seed_offsets(&combined);
        assert_eq!(seed_at, live_at, "seed is the live copy's opening brace");
        assert!(marker_at > live_at, "the marker used is the live copy's");
    }

    /// With only the headless copy in the buffer, seeding at the marker lets
    /// the parser rebuild the object head instead of parsing garbage.
    #[test]
    fn a_lone_headless_copy_seeds_at_its_marker() {
        let mut combined = b"\x00{J>\x01\x02 garbage ".to_vec();
        let marker = combined.len();
        combined.extend_from_slice(br#""SubscribedToEmails":0,"RegularCredits":42,"#);

        let (marker_at, seed_at) = blob_seed_offsets(&combined);
        assert_eq!(marker_at, marker);
        assert_eq!(seed_at, marker, "seed skips the garbage brace");
    }

    #[test]
    fn blob_json_stops_at_the_closing_brace_of_the_object() {
        // A stitched scan buffer: the blob, then the rest of the memory region
        // it happened to end in.
        let mut raw = br#"{"SubscribedToEmails":0,"DeathSquadable":false}"#.to_vec();
        let blob_len = raw.len();
        raw.extend(std::iter::repeat(0xABu8).take(1_000_000));

        let json = extract_blob_json(&raw).expect("end marker present");
        assert_eq!(json.len(), blob_len);
        assert!(serde_json::from_slice::<serde_json::Value>(&json).is_ok());
    }

    #[test]
    fn blob_json_reinstates_the_opening_brace_when_it_was_overwritten() {
        let mut raw = br#"x"SubscribedToEmails":0,"DeathSquadable":false}"#.to_vec();
        let blob_len = raw.len();
        raw.extend(std::iter::repeat(0xABu8).take(1024));

        let json = extract_blob_json(&raw).expect("end marker present");
        assert_eq!(json.len(), blob_len);
        assert_eq!(json[0], b'{');
        assert!(serde_json::from_slice::<serde_json::Value>(&json).is_ok());
    }

    #[test]
    fn blob_json_ref_borrows_in_the_common_case_and_owns_in_the_fallback() {
        let intact = br#"{"SubscribedToEmails":0,"DeathSquadable":false}"#;
        assert!(matches!(extract_blob_json_at(intact, intact.len()), Some(Cow::Borrowed(_))));

        let overwritten = br#"x"SubscribedToEmails":0,"DeathSquadable":false}"#;
        assert!(matches!(extract_blob_json_at(overwritten, overwritten.len()), Some(Cow::Owned(_))));
    }
}

// LAST_BLOB_REGION and LAST_BLOB_DIGEST are process-global statics shared by
// every test in this binary. A bare reset-then-assert is enough isolation
// when the calls in between are nanoseconds apart (no other thread gets a
// window to interleave). A test driving a real region source reads another
// process's memory between touching the digest, which is long enough for
// another test's own digest write to land in the gap. Tests that scan through
// a region source take this lock so only one of them touches the shared
// statics at a time.
#[cfg(test)]
static BLOB_DIGEST_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take the lock and clear the statics, so each test starts from a known
/// baseline instead of inheriting whatever the previous holder left behind.
///
/// The reset has to happen on entry, not on exit. The digest covers only the
/// blob's JSON, so two tests built on the same fixture produce the same digest,
/// and one of them would otherwise see its "first" sighting reported as
/// unchanged.
#[cfg(test)]
pub(crate) fn blob_digest_test_guard() -> std::sync::MutexGuard<'static, ()> {
    let guard = BLOB_DIGEST_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_last_blob_region();
    guard
}

#[cfg(test)]
mod blob_digest_tests {
    use super::{
        blob_digest_test_guard, blob_unchanged, forget_blob_digest, reset_last_blob_region,
        steady_state_notice_due,
    };

    #[test]
    fn digest_tracks_changes_and_resets() {
        let _digest_guard = blob_digest_test_guard();

        let blob = b"{\"SubscribedToEmails\":1,\"RegularCredits\":100}".to_vec();
        assert!(!blob_unchanged(&blob), "first call always reports changed");
        assert!(blob_unchanged(&blob), "identical bytes report unchanged");

        let mut mutated = blob.clone();
        mutated[0] = b'[';
        assert!(!blob_unchanged(&mutated), "a changed byte must report changed");
        assert!(blob_unchanged(&mutated), "the new bytes become the baseline");

        reset_last_blob_region();
        assert!(!blob_unchanged(&mutated), "reset forces the next call to report changed");
    }

    /// The probe stitches whole mappings, so the blob arrives with a tail of
    /// unrelated heap that a running client rewrites between probes. Digesting
    /// that tail is indistinguishable from the inventory itself changing, which
    /// reparses and re-emits a settled inventory on every probe.
    #[test]
    fn a_rewritten_tail_after_the_blob_is_not_a_change() {
        let _digest_guard = blob_digest_test_guard();

        let blob = br#"{"SubscribedToEmails":1,"DeathSquadable":false}"#;
        let mut first = blob.to_vec();
        first.extend_from_slice(b"\x00\x11garbage from a neighbouring allocation");
        let mut second = blob.to_vec();
        second.extend_from_slice(b"\xff\xfe an entirely different neighbour, and longer");

        assert!(!blob_unchanged(&first), "first sighting reports changed");
        assert!(blob_unchanged(&second), "same blob, different tail, is unchanged");
    }

    /// Probes run every couple of seconds and nearly all of them find the same
    /// bytes, so the steady-state notice has to be a transition rather than a
    /// per-probe line, otherwise it drowns out everything else in the log.
    #[test]
    fn the_steady_state_notice_fires_once_per_settle() {
        let _digest_guard = blob_digest_test_guard();

        let blob = b"{\"SubscribedToEmails\":1,\"RegularCredits\":100}".to_vec();
        assert!(!blob_unchanged(&blob), "first sighting reports changed");
        assert!(blob_unchanged(&blob), "second sighting is the steady state");
        assert!(steady_state_notice_due(), "entering the steady state logs once");
        assert!(!steady_state_notice_due(), "staying in it does not log again");

        let mut mutated = blob.clone();
        mutated[0] = b'[';
        assert!(!blob_unchanged(&mutated), "the bytes changed");
        assert!(blob_unchanged(&mutated), "and settled again");
        assert!(steady_state_notice_due(), "the next settle logs again");

        reset_last_blob_region();
        assert!(steady_state_notice_due(), "a new game process starts the cycle over");
    }

    // Unparseable bytes that persist across scan cycles must not start
    // reporting as unchanged — the skip paths read that as "already parsed
    // this", which would wedge the walk on a region that never parsed.
    #[test]
    fn forgetting_after_a_failed_parse_forces_a_retry() {
        let _digest_guard = blob_digest_test_guard();

        let garbage = b"{\"MiscItems\":[ truncated".to_vec();
        assert!(!blob_unchanged(&garbage), "first sighting reports changed");
        forget_blob_digest();
        assert!(!blob_unchanged(&garbage), "same bytes report changed again after a failed parse");
    }
}

#[cfg(test)]
mod sync_marker_tests {
    use super::{
        cold_log_search_due, looks_like_log_buffer, newest_sync_timestamp, reset_log_region,
        sync_marker_is_new, LOG_SEARCH_BACKOFF, LOG_SEARCH_BACKOFF_PROBES,
    };

    // LAST_SYNC_TIMESTAMP and LOG_SEARCH_BACKOFF are process-global, and
    // reset_log_region clears both at once, so a test calling it races any
    // other test mid-sequence. Every test here that resets takes this lock.
    static LOG_STATE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_failed_cold_search_sits_out_the_next_probes() {
        let _guard = LOG_STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_log_region();
        assert!(cold_log_search_due(), "the first search runs");

        LOG_SEARCH_BACKOFF.store(LOG_SEARCH_BACKOFF_PROBES, std::sync::atomic::Ordering::Relaxed);
        for probe in 0..LOG_SEARCH_BACKOFF_PROBES {
            assert!(!cold_log_search_due(), "probe {probe} searched during the backoff");
        }
        assert!(cold_log_search_due(), "the search resumes once the backoff expires");

        // A PID change clears it: the new client's buffers are worth looking for
        // straight away.
        LOG_SEARCH_BACKOFF.store(LOG_SEARCH_BACKOFF_PROBES, std::sync::atomic::Ordering::Relaxed);
        reset_log_region();
        assert!(cold_log_search_due());
    }

    /// The heap ring uses LF, the pending file-write buffer CRLF, and the
    /// marker has to be found in either.
    #[test]
    fn marker_is_read_from_both_buffer_shapes() {
        let ring = b"19760.121 Sys [Info]: SyncInventoryFromDB\n\
                     19761.848 Sys [Info]: OnInventoryResults completed in 339ms\n";
        assert_eq!(newest_sync_timestamp(ring), Some(19761.848));

        let pending = b"19760.121 Sys [Info]: SyncInventoryFromDB\r\n\
                        19761.848 Sys [Info]: OnInventoryResults completed in 339ms\r\n";
        assert_eq!(newest_sync_timestamp(pending), Some(19761.848));
    }

    /// The ring wraps, so the newest line is not the one at the highest
    /// address. Taking the last match would report an already-seen sync.
    #[test]
    fn newest_marker_wins_regardless_of_position() {
        let wrapped = b"19999.500 Sys [Info]: OnInventoryResults completed in 41ms\n\
                        11000.000 Sys [Info]: OnInventoryResults completed in 88ms\n";
        assert_eq!(newest_sync_timestamp(wrapped), Some(19999.500));
    }

    /// `OnInventoryResults completed in` also exists as a read-only format
    /// string, which carries no timestamp and must not be mistaken for a line
    /// the game actually wrote.
    #[test]
    fn format_string_without_a_timestamp_is_not_a_marker() {
        assert_eq!(newest_sync_timestamp(b"OnInventoryResults completed in %dms\0"), None);
        assert!(!looks_like_log_buffer(b"Sys [Info]: %s\0 Sys [Info]: %s\0"));
        assert!(looks_like_log_buffer(b"19761.848 Sys [Info]: Revive completed on KubrowPetAvatar14482\n"));
    }

    #[test]
    fn baseline_reports_only_unseen_syncs() {
        let _guard = LOG_STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_log_region();
        assert!(sync_marker_is_new(Some(100.000)), "the first marker seen is not yet reported");
        assert!(!sync_marker_is_new(Some(100.000)), "the same sync must not report twice");
        assert!(sync_marker_is_new(Some(140.250)), "a later sync reports");
        // Seconds since launch, so a stamp running backwards means the client
        // restarted; ignoring it would swallow markers until the new session
        // outran the old one.
        assert!(sync_marker_is_new(Some(12.500)), "a restarted client reports again");
        assert!(!sync_marker_is_new(None), "no marker in the buffer reports nothing");
        reset_log_region();
    }

    /// The PID change that clears the baseline lands moments before the login
    /// sync, which is the marker the gate is there to catch. Spending the
    /// first observation on a baseline would drop it on every restart.
    #[test]
    fn login_sync_after_a_restart_is_reported() {
        let _guard = LOG_STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_log_region();
        assert!(sync_marker_is_new(Some(9821.400)), "a marker from the previous client");
        reset_log_region();
        assert!(sync_marker_is_new(Some(13.036)), "the new client's login sync must report");
    }
}

#[cfg(test)]
mod credential_scan_tests {
    use super::{scan_auth_credentials, scan_steam_id};

    #[test]
    fn auth_credentials_finds_json_form() {
        let buf = br#"{"id":"594144e63ade7f2f2091c48e","Nonce":123456789}"#;
        let (account_id, nonce) = scan_auth_credentials(buf).expect("should find credentials");
        assert_eq!(account_id, "594144e63ade7f2f2091c48e");
        assert_eq!(nonce, "123456789");
    }

    #[test]
    fn auth_credentials_finds_url_encoded_form() {
        let buf = b"accountId=594144e63ade7f2f2091c48e&nonce=123456789&ct=STM";
        let (account_id, nonce) = scan_auth_credentials(buf).expect("should find credentials");
        assert_eq!(account_id, "594144e63ade7f2f2091c48e");
        assert_eq!(nonce, "123456789");
    }

    #[test]
    fn auth_credentials_none_on_no_match() {
        let buf = b"nothing interesting in here at all";
        assert_eq!(scan_auth_credentials(buf), None);
    }

    #[test]
    fn steam_id_finds_value_past_false_starts() {
        // Leading 's' bytes are false starts for the old byte-at-a-time scanner.
        let buf = b"ssssssssteamId=steamId=76561198012345678";
        let sid = scan_steam_id(buf).expect("should find steam id");
        assert_eq!(sid, "76561198012345678");
    }

    #[test]
    fn steam_id_none_on_no_match() {
        let buf = b"steamId=short";
        assert_eq!(scan_steam_id(buf), None);
    }
}

#[cfg(test)]
mod stitch_engine_tests {
    use super::{blob_digest_test_guard, stitch_blobs, BlobInventory};
    use crate::mem_regions::RecordedRegions;

    /// The parser rejects a blob under 50 KB, and one with no owned Warframe in
    /// it, so a fixture has to carry both before the engine is reached at all.
    fn make_blob(fields: &str) -> Vec<u8> {
        let filler = "x".repeat(60_000);
        format!(
            r#"{{"SubscribedToEmails":0,{fields},"XPInfo":[],"FusionPoints":0,"MiscItems":[],"Suits":[{{"ItemType":"/Lotus/Powersuits/Mag/Mag","XP":0}}],"LongGuns":[],"Melee":[],"Pistols":[],"Filler":"{filler}","DeathSquadable":false}}"#
        )
        .into_bytes()
    }

    fn run(regions: Vec<(usize, Vec<u8>)>) -> Option<BlobInventory> {
        // The engine skips a parse whose bytes match the last digest, which is
        // process-wide state shared with every other test that touches it.
        let _digest_guard = blob_digest_test_guard();
        let mut src = RecordedRegions::new(regions);
        let (tx, rx) = std::sync::mpsc::channel();
        let dir = std::env::temp_dir();
        stitch_blobs(&mut src, &dir, "test", tx, false);
        rx.try_recv().ok()
    }

    #[test]
    fn single_region_blob_is_parsed() {
        let blob = make_blob(r#""RegularCredits":12345"#);
        let inv = run(vec![(0x1000, blob)]).expect("should parse");
        assert_eq!(inv.credits, 12345);
    }

    #[test]
    fn blob_spanning_two_regions_is_stitched() {
        let blob = make_blob(r#""RegularCredits":999"#);
        let mid = blob.len() / 2;
        let r1 = (0x1000, blob[..mid].to_vec());
        let r2 = (0x1000 + mid, blob[mid..].to_vec());
        let inv = run(vec![r1, r2]).expect("should stitch and parse");
        assert_eq!(inv.credits, 999);
    }

    #[test]
    fn mission_delta_region_is_skipped() {
        let delta = b"\"InventoryChanges\":[{\"Credits\":1}]".to_vec();
        let blob  = make_blob(r#""RegularCredits":77"#);
        // Delta at a lower address; real blob follows.
        let inv = run(vec![(0x1000, delta), (0x2000, blob)]).expect("real blob should win");
        assert_eq!(inv.credits, 77);
    }

    #[test]
    fn oversized_scan_is_dropped_and_does_not_panic() {
        // First region qualifies (has start marker) but never closes;
        // second region is the real complete blob.
        let mut open = make_blob(r#""RegularCredits":1"#);
        // Strip the closing brace so it looks like a truncated blob.
        open.pop();
        // Pad it past MAX_SCAN so the engine drops it.
        open.extend(vec![b' '; super::MAX_SCAN + 1]);

        let real = make_blob(r#""RegularCredits":42"#);
        let inv = run(vec![(0x1000, open), (0x9000_0000, real)]).expect("real blob should parse");
        assert_eq!(inv.credits, 42);
    }
}


