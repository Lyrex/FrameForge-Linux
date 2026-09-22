//! Linux platform implementation (stubs for future development).

use super::*;

pub fn find_warframe_pid() -> Option<u32> {
    // TODO: implement via /proc or process enumeration
    None
}

pub fn get_system_locale() -> String {
    // Try LANG environment variable, fall back to en-US
    std::env::var("LANG")
        .ok()
        .and_then(|lang| lang.split('.').next().map(|s| s.replace('_', "-")))
        .unwrap_or_else(|| "en-US".to_string())
}
