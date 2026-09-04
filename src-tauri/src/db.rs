use crate::arbitration::{EndReason, Event, MissionType, Parser as ArbitrationParser, Run, Vitus};
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

    if version < 5 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS arbitration_runs (
                uid                TEXT    PRIMARY KEY,
                started_at         TEXT,
                run_start_sec      REAL    NOT NULL,
                run_end_sec        REAL,
                mission_name       TEXT    NOT NULL,
                node               TEXT    NOT NULL,
                sol_node           TEXT,
                mission_type       TEXT    NOT NULL,
                mission_type_raw   TEXT,
                end_reason         TEXT    NOT NULL,
                duration_sec       REAL    NOT NULL,
                rotations          INTEGER NOT NULL,
                waves              INTEGER NOT NULL,
                waves_per_rotation INTEGER NOT NULL,
                kills              INTEGER NOT NULL,
                drone_kills        INTEGER NOT NULL,
                host_telemetry     INTEGER NOT NULL,
                vitus_mean         REAL    NOT NULL,
                vitus_std          REAL    NOT NULL,
                vitus_per_minute   REAL    NOT NULL
            );
            CREATE INDEX IF NOT EXISTS arbitration_runs_started
                ON arbitration_runs(started_at);",
        )?;
        conn.pragma_update(None, "user_version", 5)?;
    }

    // A deleted run keeps its row: startup backfill re-reads the same log and
    // would otherwise insert it again. The column check keeps this step
    // re-runnable after a downgrade, as the CREATE IF NOT EXISTS above is.
    if version < 6 {
        let has_deleted: bool = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('arbitration_runs') WHERE name = 'deleted'",
            [],
            |r| r.get::<_, i64>(0).map(|n| n > 0),
        )?;
        if !has_deleted {
            conn.execute_batch(
                "ALTER TABLE arbitration_runs ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        conn.pragma_update(None, "user_version", 6)?;
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

// ── Arbitration runs ──────────────────────────────────────────────────────────
//
// Runs arrive from two directions — the tail of a live log and a backfill pass
// over the whole file at startup — and both go through `ArbitrationRecorder`,
// so there is one parser and one insert. Re-reading a log the database has
// already seen must add nothing, which is what `uid` buys.

/// Identity of a run, from the two things a log fixes about it: when it began
/// and where. Wall clock comes from the log's boot-time header; without one,
/// game time since launch stands in.
///
/// ponytail: two runs on the same node at the same game-time offset in a log
/// with no boot header collide and the second is dropped. Store a per-launch
/// discriminator if a header-less log ever shows up in practice.
fn run_uid(run: &Run) -> String {
    let when = match run.started_at {
        Some(t) => stored_timestamp(t),
        None => format!("t{:.3}", run.run_start_sec),
    };
    format!("{when}\u{1f}{}", run.node)
}

/// Date ranges are compared as text in SQL, so every timestamp that reaches
/// the column has to be written the same way: UTC, milliseconds, `Z`. Chrono's
/// plain `to_rfc3339` writes `+00:00`, which sorts *below* the `Z` form that
/// every other tool produces, and a bound in one form silently excludes rows
/// stored in the other.
fn stored_timestamp(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Puts a filter bound in the same shape as the column. A bare `YYYY-MM-DD`
/// is taken as the whole day, which is what a date picker means by it and
/// what a plain text comparison would otherwise get wrong at the upper end.
fn bound(raw: &str, day_end: bool) -> String {
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(raw) {
        return stored_timestamp(t.with_timezone(&chrono::Utc));
    }
    if chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d").is_ok() {
        let time = if day_end { "T23:59:59.999Z" } else { "T00:00:00.000Z" };
        return format!("{raw}{time}");
    }
    raw.to_string()
}

/// Returns false when the database already had this run.
pub fn store_arbitration_run(conn: &Connection, run: &Run) -> Result<bool> {
    let changed = conn.execute(
        "INSERT OR IGNORE INTO arbitration_runs (
             uid, started_at, run_start_sec, run_end_sec, mission_name, node,
             sol_node, mission_type, mission_type_raw, end_reason, duration_sec,
             rotations, waves, waves_per_rotation, kills, drone_kills,
             host_telemetry, vitus_mean, vitus_std, vitus_per_minute)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                 ?15, ?16, ?17, ?18, ?19, ?20)",
        params![
            run_uid(run),
            run.started_at.map(stored_timestamp),
            run.run_start_sec,
            run.run_end_sec,
            run.mission_name,
            run.node,
            run.sol_node,
            run.mission_type.as_str(),
            run.mission_type_raw,
            run.end_reason.as_str(),
            run.duration_sec,
            run.rotations,
            run.waves,
            run.waves_per_rotation,
            run.kills,
            run.drone_kills,
            run.host_telemetry,
            run.vitus.mean,
            run.vitus.std,
            run.vitus.per_minute,
        ],
    )?;
    Ok(changed > 0)
}

