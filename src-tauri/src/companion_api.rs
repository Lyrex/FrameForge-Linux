use crate::app_state::AppState;
use crate::inventory_state::load_inventory_state_cache;
use crate::memory_scanner;

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
