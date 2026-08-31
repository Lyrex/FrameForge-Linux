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
    /// Survives export and import; `id` is the local rowid.
    pub uid: String,
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

/// Derived from content, not random, so installs that shared a database before
/// the column existed still dedupe each other's imports. Identical rows differ
/// by insertion rank. The separator is char(31) because a field containing the
/// joiner would alias another row and fail the unique index mid-migration.
const BACKFILL_UIDS: &str = "
    UPDATE trades SET uid = (
        SELECT key FROM (
            SELECT id,
                   'v3' || char(31) || timestamp || char(31) || with_player
                        || char(31) || direction || char(31) || item_name
                        || char(31) || item_url || char(31) || quantity
                        || char(31) || platinum || char(31) || source
                        || char(31) || notes || char(31) || session_id
                        || char(31) || trade_type || char(31)
                        || row_number() OVER (
                               PARTITION BY timestamp, with_player, direction,
                                            item_name, item_url, quantity,
                                            platinum, source, notes,
                                            session_id, trade_type
                               ORDER BY id) AS key
            FROM trades) keyed
        WHERE keyed.id = trades.id)
    WHERE uid = '';";

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

    if version < 4 {
        // Pragma inside the transaction: a crash after the ALTER but before
        // the version write would make every later launch re-add the column.
        conn.execute_batch(&format!(
            "BEGIN;
             ALTER TABLE trades ADD COLUMN uid TEXT NOT NULL DEFAULT '';
             {BACKFILL_UIDS}
             CREATE UNIQUE INDEX trades_uid ON trades(uid);
             PRAGMA user_version = 4;
             COMMIT;"
        ))?;
    }

    // An older build inserts '' for uid; repair after a downgrade and upgrade.
    conn.execute_batch(BACKFILL_UIDS)?;

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