/// `from` and `to` accept either a full RFC-3339 timestamp or a bare
/// `YYYY-MM-DD`; both are normalised to the column's own form before the
/// comparison. Runs from a log with no boot-time header have no wall clock and
/// so fall outside any date range.
#[derive(Debug, Default, Clone)]
pub struct RunQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub node: Option<String>,
    pub mission_type: Option<String>,
}

pub fn get_arbitration_runs(conn: &Connection, query: &RunQuery) -> Result<Vec<Run>> {
    let mut stmt = conn.prepare(
        "SELECT started_at, run_start_sec, run_end_sec, mission_name, node,
                sol_node, mission_type, mission_type_raw, end_reason,
                duration_sec, rotations, waves, waves_per_rotation, kills,
                drone_kills, host_telemetry, vitus_mean, vitus_std,
                vitus_per_minute
         FROM arbitration_runs
         WHERE deleted = 0
           AND (?1 IS NULL OR started_at >= ?1)
           AND (?2 IS NULL OR started_at <= ?2)
           AND (?3 IS NULL OR node = ?3)
           AND (?4 IS NULL OR mission_type = ?4)
         ORDER BY started_at, run_start_sec",
    )?;
    let from = query.from.as_deref().map(|raw| bound(raw, false));
    let to = query.to.as_deref().map(|raw| bound(raw, true));
    let rows = stmt
        .query_map(
            params![from, to, query.node, query.mission_type],
            |row| {
                let started_at: Option<String> = row.get(0)?;
                let mission_type: String = row.get(6)?;
                let end_reason: String = row.get(8)?;
                Ok(Run {
                    started_at: started_at
                        .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
                        .map(|t| t.with_timezone(&chrono::Utc)),
                    run_start_sec: row.get(1)?,
                    run_end_sec: row.get(2)?,
                    mission_name: row.get(3)?,
                    node: row.get(4)?,
                    sol_node: row.get(5)?,
                    mission_type: MissionType::from_stored(&mission_type),
                    mission_type_raw: row.get(7)?,
                    end_reason: EndReason::from_stored(&end_reason),
                    duration_sec: row.get(9)?,
                    rotations: row.get(10)?,
                    waves: row.get(11)?,
                    waves_per_rotation: row.get(12)?,
                    kills: row.get(13)?,
                    drone_kills: row.get(14)?,
                    host_telemetry: row.get(15)?,
                    vitus: Vitus {
                        mean: row.get(16)?,
                        std: row.get(17)?,
                        per_minute: row.get(18)?,
                    },
                })
            },
        )?
        .collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, Serialize)]
pub struct RunRecord {
    pub uid: String,
    pub started_at: Option<String>,
    pub node: String,
    pub mission_type: &'static str,
    pub end_reason: &'static str,
    pub duration_sec: f64,
    pub rotations: u32,
    pub waves: u32,
    pub kills: u32,
    pub drone_kills: u32,
    pub vitus_mean: f64,
    pub vitus_per_minute: f64,
}

