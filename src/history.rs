//! Persistent history: per-turn records for work-time / project / intervention
//! statistics (plan: `docs/superpowers/plan/history-stats-plan.md`).
//!
//! Storage model (v1): a single cold `turn` table (+ a `project` lookup). The
//! daemon is the only writer; it appends one row per *closed* turn. The TUI
//! opens a read-only connection and runs aggregate queries. There is no hot
//! per-event table — A-tier metrics (work time / project split / intervention
//! response) are all derivable from turn rows, and row count grows with the
//! number of turns, not with runtime or event volume, so the DB stays bounded.
//!
//! SQLite is statically linked (`rusqlite` `bundled` feature): the user needs
//! nothing installed. The DB lives at `~/.roost/history.db` (override via
//! `$ROOST_HOME`). WAL mode lets the TUI read while the daemon writes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, params};

use crate::protocol::AgentFamily;

/// Schema + pragmas, applied idempotently on every open.
///
/// `journal_mode=WAL` enables a single writer + concurrent readers.
/// `synchronous=NORMAL` is the recommended durability/speed trade-off under WAL.
const SCHEMA: &str = "\
PRAGMA journal_mode=WAL;\n\
PRAGMA synchronous=NORMAL;\n\
CREATE TABLE IF NOT EXISTS project (\n\
  id   INTEGER PRIMARY KEY,\n\
  path TEXT UNIQUE NOT NULL,\n\
  name TEXT NOT NULL\n\
);\n\
CREATE TABLE IF NOT EXISTS turn (\n\
  id           INTEGER PRIMARY KEY,\n\
  session_id   TEXT    NOT NULL,\n\
  turn_seq     INTEGER NOT NULL,\n\
  family       TEXT    NOT NULL,\n\
  project_id   INTEGER NOT NULL REFERENCES project(id),\n\
  started_at   INTEGER NOT NULL,\n\
  ended_at     INTEGER NOT NULL,\n\
  busy_secs    INTEGER NOT NULL,\n\
  needed_input INTEGER NOT NULL DEFAULT 0,\n\
  wait_secs    INTEGER NOT NULL DEFAULT 0,\n\
  incomplete   INTEGER NOT NULL DEFAULT 0,\n\
  tool_counts  TEXT    NOT NULL DEFAULT '{}'\n\
);\n\
CREATE INDEX IF NOT EXISTS idx_turn_started ON turn(started_at);\n\
CREATE INDEX IF NOT EXISTS idx_turn_project ON turn(project_id);\n";

/// Resolve the roost data directory: `$ROOST_HOME`, else `~/.roost`.
pub fn roost_home() -> PathBuf {
    if let Some(dir) = std::env::var_os("ROOST_HOME") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".roost")
}

/// Path to the history database file.
pub fn db_path() -> PathBuf {
    roost_home().join("history.db")
}

/// A completed turn ready to be persisted. Produced by `AgentSession` when a
/// turn boundary is reached (Stop / next prompt / removal), consumed by the
/// daemon which writes it via [`History::record_turn`].
#[derive(Debug, Clone)]
pub struct ClosedTurn {
    pub session_id: String,
    pub turn_seq: u32,
    pub family: AgentFamily,
    /// Working directory — the dedup key for the `project` table.
    pub cwd: String,
    /// Display name (`naming::resolve_name` result) stored on the project row.
    pub name: String,
    pub started_at: SystemTime,
    pub ended_at: SystemTime,
    pub busy_secs: u64,
    pub needed_input: bool,
    pub wait_secs: u64,
    pub incomplete: bool,
    /// Tool-name → invocation count within the turn (B-tier; stored as JSON).
    /// `BTreeMap` keeps the JSON key order deterministic.
    pub tool_counts: BTreeMap<String, u32>,
}

/// Writer handle owned by the daemon (single writer).
pub struct History {
    conn: Connection,
}

impl History {
    /// Open (creating if needed) the history DB at the default path.
    pub fn open() -> Result<History> {
        Self::open_at(&db_path())
    }

