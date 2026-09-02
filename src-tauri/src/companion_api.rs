use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tauri::State;
use crate::app_state::AppState;
use crate::cache::atomic_write;
use crate::inventory_state::{InventoryStateCache, CachedItem, ApiModCopy, load_inventory_state_cache};
use crate::rivens::truncate_chars;
use crate::{cache, memory_scanner};

/// Scan all Warframe memory regions for the session credentials (accountId + nonce).
/// These are placed in memory by the game itself after login — we never handle passwords.
#[tauri::command]
pub(crate) async fn scan_warframe_credentials() -> Result<(String, String, String), String> {
    tauri::async_runtime::spawn_blocking(scan_warframe_credentials_sync)
        .await
        .map_err(|e| e.to_string())?
}

fn scan_warframe_credentials_sync() -> Result<(String, String, String), String> {
    memory_scanner::scan_warframe_credentials_process()
}

/// Scan Warframe memory for API request URLs — reveals exact endpoints the game uses.
#[cfg(target_os = "windows")]
#[tauri::command]
pub(crate) async fn scan_warframe_api_urls() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::{
                Diagnostics::Debug::ReadProcessMemory,
                Memory::{VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_NOACCESS},
                Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ},
            },
        };
        use std::ffi::c_void;
        use std::mem;

        let pid = memory_scanner::find_warframe_pid_pub()
            .ok_or("Warframe not running".to_string())?;

        let mut found = Vec::new();
        unsafe {
            let process = OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, 0, pid);
            if process == 0 { return Err("Cannot open process".into()); }

            let mut address: usize = 0x10000;
            let mbi_size = mem::size_of::<MEMORY_BASIC_INFORMATION>();

            loop {
                let mut mbi: MEMORY_BASIC_INFORMATION = mem::zeroed();
                if VirtualQueryEx(process, address as *const c_void, &mut mbi, mbi_size) == 0 { break; }
                let region_end = (mbi.BaseAddress as usize).saturating_add(mbi.RegionSize);
                if region_end <= address { break; }
                address = region_end;

                if mbi.State != MEM_COMMIT { continue; }
                let p = mbi.Protect;
                if p & PAGE_NOACCESS != 0 || p & PAGE_GUARD != 0 { continue; }
                if p == 0x10 || p == 0x20 { continue; }
                if mbi.RegionSize > 64 * 1024 * 1024 { continue; }

                let mut buffer = vec![0u8; mbi.RegionSize];
                let mut bytes_read: usize = 0;
                let ok = ReadProcessMemory(
                    process, mbi.BaseAddress as *const c_void,
                    buffer.as_mut_ptr() as *mut c_void, mbi.RegionSize, &mut bytes_read,
                );
                if ok == 0 || bytes_read == 0 { continue; }

                let data = &buffer[..bytes_read];
                // Search for various Warframe API patterns
                let needles: &[&[u8]] = &[
                    b"/API/PHP/", b"inventory.php", b"login.php",
                    b"warframe.com/A", b"Nonce", b"accountId",
                ];
                for needle in needles {
                    let mut i = 0;
                    while i + needle.len() < data.len() {
                        if &data[i..i + needle.len()] == *needle {
                            let start = i.saturating_sub(30);
                            let end = (i + 100).min(data.len());
                            let ctx: String = data[start..end].iter()
                                .map(|&b| if b >= 0x20 && b < 0x7f { b as char } else { ' ' })
                                .collect();
                            let trimmed = ctx.split_whitespace().collect::<Vec<_>>().join(" ");
                            let label = format!("[{}] {}", std::str::from_utf8(needle).unwrap_or("?"), trimmed);
                            if !found.iter().any(|s: &String| s.contains(&trimmed[..trimmed.len().min(30)])) {
                                found.push(label);
                            }
                            if found.len() >= 40 { break; }
                        }
                        i += 1;
                    }
                }
                if found.len() >= 20 { break; }
            }
            CloseHandle(process);
        }
        Ok(found)
    }).await.map_err(|e| e.to_string())?
}

#[cfg(target_os = "linux")]
#[tauri::command]
pub(crate) async fn scan_warframe_api_urls() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(memory_scanner::scan_api_url_strings)
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
#[tauri::command]
pub(crate) async fn scan_warframe_api_urls() -> Result<Vec<String>, String> {
    Err("Only supported on Windows and Linux".into())
}

