//! WFM credential management via the platform abstraction layer.

use crate::platform::{CredentialStore, Platform};

const WFM_TARGET: &str = "FrameForge_WFM";

#[tauri::command]
pub(crate) fn wfm_save_credentials(email: String, token: String) -> Result<(), String> {
    Platform::save_credentials(WFM_TARGET, &email, &token)
}

#[tauri::command]
pub(crate) fn wfm_load_credentials() -> Result<Option<(String, String)>, String> {
    Platform::load_credentials(WFM_TARGET)
}

#[tauri::command]
pub(crate) async fn wfm_delete_credentials() -> Result<(), String> {
    Platform::delete_credentials(WFM_TARGET)
}