/// Newest first. Filtering happens in the view: the whole history is small,
/// and the filters change far more often than the data.
///
/// The key is read back from the row rather than re-derived, so a delete
/// names exactly what was stored even if the derivation changes.
pub fn list_arbitration_runs(conn: &Connection) -> Result<Vec<RunRecord>> {
    let mut stmt = conn.prepare(
        "SELECT uid, started_at, node, mission_type, end_reason, duration_sec,
                rotations, waves, kills, drone_kills, vitus_mean, vitus_per_minute
         FROM arbitration_runs
         WHERE deleted = 0
         ORDER BY started_at DESC, run_start_sec DESC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let mission_type: String = row.get(3)?;
            let end_reason: String = row.get(4)?;
            Ok(RunRecord {
                uid: row.get(0)?,
                started_at: row.get(1)?,
                node: row.get(2)?,
                mission_type: MissionType::from_stored(&mission_type).as_str(),
                end_reason: EndReason::from_stored(&end_reason).as_str(),
                duration_sec: row.get(5)?,
                rotations: row.get(6)?,
                waves: row.get(7)?,
                kills: row.get(8)?,
                drone_kills: row.get(9)?,
                vitus_mean: row.get(10)?,
                vitus_per_minute: row.get(11)?,
            })
        })?
        .collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn delete_arbitration_run(conn: &Connection, uid: &str) -> Result<bool> {
    let changed = conn.execute("UPDATE arbitration_runs SET deleted = 1 WHERE uid = ?1", params![uid])?;
    Ok(changed > 0)
}

/// Log text in, stored runs out, in two steps so that the database is only
/// locked for the writes: parsing a whole log at startup takes far longer than
/// the handful of inserts it produces, and every other query waits behind that
/// same lock.
///
/// Holds the parser across calls so a run split over several reads still ends
/// as one run.
#[derive(Default)]
pub struct ArbitrationRecorder {
    parser: ArbitrationParser,
    /// A read of the growing log ends wherever the game happened to flush, so
    /// the last line of a chunk is often half a line. It waits here for its
    /// other half rather than reaching the parser mangled.
    partial: String,
    /// Runs parsed but not yet written. The lines behind a run are consumed by
    /// the parser as they are read and cannot be replayed, so a run whose
    /// insert failed waits here for the next attempt instead of being lost.
    pending: std::collections::VecDeque<Run>,
}

/// Enough runs for a long session of failed writes. Past it the oldest go,
/// because a queue that grows for as long as the database is broken is a leak.
///
/// ponytail: the queue lives in memory, so a crash loses it either way, and
/// the oldest go first on the assumption that a newer run is the one worth
/// saving. Persist it if runs lost to a full disk turn out to matter.
const MAX_PENDING_RUNS: usize = 256;

impl ArbitrationRecorder {
    /// Reads runs out of the text. A run still in progress is not parsed out:
    /// it has no end and no final counts yet.
    ///
    /// TODO: a log that stops mid-run because the game crashed looks identical
    /// to one whose run is still going, so that last run is never recorded.
    /// Recovering it needs a signal that the log is finished with.
    pub fn parse(&mut self, chunk: String) {
        // Taking the text whole rather than copying it in matters at startup,
        // where the chunk is the entire log.
        if self.partial.is_empty() {
            self.partial = chunk;
        } else {
            self.partial.push_str(&chunk);
        }
        let Some(last_newline) = self.partial.rfind('\n') else {
            return;
        };
        let half_line = self.partial.split_off(last_newline + 1);
        let complete = std::mem::replace(&mut self.partial, half_line);

        for line in complete.lines() {
            for event in self.parser.feed_line(line) {
                if let Event::RunEnded(run) = event {
                    if self.pending.len() == MAX_PENDING_RUNS {
                        self.pending.pop_front();
                        tracing::warn!(
                            queued = MAX_PENDING_RUNS,
                            "arbitration run queue is full; dropping the oldest run"
                        );
                    }
                    self.pending.push_back(*run);
                }
            }
        }
    }

