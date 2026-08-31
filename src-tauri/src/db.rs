use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct QuantityChange {
    pub id: i64,
    pub unique_name: String,
    pub item_name: String,
    pub old_qty: i64,
    pub new_qty: i64,
    pub delta: i64,
    pub timestamp: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Trade {
    pub id: i64,
    pub timestamp: String,      // ISO-8601
    pub with_player: String,
    pub direction: String,      // "sold" | "bought" | "traded-out" | "traded-in"
    pub item_name: String,
    pub item_url: String,       // WFM slug (for price lookup), may be empty
    pub quantity: i64,
    pub platinum: i64,
    pub source: String,         // "wfm" | "ingame" | "manual"
    pub notes: String,
    pub session_id: String,     // groups items from the same trade session
    pub trade_type: String,     // "sale" | "purchase" | "trade" | "" (legacy)
}

pub fn init_db(db_path: &PathBuf) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    migrate(&conn)?;
    Ok(conn)
}

/// Schema migrations keyed by version number.
/// To add a schema change in a future release: add an `if version < N` block
/// with the ALTER/CREATE SQL and bump `pragma_update` to N.
fn migrate(conn: &Connection) -> Result<()> {
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

    if version < 2 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS quantity_changes (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                unique_name TEXT    NOT NULL,
                item_name   TEXT    NOT NULL,
                old_qty     INTEGER NOT NULL,
                new_qty     INTEGER NOT NULL,
                delta       INTEGER NOT NULL,
                timestamp   INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS trades (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp   TEXT    NOT NULL,
                with_player TEXT    NOT NULL DEFAULT '',
                direction   TEXT    NOT NULL DEFAULT 'sold',
                item_name   TEXT    NOT NULL,
                item_url    TEXT    NOT NULL DEFAULT '',
                quantity    INTEGER NOT NULL DEFAULT 1,
                platinum    INTEGER NOT NULL DEFAULT 0,
                source      TEXT    NOT NULL DEFAULT 'manual',
                notes       TEXT    NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS saved_rivens (
                id          TEXT    PRIMARY KEY,
                weapon      TEXT    NOT NULL,
                label       TEXT    NOT NULL,
                stats_json  TEXT    NOT NULL,
                verdict     TEXT    NOT NULL DEFAULT '',
                score       REAL    NOT NULL DEFAULT 0,
                saved_at    TEXT    NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tracked_items (
                unique_name  TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                added_at     TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS item_snapshots (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                unique_name TEXT    NOT NULL,
                date        TEXT    NOT NULL,
                quantity    INTEGER NOT NULL,
                UNIQUE(unique_name, date)
            );",
        )?;
        conn.pragma_update(None, "user_version", 2)?;
    }

    if version < 3 {
        conn.execute_batch(
            "ALTER TABLE trades ADD COLUMN session_id TEXT NOT NULL DEFAULT '';
             ALTER TABLE trades ADD COLUMN trade_type TEXT NOT NULL DEFAULT '';"
        )?;
        conn.pragma_update(None, "user_version", 3)?;
    }

    // Prune entries older than 7 days so the log doesn't grow unbounded.
    conn.execute_batch(
        "DELETE FROM quantity_changes WHERE timestamp < unixepoch('now', '-7 days');"
    )?;

    Ok(())
}

// ── Tracked items / daily snapshots ──────────────────────────────────────────

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone, PartialEq)]
pub struct TrackedItem {
    pub unique_name:  String,
    pub display_name: String,
    pub added_at:     String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct SnapshotPoint {
    pub date:     String,
    pub quantity: i64,
    pub change:   i64,
}

pub fn add_tracked_item(conn: &Connection, unique_name: &str, display_name: &str) -> Result<()> {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    conn.execute(
        "INSERT OR IGNORE INTO tracked_items (unique_name, display_name, added_at)
         VALUES (?1, ?2, ?3)",
        params![unique_name, display_name, now],
    )?;
    Ok(())
}

pub fn remove_tracked_item(conn: &Connection, unique_name: &str) -> Result<()> {
    conn.execute("DELETE FROM tracked_items  WHERE unique_name = ?1", params![unique_name])?;
    conn.execute("DELETE FROM item_snapshots WHERE unique_name = ?1", params![unique_name])?;
    Ok(())
}

pub fn get_tracked_items(conn: &Connection) -> Result<Vec<TrackedItem>> {
    let mut stmt = conn.prepare(
        "SELECT unique_name, display_name, added_at FROM tracked_items ORDER BY display_name",
    )?;
    let rows = stmt.query_map([], |row| Ok(TrackedItem {
        unique_name:  row.get(0)?,
        display_name: row.get(1)?,
        added_at:     row.get(2)?,
    }))?.filter_map(|r| r.ok()).collect();
    Ok(rows)
}

/// Record quantity for a tracked item on a given date.
/// INSERT OR IGNORE — the first scan of each day wins (stable historical record).
pub fn record_snapshot(conn: &Connection, unique_name: &str, date: &str, quantity: i64) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO item_snapshots (unique_name, date, quantity) VALUES (?1, ?2, ?3)",
        params![unique_name, date, quantity],
    )?;
    Ok(())
}

