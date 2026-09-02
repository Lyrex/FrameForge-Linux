use crate::credentials::credential_store_available;

/// What the running platform can actually do, so the UI hides controls that
/// would only ever return an error instead of letting the user discover the
/// limitation by clicking.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlatformCapabilities {
    persistent_credentials: bool,
}

#[tauri::command]
pub(crate) async fn get_platform_capabilities() -> PlatformCapabilities {
    PlatformCapabilities {
        persistent_credentials: credential_store_available().await,
    }
}

#[tauri::command]
pub(crate) fn get_system_locale() -> String {
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