    /// Writes what `parse` found, and returns how many rows that added. A run
    /// stays queued until its insert succeeds, so a database that is busy or
    /// full delays runs rather than dropping them.
    ///
    /// One transaction for the batch, because a startup backfill's runs would
    /// otherwise cost a disk sync each. The queue is cleared only once the
    /// commit lands: a failure rolls the whole batch back, and the runs behind
    /// it have to still be there for the next attempt.
    pub fn store(&mut self, conn: &Connection) -> Result<usize> {
        if self.pending.is_empty() {
            return Ok(0);
        }
        let tx = conn.unchecked_transaction()?;
        let mut stored = 0;
        for run in &self.pending {
            if store_arbitration_run(&tx, run)? {
                stored += 1;
            }
        }
        tx.commit()?;
        self.pending.clear();
        Ok(stored)
    }
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

/// A database of a test's own, wiped before it is opened. WAL leaves sidecars,
/// and a stale one carries a previous run's rows into this one.
#[cfg(test)]
fn db(name: &str) -> Connection {
    let dir = std::env::temp_dir().join(format!("frameforge-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir is always writable");
    let path = dir.join(format!("{name}.sqlite"));
    for suffix in ["sqlite", "sqlite-wal", "sqlite-shm"] {
        let _ = std::fs::remove_file(path.with_extension(suffix));
    }
    init_db(&path).expect("a fresh database always initialises")
}

#[cfg(test)]
mod arbitration_storage_tests {
    use super::*;

    const DEFENSE: &str = include_str!("../tests/fixtures/arbitration/stoefler-defense.txt");
    const SURVIVAL: &str = include_str!("../tests/fixtures/arbitration/mot-survival.txt");
    const ABORT: &str = include_str!("../tests/fixtures/arbitration/oestrus-abort.txt");

    /// Splits the log into chunks that mostly end mid-line, the way a read of
    /// a file the game is still writing does.
    fn watch(conn: &Connection, log: &str, chunk_len: usize) -> usize {
        let mut recorder = ArbitrationRecorder::default();
        let bytes = log.as_bytes();
        let mut stored = 0;
        let mut at = 0;
        while at < bytes.len() {
            let mut end = (at + chunk_len).min(bytes.len());
            while !log.is_char_boundary(end) {
                end += 1;
            }
            recorder.parse(log[at..end].to_string());
            stored += recorder.store(conn).expect("recording succeeds");
            at = end;
        }
        stored
    }

    fn stored(conn: &Connection) -> Vec<crate::arbitration::Run> {
        get_arbitration_runs(conn, &RunQuery::default()).expect("read succeeds")
    }

    #[test]
    fn a_run_watched_in_chunks_is_stored_as_one_run() {
        let conn = db("live");
        assert_eq!(watch(&conn, DEFENSE, 97), 1);

        let runs = stored(&conn);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].node, "Stöfler (Lua)");
        assert_eq!(runs[0].end_reason, crate::arbitration::EndReason::MissionEnd);
        assert_eq!(
            runs[0].mission_type,
            crate::arbitration::MissionType::Defense
        );
    }

    #[test]
    fn an_aborted_run_is_stored_too() {
        let conn = db("abort");
        assert_eq!(watch(&conn, ABORT, 4096), 1);
        assert_eq!(stored(&conn)[0].end_reason, crate::arbitration::EndReason::Aborted);
    }

    /// Restarts and the startup backfill both re-read log text the database
    /// has already seen.
    #[test]
    fn processing_the_same_log_twice_stores_nothing_the_second_time() {
        let conn = db("idempotent");
        assert_eq!(watch(&conn, DEFENSE, 4096), 1);
        assert_eq!(watch(&conn, DEFENSE, 4096), 0);
        assert_eq!(stored(&conn).len(), 1);
    }

    #[test]
    fn a_run_still_in_progress_is_not_stored_until_it_ends() {
        let conn = db("in-progress");
        let cut = DEFENSE.len() / 2;
        let head = &DEFENSE[..DEFENSE[..cut].rfind('\n').expect("the fixture has lines") + 1];

        let mut recorder = ArbitrationRecorder::default();
        recorder.parse(head.to_string());
        assert_eq!(recorder.store(&conn).expect("recording succeeds"), 0);
        assert!(stored(&conn).is_empty());

        recorder.parse(DEFENSE[head.len()..].to_string());
        assert_eq!(recorder.store(&conn).expect("recording succeeds"), 1);
        assert_eq!(stored(&conn).len(), 1);
    }

    #[test]
    fn a_deleted_run_is_gone_and_stays_gone_across_a_backfill() {
        let conn = db("delete");
        watch(&conn, DEFENSE, 4096);
        watch(&conn, SURVIVAL, 4096);
        let listed = list_arbitration_runs(&conn).expect("read succeeds");
        assert_eq!(listed.len(), 2);
        assert!(listed[0].started_at > listed[1].started_at, "newest first");

        let survival = listed.iter().find(|r| r.node == "Mot (Void)").expect("the survival run is listed");
        assert!(delete_arbitration_run(&conn, &survival.uid).expect("delete succeeds"));
        assert!(!delete_arbitration_run(&conn, "no such run").expect("a miss is not an error"));
        assert_eq!(stored(&conn).len(), 1);
        assert_eq!(list_arbitration_runs(&conn).expect("read succeeds").len(), 1);
        assert_eq!(stored(&conn)[0].node, "Stöfler (Lua)");

        assert_eq!(watch(&conn, SURVIVAL, 4096), 0);
        assert_eq!(stored(&conn).len(), 1);
    }

