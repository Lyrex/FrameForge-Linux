//! Checking GitHub Releases for a newer signed build.
//!
//! No update, an unreachable or malformed manifest, an artifact whose
//! signature does not verify — none of it is anything the user can act on, so
//! all of it collapses to `None` rather than surfacing. Nothing is downloaded
//! here either way; this only reports that a newer release exists.

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

pub const UPDATE_AVAILABLE_EVENT: &str = "update-available";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAvailable {
    pub version: String,
    pub current_version: String,
    pub notes: Option<String>,
}

async fn find_update(app: &AppHandle) -> Option<UpdateAvailable> {
    let update = app.updater().ok()?.check().await.ok()??;
    Some(UpdateAvailable {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: update.body.clone(),
    })
}

#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Option<UpdateAvailable> {
    find_update(&app).await
}

/// Spawned rather than awaited, so a hanging endpoint delays nothing the user
/// is waiting for.
pub fn spawn_launch_check(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Some(update) = find_update(&app).await {
            let _ = app.emit(UPDATE_AVAILABLE_EVENT, update);
        }
    });
}

/// The Linux install path leaves the old binary running until the user says to
/// restart; the Windows installer restarts the app itself, so this is never
/// reached there.
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    app.restart();
}