pub fn get_snapshots(conn: &Connection, unique_name: &str, days: Option<u32>) -> Result<Vec<SnapshotPoint>> {
    let raw: Vec<(String, i64)> = match days {
        Some(d) => {
            let cutoff = format!("-{} days", d);
            let mut stmt = conn.prepare(
                "SELECT date, quantity FROM item_snapshots
                 WHERE unique_name = ?1 AND date >= date('now', ?2)
                 ORDER BY date ASC",
            )?;
            let rows: Vec<(String, i64)> = stmt
                .query_map(params![unique_name, cutoff], |r| Ok((r.get(0)?, r.get(1)?)))?
                .filter_map(|r| r.ok())
                .collect();
            rows
        }
        None => {
            let mut stmt = conn.prepare(
                "SELECT date, quantity FROM item_snapshots
                 WHERE unique_name = ?1 ORDER BY date ASC",
            )?;
            let rows: Vec<(String, i64)> = stmt
                .query_map(params![unique_name], |r| Ok((r.get(0)?, r.get(1)?)))?
                .filter_map(|r| r.ok())
                .collect();
            rows
        }
    };

    Ok(raw.iter().enumerate().map(|(i, (date, qty))| {
        let change = if i == 0 { 0 } else { qty - raw[i - 1].1 };
        SnapshotPoint { date: date.clone(), quantity: *qty, change }
    }).collect())
}

// ── Saved rivens ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SavedRiven {
    pub id: String,
    pub weapon: String,
    pub label: String,
    pub stats_json: String,   // JSON array of {name, value, positive}
    pub verdict: String,
    pub score: f64,
    pub saved_at: String,
}

pub fn save_riven(conn: &Connection, riven: &SavedRiven) -> Result<()> {
    conn.execute(
        "INSERT INTO saved_rivens (id, weapon, label, stats_json, verdict, score, saved_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![riven.id, riven.weapon, riven.label, riven.stats_json,
                riven.verdict, riven.score, riven.saved_at],
    )?;
    Ok(())
}

pub fn get_saved_rivens(conn: &Connection) -> Result<Vec<SavedRiven>> {
    let mut stmt = conn.prepare(
        "SELECT id, weapon, label, stats_json, verdict, score, saved_at
         FROM saved_rivens ORDER BY saved_at DESC LIMIT 50",
    )?;
    let rows = stmt.query_map([], |row| Ok(SavedRiven {
        id: row.get(0)?, weapon: row.get(1)?, label: row.get(2)?,
        stats_json: row.get(3)?, verdict: row.get(4)?,
        score: row.get(5)?, saved_at: row.get(6)?,
    }))?.filter_map(|r| r.ok()).collect();
    Ok(rows)
}