/// `trade.uid` is ignored; a foreign uid only enters via `import_document`.
pub fn add_trade(conn: &Connection, trade: &Trade) -> Result<i64> {
    conn.execute(
        // `randomblob(16)` has a v4 UUID's randomness without a crate for it.
        "INSERT INTO trades (uid, timestamp, with_player, direction, item_name, item_url, quantity, platinum, source, notes, session_id, trade_type)
         VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
        "SELECT id, uid, timestamp, with_player, direction, item_name, item_url,
                quantity, platinum, source, notes, session_id, trade_type
         FROM trades ORDER BY timestamp DESC",
    )?;
    let rows = stmt.query_map([], |row| Ok(Trade {
        id: row.get(0)?,
        uid: row.get(1)?,
        timestamp: row.get(2)?,
        with_player: row.get(3)?,
        direction: row.get(4)?,
        item_name: row.get(5)?,
        item_url: row.get(6)?,
        quantity: row.get(7)?,
        platinum: row.get(8)?,
        source: row.get(9)?,
        notes: row.get(10)?,
        session_id: row.get(11)?,
        trade_type: row.get(12)?,
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
pub const EXPORT_VERSION: u32 = 2;

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

pub fn export_document(conn: &Connection) -> Result<ExportDocument> {
    // Sorted and with rowids cleared so a re-export matches its source.
    let mut trades = get_trades(conn)?;
    trades.sort_by(|a, b| a.uid.cmp(&b.uid));
    for t in &mut trades { t.id = 0 }

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

    let doc: ExportDocument =
        serde_json::from_value(value).map_err(|e| format!("Export is malformed: {e}"))?;
    // One blank uid would land and make every later one read as present.
    if doc.trades.iter().any(|t| t.uid.is_empty()) {
        return Err("Export is malformed: a trade has no uid".into());
    }
    Ok(doc)
}

/// Existing rows win on every conflict and nothing is removed, so importing an
/// older backup cannot lose newer history. One transaction covers all three
/// sections: a failure part-way leaves the database as it was.
pub fn import_document(conn: &mut Connection, doc: &ExportDocument) -> Result<ImportCounts> {
    let tx = conn.transaction()?;
    let mut counts = ImportCounts::default();

    for trade in &doc.trades {
        let added = tx.execute(
            "INSERT OR IGNORE INTO trades
             (uid, timestamp, with_player, direction, item_name, item_url,
              quantity, platinum, source, notes, session_id, trade_type)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                trade.uid, trade.timestamp, trade.with_player, trade.direction,
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
            uid: String::new(),
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
    fn two_identical_trades_both_survive_a_round_trip() {
        let source = db("identical-source");
        let twice = trade(1, "Ash Prime Blueprint");
        add_trade(&source, &twice).expect("insert succeeds");
        add_trade(&source, &twice).expect("insert succeeds");
        let original = export_document(&source).expect("export succeeds");
        assert_eq!(original.trades.len(), 2);

        let mut target = db("identical-target");
        import_document(&mut target, &original).expect("import succeeds");

        let round_tripped = export_document(&target).expect("export succeeds");
        assert_eq!(round_tripped.trades.len(), 2);
        assert_eq!(
            serde_json::to_string(&round_tripped).expect("the document is serialisable"),
            serde_json::to_string(&original).expect("the document is serialisable"),
        );
    }

    #[test]
    fn an_import_never_overwrites_a_trade_the_target_already_has() {
        let source = db("merge-source");
        populate(&source);
        let mut doc = export_document(&source).expect("export succeeds");

        let mut target = db("merge-target");
        import_document(&mut target, &doc).expect("import succeeds");
        doc.trades[0].item_name = "Overwritten Prime".into();
        doc.trades.push(Trade { uid: "uid-mag".into(), ..trade(3, "Mag Prime Chassis") });
        let counts = import_document(&mut target, &doc).expect("import succeeds");

        assert_eq!(counts, ImportCounts {
            trades_added: 1, trades_skipped: 2,
            snapshots_skipped: 2, tracked_skipped: 1,
            ..Default::default()
        });
        let names: Vec<String> = export_document(&target).expect("export succeeds")
            .trades.into_iter().map(|t| t.item_name).collect();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"Rhino Prime Systems".to_string()));
        assert!(!names.contains(&"Overwritten Prime".to_string()));
    }

    #[test]
    fn trades_from_another_install_survive_colliding_ids() {
        let source = db("collide-source");
        add_trade(&source, &trade(1, "Mag Prime Chassis")).expect("insert succeeds");
        add_trade(&source, &trade(2, "Volt Prime Neuroptics")).expect("insert succeeds");
        let doc = export_document(&source).expect("export succeeds");

        let mut target = db("collide-target");
        populate(&target);
        let counts = import_document(&mut target, &doc).expect("import succeeds");

        assert_eq!(counts.trades_added, 2);
        assert_eq!(get_trades(&target).expect("read succeeds").len(), 4);
    }

    fn pre_uid_db(name: &str) -> Connection {
        let conn = db(name);
        conn.execute_batch(
            "DROP INDEX trades_uid;
             ALTER TABLE trades DROP COLUMN uid;
             INSERT INTO trades (timestamp, item_name) VALUES ('2026-01-01T00:00:00Z', 'Boar Prime Stock');
             INSERT INTO trades (timestamp, item_name) VALUES ('2026-01-01T00:00:00Z', 'Boar Prime Stock');
             PRAGMA user_version = 3;",
        ).expect("the current schema can be wound back one version");
        conn
    }

    fn uids(conn: &Connection) -> Vec<String> {
        get_trades(conn).expect("read succeeds").into_iter().map(|t| t.uid).collect()
    }

    #[test]
    fn the_migration_gives_pre_existing_trades_an_id_and_can_run_again() {
        let conn = pre_uid_db("migrate");
        migrate(&conn).expect("migration succeeds on a populated database");
        migrate(&conn).expect("a second launch migrates nothing");

        let uids = uids(&conn);
        assert_eq!(uids.len(), 2);
        assert!(uids.iter().all(|u| !u.is_empty()));
        assert_ne!(uids[0], uids[1]);
    }

    #[test]
    fn two_installs_with_the_same_old_history_migrate_to_the_same_ids() {
        let desktop = pre_uid_db("migrate-desktop");
        let laptop = pre_uid_db("migrate-laptop");
        migrate(&desktop).expect("migration succeeds");
        migrate(&laptop).expect("migration succeeds");

        let mut desktop_uids = uids(&desktop);
        let mut laptop_uids = uids(&laptop);
        desktop_uids.sort();
        laptop_uids.sort();
        assert_eq!(desktop_uids, laptop_uids);
    }

    #[test]
    fn a_trade_logged_by_an_older_build_gets_its_id_on_the_next_start() {
        let conn = db("repair");
        conn.execute(
            "INSERT INTO trades (timestamp, item_name) VALUES ('2026-01-01T00:00:00Z', 'Boar Prime Stock')",
            [],
        ).expect("the column default admits one blank uid");
        migrate(&conn).expect("a later start repairs it");
        assert!(uids(&conn).iter().all(|u| !u.is_empty()));
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
            r#"{"format":"frameforge-stats","version":2,"trades":"nope"}"#,
            r#"{"format":"frameforge-stats","version":2,"trades":[{"id":0,"uid":"","timestamp":"","with_player":"","direction":"","item_name":"","item_url":"","quantity":1,"platinum":0,"source":"","notes":"","session_id":"","trade_type":""}],"item_snapshots":[],"tracked_items":[]}"#,
        ] {
            let err = parse_export(bad).expect_err("a bad document must not parse");
            assert!(!err.is_empty(), "rejection must explain itself");
        }

        // Nothing above reached import_document, so the database is untouched.
        assert_eq!(export_document(&conn).expect("export succeeds"), before);
    }
}
