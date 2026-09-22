use tauri::State;

use crate::app_state::AppState;
use crate::db;
use crate::db::{QuantityChange, SnapshotPoint, TrackedItem};

#[tauri::command]
pub(crate) fn get_change_log(state: State<AppState>, limit: i64) -> Result<Vec<QuantityChange>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let ignored_paths: std::collections::HashSet<&str> = state.corrections.iter()
        .filter(|(_, correction)| correction.category.as_deref() == Some("Ignored"))
        .map(|(path, _)| path.as_str())
        .collect();
    db::get_quantity_changes(&conn, limit)
        .map(|changes| changes.into_iter()
            .filter(|change| !ignored_paths.contains(change.unique_name.as_str()))
            .collect())
        .map_err(|e| e.to_string())
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
