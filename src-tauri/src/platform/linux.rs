//! Linux platform implementation.

/// The keyring calls block for as long as the user takes to answer an unlock
/// prompt, and Tauri runs a sync command on the main thread, so the commands in
/// [`crate::credentials`] stay async and hand these to `spawn_blocking`. Call
/// them from a blocking context only.
pub fn save_credentials(target: &str, email: &str, token: &str) -> Result<(), String> {
    crate::credentials::secret_save(target, email, token)
}

pub fn load_credentials(target: &str) -> Result<Option<(String, String)>, String> {
    crate::credentials::secret_load(target)
}

pub fn delete_credentials(target: &str) -> Result<(), String> {
    crate::credentials::secret_delete(target)
}

pub fn find_warframe_pid() -> Option<u32> {
    crate::memory_scanner_linux::find_warframe_pid()
}

pub fn get_system_locale() -> String {
    // POSIX locales look like "de_DE.UTF-8" or "de_DE@euro"; the frontend
    // feeds this to Intl, which wants a BCP-47 tag like "de-DE". LC_TIME
    // outranks LANG because the locale only ever picks the clock format.
    let posix = ["LC_ALL", "LC_TIME", "LANG"].iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|s| !s.is_empty());
    if let Some(lang) = posix {
        let tag = lang.split(['.', '@']).next().unwrap_or("").replace('_', "-");
        if !tag.is_empty() && tag != "C" && tag != "POSIX" {
            return tag;
        }
    }
    "en-US".to_string()
}

pub fn get_warframe_window_rect() -> Result<[i32; 4], String> {
    crate::ocr::warframe_window_rect()
}
