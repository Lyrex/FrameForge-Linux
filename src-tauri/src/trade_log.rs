use std::collections::HashMap;
use tauri::{Emitter, State};
use crate::app_state::AppState;
use crate::db::Trade;
use crate::db;

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

// ── Trade dialog parser ───────────────────────────────────────────────────────

pub(crate) struct ParsedTrade {
    pub(crate) with_player: String,
    pub(crate) trade_type: String,
    pub(crate) offered_items: Vec<(String, i64)>,
    pub(crate) offered_plat: i64,
    pub(crate) received_items: Vec<(String, i64)>,
    pub(crate) received_plat: i64,
    pub(crate) session_id: String,
    pub(crate) timestamp: String,
}

/// Clean a single item line from a trade dialog:
/// strips Warframe PUA rank-dot characters and normalises mod rank suffixes.
fn clean_trade_item(raw: &str) -> String {
    let raw = raw.trim();
    let filled = raw.chars().filter(|&c| c == '\u{E114}').count();
    let total  = raw.chars().filter(|&c| c == '\u{E114}' || c == '\u{E112}').count();
    if total > 0 {
        let base: String = raw.chars().take_while(|&c| c != '\u{E114}' && c != '\u{E112}').collect();
        let base = base.trim();
        return if filled == 0 { format!("{} (R0)", base) } else { format!("{} (R{})", base, filled) };
    }
    if let Some(p) = raw.find(" (") {
        let inside = &raw[p + 2..];
        if let Some(r) = inside.to_lowercase().find("rank ") {
            let rank_n = inside[r + 5..].trim_end_matches(')').trim();
            return format!("{} (R{})", &raw[..p], rank_n);
        }
        return raw[..p].trim().to_string();
    }
    raw.to_string()
}

/// Parse all items from one section of a trade dialog (offered or received).
/// Handles both repeated-line stacking and "Item x N" inline quantities.
fn extract_trade_items(section: &str) -> Vec<(String, i64)> {
    let mut order: Vec<String> = Vec::new();
    let mut counts: HashMap<String, i64> = HashMap::new();
    for line in section.lines() {
        let raw = line.trim();
        if raw.is_empty() || raw.to_lowercase().contains("platinum") { continue; }
        let (raw_name, qty) = if let Some(x_pos) = raw.rfind(" x ") {
            let qty_part = raw[x_pos + 3..].trim();
            if let Ok(n) = qty_part.parse::<i64>() { (&raw[..x_pos], n) } else { (raw, 1i64) }
        } else {
            (raw, 1i64)
        };
        let name = clean_trade_item(raw_name);
        if !name.is_empty() {
            if !counts.contains_key(&name) { order.push(name.clone()); }
            *counts.entry(name).or_insert(0) += qty;
        }
    }
    order.into_iter().map(|k| { let q = counts[&k]; (k, q) }).collect()
}