    fn open_at(db_path: &Path) -> Result<History> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create history dir {}", parent.display()))?;
        }
        let conn = Connection::open(db_path)
            .with_context(|| format!("open history db {}", db_path.display()))?;
        conn.busy_timeout(Duration::from_secs(3))?;
        conn.execute_batch(SCHEMA)
            .context("apply history schema")?;
        Ok(History { conn })
    }

    /// Persist one closed turn. Upserts the project row, then inserts the turn.
    /// Wrapped in a transaction so the two writes are atomic.
    pub fn record_turn(&self, t: &ClosedTurn) -> Result<()> {
        let started = unix_secs(t.started_at);
        let ended = unix_secs(t.ended_at);
        let tool_json = serde_json::to_string(&t.tool_counts).unwrap_or_else(|_| "{}".to_string());

        self.conn
            .execute(
                "INSERT INTO project(path, name) VALUES (?1, ?2) \
                 ON CONFLICT(path) DO UPDATE SET name=excluded.name",
                params![t.cwd, t.name],
            )
            .context("upsert project")?;
        let project_id: i64 = self
            .conn
            .query_row(
                "SELECT id FROM project WHERE path=?1",
                params![t.cwd],
                |r| r.get(0),
            )
            .context("lookup project id")?;

        self.conn
            .execute(
                "INSERT INTO turn(session_id, turn_seq, family, project_id, started_at, \
                 ended_at, busy_secs, needed_input, wait_secs, incomplete, tool_counts) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    t.session_id,
                    t.turn_seq,
                    t.family.as_str(),
                    project_id,
                    started,
                    ended,
                    t.busy_secs,
                    t.needed_input as i64,
                    t.wait_secs,
                    t.incomplete as i64,
                    tool_json,
                ],
            )
            .context("insert turn")?;
        Ok(())
    }
}

/// Open a read-only connection for the TUI stats page at the default path.
pub fn open_readonly() -> Result<Connection> {
    open_readonly_at(&db_path())
}

fn open_readonly_at(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open history db (read-only) {}", db_path.display()))?;
    conn.busy_timeout(Duration::from_secs(1))?;
    Ok(conn)
}

fn unix_secs(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── Aggregate queries (read side) ───────────────────────────────────────────
//
// All take `now_secs` (caller-supplied unix time) so they are deterministic and
// testable rather than depending on SQLite's `now`.

/// Per-day work totals for the last `days` days, newest day first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyWork {
    /// Local date, `YYYY-MM-DD`.
    pub day: String,
    pub busy_secs: i64,
    pub turns: i64,
}

/// Aggregate work for one project over a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectWork {
    pub name: String,
    pub busy_secs: i64,
    pub turns: i64,
}

/// Summary counters over a window (today / this week / …).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowStats {
    /// Number of turns that required the user's input at least once.
    pub interventions: i64,
    /// Mean wait time (seconds) over the turns that required input.
    pub avg_wait_secs: f64,
    /// Total turns in the window.
    pub turns: i64,
    /// Total active (busy) seconds in the window.
    pub busy_secs: i64,
}

