use tauri::{Emitter, State};
use tauri_plugin_dialog::DialogExt;
use crate::app_state::AppState;
use crate::db::{QuantityChange, SnapshotPoint, TrackedItem};
use crate::db;

// ─── Change log ───────────────────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn get_change_log(state: State<AppState>, limit: i64) -> Result<Vec<QuantityChange>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::get_quantity_changes(&conn, limit).map_err(|e| e.to_string())
}

// ─── Tracked items / snapshots ───────────────────────────────────────────────

#[tauri::command]
pub(crate) fn get_tracked_items(state: State<AppState>) -> Result<Vec<TrackedItem>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::get_tracked_items(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn add_tracked_item(state: State<AppState>, unique_name: String, display_name: String) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::add_tracked_item(&conn, &unique_name, &display_name).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn remove_tracked_item(state: State<AppState>, unique_name: String) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::remove_tracked_item(&conn, &unique_name).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn get_item_snapshots(state: State<AppState>, unique_name: String, days: Option<u32>) -> Result<Vec<SnapshotPoint>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::get_snapshots(&conn, &unique_name, days).map_err(|e| e.to_string())
}

// ─── Stats export / import ───────────────────────────────────────────────────

/// `None` means the user dismissed the file dialog, which is not an error.
///
/// Both commands are `async` so Tauri runs them off the main thread: the
/// blocking dialog would deadlock the event loop there.
#[tauri::command]
pub(crate) async fn export_stats(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let doc = {
        let conn = state.conn.lock().map_err(|e| e.to_string())?;
        db::export_document(&conn).map_err(|e| e.to_string())?
    };
    let json = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;

    let Some(target) = app
        .dialog()
        .file()
        .add_filter("FrameForge export", &["json"])
        .set_file_name("frameforge-stats.json")
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = target.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(Some(path.display().to_string()))
}

#[tauri::command]
pub(crate) async fn import_stats(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<db::ImportCounts>, String> {
    let Some(source) = app
        .dialog()
        .file()
        .add_filter("FrameForge export", &["json"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = source.into_path().map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let doc = db::parse_export(&text)?;

    let counts = {
        let mut conn = state.conn.lock().map_err(|e| e.to_string())?;
        db::import_document(&mut conn, &doc).map_err(|e| e.to_string())?
    };
    app.emit("stats-changed", ()).ok();
    Ok(Some(counts))
}