/// Persist mastery data (unique_name → rank 0-30) from the Companion API or any other source.
/// Merges into each item's entry in inventory_state_cache.json; higher rank always wins.
#[tauri::command]
pub(crate) fn save_mastery_data(
    state: tauri::State<'_, AppState>,
    data: HashMap<String, u32>,
) -> Result<(), String> {
    if data.is_empty() { return Ok(()); }
    let path = state.inventory_state_cache_path.clone();
    let mut cache: InventoryStateCache = std::fs::read_to_string(&path).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    for (k, v) in &data {
        let entry = cache.items.entry(k.clone()).or_insert_with(|| CachedItem {
            unique_name: k.clone(), ..Default::default()
        });
        if *v > entry.mastery_rank { entry.mastery_rank = *v; }
    }
    serde_json::to_string(&cache).map_err(|e| e.to_string())
        .and_then(|json| atomic_write(&path, json.as_bytes()).map_err(|e| e.to_string()))
}

/// Return statement for get_saved_inventory — camelCase so TypeScript receives it without conversion.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavedInventory {
    api_quantities: HashMap<String, i64>,
    api_mod_copies: Vec<ApiModCopy>,
    consumed_suits: Vec<String>,
}

/// Returns all owned riven mods (veiled and revealed) from the persisted inventory cache.
/// Runs in a blocking thread so the large inventory JSON deserialization doesn't stall the UI.
#[tauri::command]
pub(crate) async fn get_rivens(state: tauri::State<'_, AppState>) -> Result<Vec<memory_scanner::BlobRivenEntry>, String> {
    let path = state.inventory_state_cache_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        load_inventory_state_cache(&path).rivens
    })
    .await
    .map_err(|e| e.to_string())
}

/// Called once on startup so the frontend can restore state without waiting for Warframe to run.
#[tauri::command]
pub(crate) fn get_saved_inventory(state: tauri::State<'_, AppState>) -> SavedInventory {
    let cache = load_inventory_state_cache(&state.inventory_state_cache_path);
    SavedInventory {
        api_quantities: state.api_quantities_cache.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        api_mod_copies: state.api_mod_copies_cache.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        consumed_suits: cache.consumed_suits(),
    }
}

/// Persist Companion API quantities, mod copies, and subsumed warframes.
/// Updates AppState in-memory (scanner picks them up on next write) and writes immediately to disk.
#[tauri::command]
pub(crate) fn save_api_inventory(
    state: tauri::State<'_, AppState>,
    api_quantities: HashMap<String, i64>,
    api_mod_copies: Vec<ApiModCopy>,
    consumed_suits: Vec<String>,
) -> Result<(), String> {
    // Update in-memory cache so the scan loop picks these up without a file read.
    *state.api_quantities_cache.lock().unwrap_or_else(|e| e.into_inner()) = api_quantities.clone();
    *state.api_mod_copies_cache.lock().unwrap_or_else(|e| e.into_inner()) = api_mod_copies.clone();

    let path = state.inventory_state_cache_path.clone();
    let mut cache: InventoryStateCache = std::fs::read_to_string(&path).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    // API quantities: only write items not already present from the scanner.
    // Scanner data is authoritative — API only fills gaps for items not yet scanned.
    for (k, qty) in &api_quantities {
        let entry = cache.items.entry(k.clone()).or_insert_with(|| CachedItem {
            unique_name: k.clone(), ..Default::default()
        });
        if entry.amount == 0 { entry.amount = *qty; }
    }
    // API mod copies: same — only fill mods the scanner hasn't recorded.
    for mc in &api_mod_copies {
        let entry = cache.items.entry(mc.unique_name.clone()).or_insert_with(|| CachedItem {
            unique_name: mc.unique_name.clone(), ..Default::default()
        });
        if entry.mod_ranks.is_none() {
            let ranks = entry.mod_ranks.get_or_insert_with(HashMap::new);
            let rank_key = mc.rank.map(|r| r.to_string()).unwrap_or_else(|| "0".to_string());
            *ranks.entry(rank_key).or_insert(0) = mc.count;
            entry.amount = ranks.values().sum();
        }
    }
    for suit in consumed_suits {
        cache.items.entry(suit.clone()).or_insert_with(|| CachedItem {
            unique_name: suit.clone(), ..Default::default()
        }).subsumed = true;
    }
    serde_json::to_string(&cache).map_err(|e| e.to_string())
        .and_then(|json| atomic_write(&path, json.as_bytes()).map_err(|e| e.to_string()))
}