/// Parse the full trade confirmation dialog from EE.log.
/// Returns None if the dialog doesn't contain the expected markers.
pub(crate) fn parse_trade_dialog(raw: &str) -> Option<ParsedTrade> {
    // The game sometimes appends a private-use-area glyph (U+E000, an in-game
    // status icon) to the player name with no space before it.
    let with_player = raw.find("will receive from ")
        .and_then(|i| { let a = &raw[i + 18..]; a.find(" the following").map(|j| {
            a[..j].trim().trim_end_matches(|c: char| ('\u{E000}'..='\u{F8FF}').contains(&c)).trim().to_string()
        }) })?;
    let offered_raw = raw.find("You are offering:")
        .and_then(|i| { let a = &raw[i + 17..]; a.find("and will receive from").map(|j| a[..j].trim().to_string()) })
        .unwrap_or_default();
    let received_raw = raw.find("the following:")
        .and_then(|i| { let a = &raw[i + 14..]; a.find(", title=").map(|j| a[..j].trim().to_string()) })
        .unwrap_or_default();

    let parse_plat = |s: &str| -> i64 {
        s.find("Platinum x ")
            .and_then(|i| s[i + 11..].split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    };

    let offered_plat  = parse_plat(&offered_raw);
    let received_plat = parse_plat(&received_raw);
    let offered_items  = extract_trade_items(&offered_raw);
    let received_items = extract_trade_items(&received_raw);

    if offered_items.is_empty() && received_items.is_empty() && offered_plat == 0 && received_plat == 0 {
        return None;
    }

    let trade_type = if offered_plat > 0 { "purchase" } else if received_plat > 0 { "sale" } else { "trade" };
    let now = chrono::Utc::now();

    Some(ParsedTrade {
        with_player,
        trade_type: trade_type.to_string(),
        offered_items,
        offered_plat,
        received_items,
        received_plat,
        session_id: now.format("%Y%m%dT%H%M%S%3f").to_string(),
        timestamp: now.to_rfc3339(),
    })
}

const MAX_TRADE_BUFFER_BYTES: usize = 256 * 1024;
const TRADE_SUCCESS_MARKER: &str = "the trade was successful";

/// Feeds one EE.log read into `trade_buffer`. Once the success line arrives,
/// parses the last confirmation dialog before it and clears the buffer. The
/// flag reports the success line even when no dialog parsed.
///
/// The tailer can wake while the game is still writing the multi-line dialog,
/// so the dialog, and even the success line, can span several reads.
pub(crate) fn collect_trade_completion(
    buf: &str,
    trade_buffer: &mut String,
) -> (bool, Option<ParsedTrade>) {
    trade_buffer.push_str(buf);
    if trade_buffer.len() > MAX_TRADE_BUFFER_BYTES {
        let start = trade_buffer.ceil_char_boundary(trade_buffer.len() - MAX_TRADE_BUFFER_BYTES);
        trade_buffer.drain(..start);
    }

    let scan_start = trade_buffer
        .floor_char_boundary(trade_buffer.len().saturating_sub(buf.len() + TRADE_SUCCESS_MARKER.len()));
    if !trade_buffer[scan_start..].to_ascii_lowercase().contains(TRADE_SUCCESS_MARKER) {
        return (false, None);
    }

    // ASCII lowercasing keeps byte offsets, so an index into `lower` is valid
    // in `trade_buffer` too.
    let lower = trade_buffer.to_ascii_lowercase();
    let trade = lower
        .rfind("dialog::createokcancel")
        .and_then(|start| parse_trade_dialog(&trade_buffer[start..]));
    trade_buffer.clear();
    (true, trade)
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
    app.emit("stats-changed", ()).ok();
    Ok(trade_id)
}

#[cfg(test)]
mod trade_dialog_tests {
    use super::*;

    /// Real (player-redacted) dialog text captured from EE.log for a trade where
    /// the local player sold 4 Sevagoth Prime blueprints for 33 platinum. The
    /// player name is followed by a private-use-area glyph (U+E000) with no
    /// preceding space, and item lines are prefixed with a bare '\r'.
    #[test]
    fn trade_sale_for_platinum_extracts_received_plat_and_offered_items() {
        let raw = "1037.335 Script [Info]: Dialog.lua: Dialog::CreateOkCancel(description=Are you sure you want to accept this trade? You are offering:\n\rSevagoth Prime Chassis Blueprint\n\rSevagoth Prime Neuroptics Blueprint\n\rSevagoth Prime Systems Blueprint\n\rSevagoth Prime Blueprint\r\n\r\nand will receive from Winter.Mine\u{E000} the following:\n\rPlatinum x 33, title= leftItem=/Menu/Confirm_Item_Ok, rightItem=/Menu/Confirm_Item_Cancel)\n";

        let parsed = parse_trade_dialog(raw).expect("dialog should parse");
        assert_eq!(parsed.with_player, "Winter.Mine");
        assert_eq!(parsed.trade_type, "sale");
        assert_eq!(parsed.received_plat, 33);
        assert_eq!(parsed.offered_plat, 0);
        assert!(parsed.received_items.is_empty());
        assert_eq!(
            parsed.offered_items,
            vec![
                ("Sevagoth Prime Chassis Blueprint".to_string(), 1),
                ("Sevagoth Prime Neuroptics Blueprint".to_string(), 1),
                ("Sevagoth Prime Systems Blueprint".to_string(), 1),
                ("Sevagoth Prime Blueprint".to_string(), 1),
            ]
        );
    }

    #[test]
    fn trade_purchase_extracts_received_items() {
        let raw = "Dialog::CreateOkCancel(description=Are you sure you want to accept this trade? You are offering:\r\nPlatinum x 20\r\n\r\nand will receive from Buyer123 the following:\r\nAyatan Anasa Sculpture, title= leftItem=/Menu/Confirm_Item_Ok, rightItem=/Menu/Confirm_Item_Cancel)";

        let parsed = parse_trade_dialog(raw).expect("dialog should parse");
        assert_eq!(parsed.trade_type, "purchase");
        assert_eq!(parsed.offered_plat, 20);
        assert_eq!(parsed.received_items, vec![("Ayatan Anasa Sculpture".to_string(), 1)]);
    }

    #[test]
    fn trade_completion_reassembles_split_log_reads() {
        let chunks = [
            "unrelated log text\nDialog::CreateOkCan",
            "cel(description=Are you sure? You are off",
            "ering:\r\nSaryn Prime Chassis Blueprint\r\n\r\nand will receive from Buyer123 the follow",
            "ing:\r\nPlatinum x 15, title= leftItem=/Menu/Confirm_Item_Ok)\nThe trade was succ",
            "essful\n",
        ];
        let mut trade_buffer = String::new();
        let mut result = None;

        for chunk in chunks {
            let (_, parsed) = collect_trade_completion(chunk, &mut trade_buffer);
            if parsed.is_some() {
                result = parsed;
            }
        }

        let parsed = result.expect("split dialog and completion should be reconstructed");
        assert_eq!(parsed.with_player, "Buyer123");
        assert_eq!(parsed.trade_type, "sale");
        assert_eq!(parsed.received_plat, 15);
        assert_eq!(parsed.offered_items, vec![("Saryn Prime Chassis Blueprint".to_string(), 1)]);
        assert!(trade_buffer.is_empty());
    }
}