    #[test]
    fn runs_are_queryable_by_date_range_node_and_mission_type() {
        let conn = db("query");
        watch(&conn, DEFENSE, 4096);
        watch(&conn, SURVIVAL, 4096);
        assert_eq!(stored(&conn).len(), 2);

        let query = |q: RunQuery| {
            get_arbitration_runs(&conn, &q)
                .expect("read succeeds")
                .into_iter()
                .map(|r| r.node)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            query(RunQuery { node: Some("Mot (Void)".into()), ..Default::default() }),
            vec!["Mot (Void)"]
        );
        assert_eq!(
            query(RunQuery { mission_type: Some("defense".into()), ..Default::default() }),
            vec!["Stöfler (Lua)"]
        );

        let earliest = stored(&conn)
            .iter()
            .map(|r| r.started_at.expect("the fixtures carry a boot header"))
            .min()
            .expect("two runs are stored");
        assert_eq!(
            query(RunQuery { from: Some(stored_timestamp(earliest)), ..Default::default() }).len(),
            2
        );
        assert!(query(RunQuery { to: Some("2000-01-01T00:00:00Z".into()), ..Default::default() }).is_empty());
    }

    /// A bound written the way anything but chrono writes it — `Z` rather than
    /// `+00:00`, or a bare date — has to select the same rows. Compared as raw
    /// text these forms sort against each other, not with each other.
    #[test]
    fn a_bound_in_any_iso_form_selects_the_same_runs() {
        let conn = db("bounds");
        watch(&conn, DEFENSE, 4096);
        let run = stored(&conn).remove(0);
        let at = run.started_at.expect("the fixture carries a boot header");
        let day = at.format("%Y-%m-%d").to_string();

        let count = |q: RunQuery| get_arbitration_runs(&conn, &q).expect("read succeeds").len();

        for from in [
            at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            at.to_rfc3339(),
            day.clone(),
        ] {
            assert_eq!(count(RunQuery { from: Some(from.clone()), ..Default::default() }), 1, "from {from}");
        }
        // Upper bounds are where the raw text comparison goes wrong: `Z` sorts
        // above `+00:00`, and a bare date sorts below every time on that date.
        assert_eq!(count(RunQuery { to: Some(at.to_rfc3339()), ..Default::default() }), 1);
        assert_eq!(count(RunQuery { to: Some(day.clone()), ..Default::default() }), 1);
        assert_eq!(
            count(RunQuery { from: Some(day.clone()), to: Some(day), ..Default::default() }),
            1
        );
    }

    /// The parser has already consumed the lines behind a queued run, so a
    /// write that fails has to leave the run recoverable rather than drop it.
    #[test]
    fn a_run_survives_a_failed_write_and_lands_on_the_next_attempt() {
        let conn = db("write-failure");
        conn.execute_batch("ALTER TABLE arbitration_runs RENAME TO arbitration_runs_hidden;")
            .expect("the table can be moved out of the way");

        let mut recorder = ArbitrationRecorder::default();
        recorder.parse(DEFENSE.to_string());
        assert!(recorder.store(&conn).is_err(), "the write must fail with no table");

        conn.execute_batch("ALTER TABLE arbitration_runs_hidden RENAME TO arbitration_runs;")
            .expect("the table can be put back");
        assert_eq!(
            recorder.store(&conn).expect("the retry succeeds"),
            1,
            "the run held on across the failure"
        );
        assert_eq!(stored(&conn).len(), 1);
    }

    /// Compares every field, so a column left out of the table fails here
    /// rather than reading back as a silent zero.
    #[test]
    fn a_stored_run_equals_the_run_the_parser_produced() {
        let whole = crate::arbitration::parse_log(DEFENSE.lines());
        for chunk_len in [1, 13, 512, 100_000] {
            let conn = db(&format!("boundary-{chunk_len}"));
            watch(&conn, DEFENSE, chunk_len);
            assert_eq!(stored(&conn), whole, "chunk length {chunk_len}");
        }
    }
}

#[cfg(test)]
mod export_import_tests {
    use super::*;

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