/// Login to Warframe API with email + password (same flow as mobile companion app).
/// Password is hashed with Whirlpool before sending — never sent in plaintext.
/// Returns (accountId, nonce) for subsequent API calls.
#[tauri::command]
pub(crate) async fn warframe_login(email: String, password: String) -> Result<(String, String), String> {
    use whirlpool::{Whirlpool, Digest};
    let hash = format!("{:x}", Whirlpool::digest(password.as_bytes()));
    let now = cache::now_unix();

    // Try multiple endpoint + body format variants.
    // mobile=true prevents clobbering an active game session.
    // date=9999999999999999 is required by some versions of the API (device-ID placeholder).
    let form_body = format!(
        "email={}&password={}&time={}&mobile=true&appVersion=live&date=9999999999999999",
        urlencoding(&email), hash, now
    );
    let json_body = format!(
        r#"{{"email":"{}","password":"{}","time":{},"date":9999999999999999,"mobile":true,"appVersion":"live"}}"#,
        email.replace('"', "\\\""), hash, now
    );

    let candidates: &[(&str, &str, &str)] = &[
        ("https://api.warframe.com/api/login.php",     "application/json",                  &json_body),
        ("https://mobile.warframe.com/api/login.php",  "application/json",                  &json_body),
        ("https://api.warframe.com/api/login.php",     "application/x-www-form-urlencoded", &form_body),
        ("https://mobile.warframe.com/api/login.php",  "application/x-www-form-urlencoded", &form_body),
    ];

    let mut errors: Vec<String> = Vec::new();
    for (url, ct, body) in candidates {
        let result = ureq::post(url)
            .set("X-Titanium-Id", "9bbd1ddd-f7f2-402d-9777-873f458cb50c")
            .set("X-Requested-With", "XMLHttpRequest")
            .set("Content-Type", ct)
            .set("User-Agent", "Dalvik/2.1.0 (Linux; U; Android 8.1.0)")
            .send_string(body);
        match result {
            Ok(resp) => {
                let text = resp.into_string().unwrap_or_default();
                let json: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => { errors.push(format!("{}: non-JSON: {}", url, truncate_chars(&text, 200))); continue; }
                };
                let id    = json["id"].as_str().unwrap_or("").to_string();
                let nonce = json["Nonce"].to_string().trim_matches('"').to_string();
                if !id.is_empty() && nonce != "null" {
                    return Ok((id, nonce));
                }
                errors.push(format!("{}: rejected: {}", url, truncate_chars(&text, 300)));
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                errors.push(format!("{}: HTTP {}: {}", url, code, truncate_chars(&body, 200)));
            }
            Err(e) => { errors.push(format!("{}: {}", url, e)); }
        }
    }
    Err(format!("All login endpoints failed:\n{}", errors.join("\n")))
}

fn urlencoding(s: &str) -> String {
    s.chars().flat_map(|c| match c {
        'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => vec![c],
        '@' => vec!['%', '4', '0'],
        _ => format!("%{:02X}", c as u8).chars().collect(),
    }).collect()
}

/// Fetch the player's full inventory from the Warframe companion API.
#[tauri::command]
pub(crate) async fn fetch_warframe_inventory(account_id: String, nonce: String, steam_id: String, state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let log_enabled = state.api_log_enabled.load(Ordering::SeqCst);
    let log_dir     = state.api_log_dir.clone();

    // Base URL uses lowercase /api/ (not /API/PHP/). ct=STM for Steam platform.
    let endpoints = [
        "https://api.warframe.com/api/inventory.php",
        "https://api.warframe.com/api/profile.php",
    ];
    let body = format!(
        "accountId={}&nonce={}&ct=STM{}&SteamOnly=1",
        account_id, nonce,
        if !steam_id.is_empty() { format!("&steamId={}", steam_id) } else { String::new() }
    );
    let headers = [
        ("Content-Type", "application/x-www-form-urlencoded"),
        ("User-Agent", "Mozilla/5.0"),
        ("Accept", "application/json"),
        ("Host", "api.warframe.com"),
    ];

    let mut last_err = String::new();
    for url in &endpoints {
        let mut req = ureq::post(url);
        for (k, v) in &headers { req = req.set(k, v); }
        match req.send_string(&body) {
            Ok(resp) => {
                let status = resp.status();
                let text = resp.into_string().unwrap_or_default();
                if log_enabled {
                    let endpoint_name = url.split('/').last().unwrap_or("response");
                    let ts   = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();
                    let path = log_dir.join(format!("{}_{}.json", ts, endpoint_name));
                    let _ = std::fs::write(&path, &text);
                }
                if status == 200 {
                    return serde_json::from_str(&text)
                        .map_err(|e| format!("Parse failed: {} — body: {}", e, truncate_chars(&text, 200)));
                }
                last_err = format!("HTTP {} from {}: {}", status, url, truncate_chars(&text, 100));
            }
            Err(e) => { last_err = format!("Request to {} failed: {}", url, e); }
        }
    }
    Err(last_err)
}