/// Daily work buckets for the trailing `days` days (grouped by *local* date).
pub fn daily_work(conn: &Connection, now_secs: i64, days: i64) -> Result<Vec<DailyWork>> {
    let since = now_secs - days * 86_400;
    let mut stmt = conn.prepare(
        "SELECT date(started_at,'unixepoch','localtime') AS day, \
                COALESCE(SUM(busy_secs),0), COUNT(*) \
         FROM turn WHERE started_at >= ?1 \
         GROUP BY day ORDER BY day DESC",
    )?;
    let rows = stmt.query_map([since], |r| {
        Ok(DailyWork {
            day: r.get(0)?,
            busy_secs: r.get(1)?,
            turns: r.get(2)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Top projects by busy time over the trailing `days` days.
pub fn project_breakdown(
    conn: &Connection,
    now_secs: i64,
    days: i64,
    limit: i64,
) -> Result<Vec<ProjectWork>> {
    let since = now_secs - days * 86_400;
    let mut stmt = conn.prepare(
        "SELECT p.name, COALESCE(SUM(t.busy_secs),0) AS secs, COUNT(*) \
         FROM turn t JOIN project p ON p.id=t.project_id \
         WHERE t.started_at >= ?1 \
         GROUP BY p.id ORDER BY secs DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![since, limit], |r| {
        Ok(ProjectWork {
            name: r.get(0)?,
            busy_secs: r.get(1)?,
            turns: r.get(2)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Summary counters for the trailing `days` days.
pub fn window_stats(conn: &Connection, now_secs: i64, days: i64) -> Result<WindowStats> {
    let since = now_secs - days * 86_400;
    let s = conn.query_row(
        "SELECT COALESCE(SUM(needed_input),0), \
                COALESCE(AVG(CASE WHEN needed_input=1 THEN wait_secs END),0), \
                COUNT(*), COALESCE(SUM(busy_secs),0) \
         FROM turn WHERE started_at >= ?1",
        [since],
        |r| {
            Ok(WindowStats {
                interventions: r.get(0)?,
                avg_wait_secs: r.get(1)?,
                turns: r.get(2)?,
                busy_secs: r.get(3)?,
            })
        },
    )?;
    Ok(s)
}

/// Summary counters for *today* (local calendar day of `now_secs`).
pub fn today_stats(conn: &Connection, now_secs: i64) -> Result<WindowStats> {
    let s = conn.query_row(
        "SELECT COALESCE(SUM(needed_input),0), \
                COALESCE(AVG(CASE WHEN needed_input=1 THEN wait_secs END),0), \
                COUNT(*), COALESCE(SUM(busy_secs),0) \
         FROM turn \
         WHERE date(started_at,'unixepoch','localtime') = date(?1,'unixepoch','localtime')",
        [now_secs],
        |r| {
            Ok(WindowStats {
                interventions: r.get(0)?,
                avg_wait_secs: r.get(1)?,
                turns: r.get(2)?,
                busy_secs: r.get(3)?,
            })
        },
    )?;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `ClosedTurn` anchored at a fixed unix second for deterministic tests.
    fn turn(secs: i64, busy: u64, needed: bool, wait: u64, proj: &str) -> ClosedTurn {
        ClosedTurn {
            session_id: "s1".to_string(),
            turn_seq: 1,
            family: AgentFamily::Claude,
            cwd: format!("/p/{proj}"),
            name: proj.to_string(),
            started_at: UNIX_EPOCH + Duration::from_secs(secs as u64),
            ended_at: UNIX_EPOCH + Duration::from_secs(secs as u64 + busy),
            busy_secs: busy,
            needed_input: needed,
            wait_secs: wait,
            incomplete: false,
            tool_counts: BTreeMap::new(),
        }
    }

    // A fixed "now": 2033-05-18 (well after any 1970 anchor we might use); we
    // place test turns relative to it.
    const NOW: i64 = 2_000_000_000;

    fn temp_db() -> (tempfile::TempDir, History) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        let h = History::open_at(&path).unwrap();
        (dir, h)
    }

    #[test]
    fn schema_applies_and_records_turn() {
        let (_d, h) = temp_db();
        h.record_turn(&turn(NOW - 100, 60, false, 0, "alpha"))
            .unwrap();
        let n: i64 = h
            .conn
            .query_row("SELECT COUNT(*) FROM turn", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn project_is_deduped_and_name_updated() {
        let (_d, h) = temp_db();
        // Two turns, same cwd → one project row.
        let mut t1 = turn(NOW - 200, 10, false, 0, "alpha");
        let mut t2 = turn(NOW - 100, 20, false, 0, "alpha");
        t1.name = "old-name".to_string();
        t2.name = "new-name".to_string();
        h.record_turn(&t1).unwrap();
        h.record_turn(&t2).unwrap();

        let count: i64 = h
            .conn
            .query_row("SELECT COUNT(*) FROM project", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "same cwd must map to one project row");
        let name: String = h
            .conn
            .query_row("SELECT name FROM project", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "new-name", "project name should reflect latest turn");
    }

    #[test]
    fn daily_work_groups_and_sums() {
        let (_d, h) = temp_db();
        // Two turns ~same time today, one yesterday.
        h.record_turn(&turn(NOW - 3600, 100, false, 0, "alpha"))
            .unwrap();
        h.record_turn(&turn(NOW - 1800, 200, false, 0, "alpha"))
            .unwrap();
        h.record_turn(&turn(NOW - 86_400 - 1800, 50, false, 0, "alpha"))
            .unwrap();

        let rows = daily_work(&h.conn, NOW, 7).unwrap();
        assert!(rows.len() >= 2, "expected at least two day buckets");
        // Newest day first; today's two turns sum to 300 busy secs.
        assert_eq!(rows[0].turns, 2);
        assert_eq!(rows[0].busy_secs, 300);
    }

    #[test]
    fn project_breakdown_orders_by_busy_desc() {
        let (_d, h) = temp_db();
        h.record_turn(&turn(NOW - 100, 30, false, 0, "small"))
            .unwrap();
        h.record_turn(&turn(NOW - 100, 300, false, 0, "big"))
            .unwrap();
        h.record_turn(&turn(NOW - 100, 70, false, 0, "small"))
            .unwrap();

        let rows = project_breakdown(&h.conn, NOW, 7, 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "big");
        assert_eq!(rows[0].busy_secs, 300);
        assert_eq!(rows[1].name, "small");
        assert_eq!(rows[1].busy_secs, 100, "small project sums to 30+70");
    }

    #[test]
    fn window_stats_counts_interventions_and_avg_wait() {
        let (_d, h) = temp_db();
        // 3 turns: two needed input (wait 10s, 30s), one didn't.
        h.record_turn(&turn(NOW - 300, 60, true, 10, "alpha"))
            .unwrap();
        h.record_turn(&turn(NOW - 200, 60, true, 30, "alpha"))
            .unwrap();
        h.record_turn(&turn(NOW - 100, 60, false, 0, "alpha"))
            .unwrap();

        let s = window_stats(&h.conn, NOW, 7).unwrap();
        assert_eq!(s.turns, 3);
        assert_eq!(s.interventions, 2);
        assert_eq!(s.busy_secs, 180);
        // avg wait over the two needing-input turns = (10+30)/2 = 20.
        assert!((s.avg_wait_secs - 20.0).abs() < 0.001, "got {}", s.avg_wait_secs);
    }

    #[test]
    fn window_respects_time_horizon() {
        let (_d, h) = temp_db();
        h.record_turn(&turn(NOW - 100, 60, false, 0, "recent"))
            .unwrap();
        // 10 days ago — outside a 7-day window.
        h.record_turn(&turn(NOW - 10 * 86_400, 999, false, 0, "old"))
            .unwrap();

        let s = window_stats(&h.conn, NOW, 7).unwrap();
        assert_eq!(s.turns, 1, "turn older than the window must be excluded");
        assert_eq!(s.busy_secs, 60);
    }

    #[test]
    fn tool_counts_persist_as_json() {
        let (_d, h) = temp_db();
        let mut t = turn(NOW - 100, 60, false, 0, "alpha");
        t.tool_counts.insert("Edit".to_string(), 12);
        t.tool_counts.insert("Bash".to_string(), 5);
        h.record_turn(&t).unwrap();

        let json: String = h
            .conn
            .query_row("SELECT tool_counts FROM turn", [], |r| r.get(0))
            .unwrap();
        // BTreeMap → deterministic key order.
        assert_eq!(json, r#"{"Bash":5,"Edit":12}"#);
    }

    #[test]
    fn readonly_connection_reads_written_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        let h = History::open_at(&path).unwrap();
        h.record_turn(&turn(NOW - 100, 42, false, 0, "alpha"))
            .unwrap();

        // A separate read-only connection (the TUI's access path) must see it.
        let ro = open_readonly_at(&path).unwrap();
        let s = window_stats(&ro, NOW, 7).unwrap();
        assert_eq!(s.turns, 1);
        assert_eq!(s.busy_secs, 42);
    }
}
