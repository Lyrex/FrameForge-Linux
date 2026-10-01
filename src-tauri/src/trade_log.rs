use tauri::{Emitter, State};

use crate::app_state::AppState;
use crate::db::{self, Trade};
use crate::events;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AddTradeParams {
    with_player: String,
    direction: String,
    item_name: String,
    item_url: String,
    quantity: i64,
    platinum: i64,
    source: String,
    notes: String,
    session_id: Option<String>,
    trade_type: Option<String>,
    timestamp: Option<String>,
}

// ─── Trade log ────────────────────────────────────────────────────────────────

#[tracing::instrument(level = "debug", skip_all)]
#[tauri::command]
pub(crate) fn get_trades(state: State<AppState>) -> Result<Vec<Trade>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::get_trades(&conn).map_err(|e| e.to_string())
}

#[tracing::instrument(level = "debug", skip_all)]
#[tauri::command]
pub(crate) fn add_trade(
    app: tauri::AppHandle,
    state: State<AppState>,
    params: AddTradeParams,
) -> Result<i64, String> {
    let trade = Trade {
        id: 0,
        uid: String::new(),
        timestamp: params.timestamp.unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
        with_player: params.with_player,
        direction: params.direction,
        item_name: params.item_name,
        item_url: params.item_url,
        quantity: params.quantity,
        platinum: params.platinum,
        source: params.source,
        notes: params.notes,
        session_id: params.session_id.unwrap_or_default(),
        trade_type: params.trade_type.unwrap_or_default(),
    };
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let trade_id = db::add_trade(&conn, &trade).map_err(|e| e.to_string())?;
    tracing::info!(
        trade_id,
        session_id = %trade.session_id,
        direction = %trade.direction,
        item_name = %trade.item_name,
        "trade saved"
    );
    app.emit(events::STATS_CHANGED, ()).ok();
    Ok(trade_id)
}