pub fn delete_saved_riven(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM saved_rivens WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn rename_saved_riven(conn: &Connection, id: &str, label: &str) -> Result<()> {
    conn.execute("UPDATE saved_rivens SET label = ?1 WHERE id = ?2", params![label, id])?;
    Ok(())
}

pub fn count_saved_rivens(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM saved_rivens", [], |r| r.get(0))
}

pub fn add_trade(conn: &Connection, trade: &Trade) -> Result<i64> {
    conn.execute(
        "INSERT INTO trades (timestamp, with_player, direction, item_name, item_url, quantity, platinum, source, notes, session_id, trade_type)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            trade.timestamp, trade.with_player, trade.direction,
            trade.item_name, trade.item_url, trade.quantity,
            trade.platinum, trade.source, trade.notes,
            trade.session_id, trade.trade_type,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_trades(conn: &Connection) -> Result<Vec<Trade>> {
    let mut stmt = conn.prepare(
        "SELECT id, timestamp, with_player, direction, item_name, item_url,
                quantity, platinum, source, notes, session_id, trade_type
         FROM trades ORDER BY timestamp DESC",
    )?;
    let rows = stmt.query_map([], |row| Ok(Trade {
        id: row.get(0)?,
        timestamp: row.get(1)?,
        with_player: row.get(2)?,
        direction: row.get(3)?,
        item_name: row.get(4)?,
        item_url: row.get(5)?,
        quantity: row.get(6)?,
        platinum: row.get(7)?,
        source: row.get(8)?,
        notes: row.get(9)?,
        session_id: row.get(10)?,
        trade_type: row.get(11)?,
    }))?.filter_map(|r| r.ok()).collect();
    Ok(rows)
}

pub fn delete_trade(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM trades WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn add_quantity_change(
    conn: &Connection,
    unique_name: &str,
    item_name: &str,
    old_qty: i64,
    new_qty: i64,
) -> Result<()> {
    let ts = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO quantity_changes (unique_name, item_name, old_qty, new_qty, delta, timestamp)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![unique_name, item_name, old_qty, new_qty, new_qty - old_qty, ts],
    )?;
    Ok(())
}

pub fn get_quantity_changes(conn: &Connection, limit: i64) -> Result<Vec<QuantityChange>> {
    let mut stmt = conn.prepare(
        "SELECT id, unique_name, item_name, old_qty, new_qty, delta, timestamp
         FROM quantity_changes
         ORDER BY id DESC
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit], |row| {
            Ok(QuantityChange {
                id: row.get(0)?,
                unique_name: row.get(1)?,
                item_name: row.get(2)?,
                old_qty: row.get(3)?,
                new_qty: row.get(4)?,
                delta: row.get(5)?,
                timestamp: row.get(6)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

// ── Stats export / import ─────────────────────────────────────────────────────

/// Written into every export so a file from some other tool is rejected before
/// it can be interpreted as trade history.
pub const EXPORT_FORMAT: &str = "frameforge-stats";

/// Bump only for a change older builds cannot read; import refuses anything
/// other than the version it was built for.
pub const EXPORT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SnapshotRow {
    pub unique_name: String,
    pub date:        String,
    pub quantity:    i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ExportDocument {
    pub format:         String,
    pub version:        u32,
    pub trades:         Vec<Trade>,
    pub item_snapshots: Vec<SnapshotRow>,
    pub tracked_items:  Vec<TrackedItem>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct ImportCounts {
    pub trades_added:      usize,
    pub trades_skipped:    usize,
    pub snapshots_added:   usize,
    pub snapshots_skipped: usize,
    pub tracked_added:     usize,
    pub tracked_skipped:   usize,
}

/// A trade carries no identifier that survives moving between installs: `id` is
/// a per-database autoincrement, so two machines both number their trades from
/// one. What distinguishes a trade is therefore what was recorded about it.
fn trade_identity(t: &Trade) -> impl Ord + '_ {
    (
        &t.timestamp, &t.with_player, &t.direction, &t.item_name, &t.item_url,
        t.quantity, t.platinum, &t.source, &t.notes, &t.session_id, &t.trade_type,
    )
}

pub fn export_document(conn: &Connection) -> Result<ExportDocument> {
    // Ordered by identity rather than recency so exporting the same data twice
    // yields the same document.
    let mut trades = get_trades(conn)?;
    trades.sort_by(|a, b| trade_identity(a).cmp(&trade_identity(b)));

    let mut stmt = conn.prepare(
        "SELECT unique_name, date, quantity FROM item_snapshots ORDER BY unique_name, date",
    )?;
    let item_snapshots = stmt.query_map([], |row| Ok(SnapshotRow {
        unique_name: row.get(0)?,
        date:        row.get(1)?,
        quantity:    row.get(2)?,
    }))?.collect::<Result<Vec<_>>>()?;
    drop(stmt);

    let mut tracked_items = get_tracked_items(conn)?;
    tracked_items.sort_by(|a, b| a.unique_name.cmp(&b.unique_name));

    Ok(ExportDocument {
        format: EXPORT_FORMAT.to_string(),
        version: EXPORT_VERSION,
        trades,
        item_snapshots,
        tracked_items,
    })
}

/// The header is checked ahead of the body so a version mismatch says so
/// rather than surfacing as a missing-field error.
pub fn parse_export(json: &str) -> std::result::Result<ExportDocument, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Not valid JSON: {e}"))?;

    match value.get("format").and_then(|v| v.as_str()) {
        Some(EXPORT_FORMAT) => {}
        Some(other) => return Err(format!("Not a FrameForge export (format: {other})")),
        None => return Err("Not a FrameForge export (no format marker)".into()),
    }
    match value.get("version").and_then(|v| v.as_u64()) {
        Some(v) if v as u32 == EXPORT_VERSION => {}
        Some(v) => return Err(format!(
            "Export version {v} cannot be read by this build (expected {EXPORT_VERSION})"
        )),
        None => return Err("Export is missing its version".into()),
    }

    serde_json::from_value(value).map_err(|e| format!("Export is malformed: {e}"))
}

/// Existing rows win on every conflict and nothing is removed, so importing an
/// older backup cannot lose newer history. One transaction covers all three
/// sections: a failure part-way leaves the database as it was.
pub fn import_document(conn: &mut Connection, doc: &ExportDocument) -> Result<ImportCounts> {
    let tx = conn.transaction()?;
    let mut counts = ImportCounts::default();

    for trade in &doc.trades {
        // No id is carried over: the destination assigns its own.
        let added = tx.execute(
            "INSERT INTO trades
             (timestamp, with_player, direction, item_name, item_url,
              quantity, platinum, source, notes, session_id, trade_type)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11
             WHERE NOT EXISTS (
                 SELECT 1 FROM trades
                 WHERE timestamp = ?1 AND with_player = ?2 AND direction = ?3
                   AND item_name = ?4 AND item_url = ?5 AND quantity = ?6
                   AND platinum = ?7 AND source = ?8 AND notes = ?9
                   AND session_id = ?10 AND trade_type = ?11)",
            params![
                trade.timestamp, trade.with_player, trade.direction,
                trade.item_name, trade.item_url, trade.quantity, trade.platinum,
                trade.source, trade.notes, trade.session_id, trade.trade_type,
            ],
        )?;
        if added == 1 { counts.trades_added += 1 } else { counts.trades_skipped += 1 }
    }

    for snap in &doc.item_snapshots {
        let added = tx.execute(
            "INSERT OR IGNORE INTO item_snapshots (unique_name, date, quantity)
             VALUES (?1, ?2, ?3)",
            params![snap.unique_name, snap.date, snap.quantity],
        )?;
        if added == 1 { counts.snapshots_added += 1 } else { counts.snapshots_skipped += 1 }
    }

    for item in &doc.tracked_items {
        let added = tx.execute(
            "INSERT OR IGNORE INTO tracked_items (unique_name, display_name, added_at)
             VALUES (?1, ?2, ?3)",
            params![item.unique_name, item.display_name, item.added_at],
        )?;
        if added == 1 { counts.tracked_added += 1 } else { counts.tracked_skipped += 1 }
    }

    tx.commit()?;
    Ok(counts)
}

#[cfg(test)]
mod export_import_tests {
    use super::*;

    fn db(name: &str) -> Connection {
        let dir = std::env::temp_dir().join("frameforge-export-tests");
        std::fs::create_dir_all(&dir).expect("temp dir is always writable");
        let path = dir.join(format!("{name}.sqlite"));
        let _ = std::fs::remove_file(&path);
        init_db(&path).expect("a fresh database always initialises")
    }

    fn trade(id: i64, item: &str) -> Trade {
        Trade {
            id,
            timestamp: format!("2026-01-{id:02}T00:00:00Z"),
            with_player: "Tenno".into(),
            direction: "sold".into(),
            item_name: item.into(),
            item_url: "".into(),
            quantity: 1,
            platinum: 20,
            source: "manual".into(),
            notes: "".into(),
            session_id: "".into(),
            trade_type: "sale".into(),
        }
    }

    fn populate(conn: &Connection) {
        add_trade(conn, &trade(1, "Rhino Prime Systems")).expect("insert succeeds");
        add_trade(conn, &trade(2, "Soma Prime Barrel")).expect("insert succeeds");
        add_tracked_item(conn, "/Lotus/Types/Items/Ducats", "Ducats").expect("insert succeeds");
        record_snapshot(conn, "/Lotus/Types/Items/Ducats", "2026-01-01", 500).expect("insert succeeds");
        record_snapshot(conn, "/Lotus/Types/Items/Ducats", "2026-01-02", 620).expect("insert succeeds");
    }

    #[test]
    fn a_round_trip_through_a_fresh_database_reproduces_the_document() {
        let source = db("roundtrip-source");
        populate(&source);
        let original = export_document(&source).expect("export succeeds");

        let mut target = db("roundtrip-target");
        let json = serde_json::to_string(&original).expect("the document is serialisable");
        let parsed = parse_export(&json).expect("our own export parses");
        import_document(&mut target, &parsed).expect("import succeeds");

        assert_eq!(export_document(&target).expect("export succeeds"), original);
    }

    #[test]
    fn importing_the_same_file_twice_adds_nothing_the_second_time() {
        let source = db("idempotent-source");
        populate(&source);
        let doc = export_document(&source).expect("export succeeds");

        let mut target = db("idempotent-target");
        import_document(&mut target, &doc).expect("import succeeds");
        let second = import_document(&mut target, &doc).expect("import succeeds");

        assert_eq!(second, ImportCounts {
            trades_skipped: 2, snapshots_skipped: 2, tracked_skipped: 1, ..Default::default()
        });
    }

    #[test]
    fn an_overlapping_import_keeps_both_histories() {
        let source = db("merge-source");
        populate(&source);
        let mut doc = export_document(&source).expect("export succeeds");
        doc.trades[0].item_name = "Overwritten Prime".into();
        doc.trades.push(trade(3, "Mag Prime Chassis"));

        let mut target = db("merge-target");
        populate(&target);
        let counts = import_document(&mut target, &doc).expect("import succeeds");

        assert_eq!(counts, ImportCounts {
            trades_added: 2, trades_skipped: 1,
            snapshots_skipped: 2, tracked_skipped: 1,
            ..Default::default()
        });
        let names: Vec<String> = export_document(&target).expect("export succeeds")
            .trades.into_iter().map(|t| t.item_name).collect();
        assert_eq!(names.len(), 4);
        assert!(names.contains(&"Rhino Prime Systems".to_string()));
        assert!(names.contains(&"Overwritten Prime".to_string()));
    }

    #[test]
    fn trades_from_another_install_survive_colliding_ids() {
        let source = db("collide-source");
        add_trade(&source, &trade(1, "Mag Prime Chassis")).expect("insert succeeds");
        add_trade(&source, &trade(2, "Volt Prime Neuroptics")).expect("insert succeeds");
        let doc = export_document(&source).expect("export succeeds");

        // Both databases number these trades 1 and 2, but no two of them are the
        // same trade.
        let mut target = db("collide-target");
        populate(&target);
        let counts = import_document(&mut target, &doc).expect("import succeeds");

        assert_eq!(counts.trades_added, 2);
        assert_eq!(get_trades(&target).expect("read succeeds").len(), 4);
    }

    #[test]
    fn a_file_that_is_not_an_export_is_refused() {
        let conn = db("reject");
        populate(&conn);
        let before = export_document(&conn).expect("export succeeds");

        for bad in [
            "{not json",
            r#"{"trades":[],"item_snapshots":[],"tracked_items":[]}"#,
            r#"{"format":"some-other-tool","version":1,"trades":[],"item_snapshots":[],"tracked_items":[]}"#,
            r#"{"format":"frameforge-stats","trades":[],"item_snapshots":[],"tracked_items":[]}"#,
            r#"{"format":"frameforge-stats","version":99,"trades":[],"item_snapshots":[],"tracked_items":[]}"#,
            r#"{"format":"frameforge-stats","version":1,"trades":"nope"}"#,
        ] {
            let err = parse_export(bad).expect_err("a bad document must not parse");
            assert!(!err.is_empty(), "rejection must explain itself");
        }

        // Nothing above reached import_document, so the database is untouched.
        assert_eq!(export_document(&conn).expect("export succeeds"), before);
    }
}
