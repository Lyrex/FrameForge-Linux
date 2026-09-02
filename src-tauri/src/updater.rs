//! Checking GitHub Releases for a newer signed build.
//!
//! Nothing is downloaded here; this only reports that a newer release exists.
//! A check the user asked for reports why it failed, while the one at launch
//! stays silent — an unreachable or malformed manifest is not something they
//! asked about.

use std::sync::Mutex;

use serde::Serialize;
use tauri::utils::{config::BundleType, platform::bundle_type};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::UpdaterExt;

pub const UPDATE_AVAILABLE_EVENT: &str = "update-available";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAvailable {
    pub version: String,
    pub current_version: String,
    pub notes: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub update: Option<UpdateAvailable>,
    pub self_updates: bool,
}

/// The launch check's result, kept for the frontend to collect. The check
/// starts during setup, before the webview can subscribe, and an event with no
/// listener is dropped.
#[derive(Default)]
pub struct LaunchCheck(Mutex<Option<UpdateAvailable>>);

/// The plugin picks its installer from the bundle type stamped into the binary
/// at bundle time, so a `.deb` or `.rpm` install routes the downloaded AppImage
/// into a package install that rejects those bytes. The bundler signs no `.deb`
/// or `.rpm` either, so there is nothing to offer them.
fn applies_own_updates() -> bool {
    !matches!(bundle_type(), Some(BundleType::Deb | BundleType::Rpm))
}

async fn find_update(app: &AppHandle) -> Result<Option<UpdateAvailable>, String> {
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    Ok(update.map(|update| UpdateAvailable {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: update.body.clone(),
    }))
}

#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<UpdateStatus, String> {
    if !applies_own_updates() {
        return Ok(UpdateStatus { update: None, self_updates: false });
    }
    Ok(UpdateStatus { update: find_update(&app).await?, self_updates: true })
}

/// `None` while the launch check is still running, as well as when it found
/// nothing.
#[tauri::command]
pub fn pending_update(state: State<'_, LaunchCheck>) -> Option<UpdateAvailable> {
    state.0.lock().expect("the launch-check lock is only held for a clone").clone()
}

/// Spawned rather than awaited, so a hanging endpoint delays nothing the user
/// is waiting for.
pub fn spawn_launch_check(app: AppHandle) {
    if !applies_own_updates() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let Ok(Some(update)) = find_update(&app).await else {
            return;
        };
        *app.state::<LaunchCheck>()
            .0
            .lock()
            .expect("the launch-check lock is only held for a store") = Some(update.clone());
        let _ = app.emit(UPDATE_AVAILABLE_EVENT, update);
    });
}

/// The Linux install path leaves the old binary running until the user says to
/// restart; the Windows installer restarts the app itself, so this is never
/// reached there.
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    app.restart();
}
