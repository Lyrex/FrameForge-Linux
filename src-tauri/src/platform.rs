use crate::credentials::credential_store_available;

/// What the running platform can actually do, so the UI hides controls that
/// would only ever return an error instead of letting the user discover the
/// limitation by clicking.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlatformCapabilities {
    linux: bool,
    ocr: bool,
    persistent_credentials: bool,
}

#[tauri::command]
pub(crate) async fn get_platform_capabilities() -> PlatformCapabilities {
    PlatformCapabilities {
        linux: cfg!(target_os = "linux"),
        // Linux grabs the game window over X11 and reads it with Tesseract, so
        // every screen-reading feature is available there too.
        ocr: cfg!(any(target_os = "windows", target_os = "linux")),
        persistent_credentials: credential_store_available().await,
    }
}

#[tauri::command]
pub(crate) fn get_system_locale() -> String {
    #[cfg(target_os = "windows")]
    {
        let mut buf = [0u16; 85]; // LOCALE_NAME_MAX_LENGTH
        let len = unsafe { windows_sys::Win32::Globalization::GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
        if len > 1 {
            return String::from_utf16_lossy(&buf[..(len as usize - 1)]);
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
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
    }
    "en-US".to_string()
}
