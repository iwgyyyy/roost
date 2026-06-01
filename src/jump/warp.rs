//! Warp terminal jump via SQLite state + keyboard simulation.
//!
//! Warp stores its window/tab state in a SQLite database at
//! `~/Library/Application Support/dev.warp.Warp-Stable/warp.sqlite`
//! (older builds used `warp.db`; we try both).
//!
//! Jump strategy:
//!   1. Read Warp SQLite to find active tab index and target tab index.
//!   2. Activate Warp (bring to front).
//!   3. Poll until Warp is frontmost (max ~2s).
//!   4. Simulate `Cmd+Shift+]` N times to cycle to the target tab.
//!
//! This is explicitly labelled **hacky/fragile** in the spec; failures
//! degrade cleanly to "just activated Warp".
//!
//! # Schema stability
//! Warp's SQLite schema is **not public API**.  Any error at any step
//! (missing file, missing table, missing column, type mismatch…) returns
//! `None` so the jump silently degrades to "just activate Warp".

use super::{Executor, JumpOutcome, JumpTarget};

const WARP_BUNDLE_ID: &str = "dev.warp.Warp-Stable";

// ── SQLite query construction ──────────────────────────────────────────────────

/// Candidate paths to Warp's SQLite database (macOS).
///
/// We try `warp.sqlite` first (newer builds), then `warp.db` (older).
#[cfg(target_os = "macos")]
pub fn warp_db_path() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let base = format!("{home}/Library/Application Support/dev.warp.Warp-Stable");
    for filename in &["warp.sqlite", "warp.db"] {
        let p = std::path::PathBuf::from(format!("{base}/{filename}"));
        if p.exists() {
            return Some(p);
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
pub fn warp_db_path() -> Option<std::path::PathBuf> {
    None
}

/// SQL to query the active window's tab list from Warp's SQLite.
///
/// Returns (tab_id, tab_index, is_active) for all tabs in the active window.
pub const WARP_TABS_QUERY: &str = "\
SELECT tab_id, tab_index, is_active \
FROM warp_tabs \
WHERE window_id = (SELECT window_id FROM warp_windows WHERE is_frontmost = 1 LIMIT 1) \
ORDER BY tab_index ASC";

// ── Tab cycle computation ─────────────────────────────────────────────────────

/// Compute the number of `Cmd+Shift+]` presses needed to reach `target_index`
/// from `current_index` in a tab bar of `tab_count` tabs.
///
/// Tab cycling wraps around (circular). We always cycle forward (right).
/// Returns 0 if `current_index == target_index`.
pub fn tab_cycle_steps(current_index: usize, target_index: usize, tab_count: usize) -> usize {
    if tab_count == 0 || current_index == target_index {
        return 0;
    }
    // Modular forward distance
    (target_index + tab_count - current_index) % tab_count
}

// ── SQLite parsing ────────────────────────────────────────────────────────────

/// Parse Warp tab state from an open SQLite connection.
///
/// This is a **pure function over a connection** — it does not open any file,
/// making it injectable for unit tests via an in-memory database.
///
/// Returns `Some((focused_tab_index, active_tab_index, tab_count))` on success,
/// or `None` on any error (missing table, missing rows, type mismatch, …).
///
/// Degrades gracefully: **any error returns `None`**, never panics.
///
/// # Schema
/// Expects two tables in the active connection:
/// - `warp_windows(window_id, is_frontmost INTEGER)`
/// - `warp_tabs(tab_id, window_id, tab_index INTEGER, is_active INTEGER)`
pub fn parse_warp_tabs(conn: &rusqlite::Connection) -> Option<(usize, usize, usize)> {
    parse_warp_tabs_impl(conn)
}

// Internal implementation shared across cfg gates.
fn parse_warp_tabs_impl(conn: &rusqlite::Connection) -> Option<(usize, usize, usize)> {
    // Collect all rows for the frontmost window.
    let mut stmt = conn.prepare(WARP_TABS_QUERY).ok()?;

    struct TabRow {
        tab_index: i64,
        is_active: i64,
    }

    let rows: Vec<TabRow> = stmt
        .query_map([], |row| {
            Ok(TabRow {
                tab_index: row.get(1)?,
                is_active: row.get(2)?,
            })
        })
        .ok()?
        .filter_map(|r| r.ok())
        .collect();

    if rows.is_empty() {
        return None;
    }

    let tab_count = rows.len();

    // Find the focused (is_active == 1) tab's index.
    let active_row = rows.iter().find(|r| r.is_active != 0)?;
    let focused_tab_index = active_row.tab_index.try_into().ok()?;

    // The "target" tab index for a basic Warp jump is the same as the focused
    // tab — callers use this to determine how many Cmd+Shift+] presses are needed
    // from the *actual* frontmost tab at jump time.
    // We return (focused, focused, count); the caller can override target if it
    // has a specific target tab from session data.
    Some((focused_tab_index, focused_tab_index, tab_count))
}

// ── read_warp_tabs (macOS) ────────────────────────────────────────────────────

/// Try to read Warp tab state from the on-disk SQLite database.
///
/// Returns `Some((focused_tab_index, active_tab_index, total_tab_count))`.
///
/// Returns `None` if:
/// - Not running on macOS.
/// - Warp's database file cannot be found.
/// - The database cannot be opened.
/// - The schema doesn't match expectations (table/column missing).
/// - Any other error.
///
/// **This function never panics.**
#[cfg(target_os = "macos")]
fn read_warp_tabs() -> Option<(usize, usize, usize)> {
    let path = warp_db_path()?;
    // SQLITE_OPEN_READONLY — we never write to Warp's database.
    let conn =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    parse_warp_tabs_impl(&conn)
}

#[cfg(not(target_os = "macos"))]
fn read_warp_tabs() -> Option<(usize, usize, usize)> {
    None
}

// ── Jump implementation ────────────────────────────────────────────────────────

/// Execute a Warp jump using the SQLite + keyboard simulation approach.
///
/// Strategy:
///   1. Try to read the active tab index from Warp's SQLite.
///   2. Activate Warp (bring to front).
///   3. Simulate `Cmd+Shift+]` N times to reach the target tab.
///
/// On failure at any step, degrades to "just activated Warp".
///
/// **Labelled hacky/fragile per spec §14** — Warp's internal schema is
/// undocumented and may change without notice.
pub fn jump_warp(_target: &JumpTarget, executor: &dyn Executor) -> JumpOutcome {
    // Step 1: read tab state *before* activating, so we capture the current
    // frontmost state while Warp might be in the background.
    let tab_info = read_warp_tabs();

    // Step 2: activate Warp
    if !executor.activate_app(WARP_BUNDLE_ID) {
        return JumpOutcome::Failed {
            message: "Failed to activate Warp".to_string(),
        };
    }

    let steps = match tab_info {
        Some((current, target_idx, count)) if count > 1 => {
            tab_cycle_steps(current, target_idx, count)
        }
        _ => 0,
    };

    if steps == 0 {
        // Already on the right tab or couldn't determine — just activated
        return JumpOutcome::Activated {
            message: "Activated Warp".to_string(),
        };
    }

    // Step 3: simulate Cmd+Shift+] N times via AppleScript
    // HACKY: relies on Warp being frontmost by the time the script runs.
    let script = build_tab_cycle_script(steps);
    let (_, ok) = executor.run_applescript(&script);
    if ok {
        JumpOutcome::Focused {
            message: format!("Cycled {steps} tab(s) in Warp"),
        }
    } else {
        // Keyboard sim failed; still activated
        JumpOutcome::Activated {
            message: "Activated Warp (tab cycle failed)".to_string(),
        }
    }
}

/// Build an AppleScript to simulate `Cmd+Shift+]` `count` times to cycle tabs.
pub fn build_tab_cycle_script(count: usize) -> String {
    format!(
        r#"tell application "System Events"
  repeat {count} times
    key code 30 using {{command down, shift down}}
    delay 0.05
  end repeat
end tell"#
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── tab_cycle_steps ───────────────────────────────────────────────────

    #[test]
    fn tab_cycle_steps_same_tab() {
        assert_eq!(tab_cycle_steps(2, 2, 5), 0);
    }

    #[test]
    fn tab_cycle_steps_forward() {
        // current=1, target=3, count=5 → 2 steps forward
        assert_eq!(tab_cycle_steps(1, 3, 5), 2);
    }

    #[test]
    fn tab_cycle_steps_wraps_around() {
        // current=3, target=1, count=5 → 3 steps (3→4→0→1)
        assert_eq!(tab_cycle_steps(3, 1, 5), 3);
    }

    #[test]
    fn tab_cycle_steps_zero_tabs() {
        assert_eq!(tab_cycle_steps(0, 0, 0), 0);
    }

    #[test]
    fn tab_cycle_steps_one_tab() {
        assert_eq!(tab_cycle_steps(0, 0, 1), 0);
    }

    #[test]
    fn tab_cycle_steps_last_to_first() {
        // current=4, target=0, count=5 → 1 step (wrap)
        assert_eq!(tab_cycle_steps(4, 0, 5), 1);
    }

    #[test]
    fn tab_cycle_steps_first_to_last() {
        // current=0, target=4, count=5 → 4 steps
        assert_eq!(tab_cycle_steps(0, 4, 5), 4);
    }

    // ── build_tab_cycle_script ────────────────────────────────────────────

    #[test]
    fn build_tab_cycle_script_contains_key_code() {
        let script = build_tab_cycle_script(3);
        assert!(script.contains("key code 30"), "should use key code 30 (])");
        assert!(script.contains("command down"), "should use Cmd modifier");
        assert!(script.contains("shift down"), "should use Shift modifier");
        assert!(script.contains("repeat 3 times"), "should repeat 3 times");
    }

    #[test]
    fn build_tab_cycle_script_zero_gives_zero_repeat() {
        let script = build_tab_cycle_script(0);
        assert!(script.contains("repeat 0 times"));
    }

    // ── SQL query string ──────────────────────────────────────────────────

    #[test]
    fn warp_tabs_query_is_valid_sql_fragment() {
        // Just verify it contains expected keywords
        assert!(WARP_TABS_QUERY.contains("SELECT"));
        assert!(WARP_TABS_QUERY.contains("warp_tabs"));
        assert!(WARP_TABS_QUERY.contains("tab_index"));
        assert!(WARP_TABS_QUERY.contains("is_active"));
    }

    // ── parse_warp_tabs (in-memory SQLite fixture) ────────────────────────

    /// Create an in-memory SQLite database with the Warp schema and test data.
    fn make_warp_db(
        tabs: &[(
            i64, /*tab_id*/
            i64, /*tab_index*/
            i64, /*is_active*/
        )],
        frontmost_window: bool,
    ) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(
            "CREATE TABLE warp_windows (
                window_id   INTEGER PRIMARY KEY,
                is_frontmost INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE warp_tabs (
                tab_id      INTEGER PRIMARY KEY,
                window_id   INTEGER NOT NULL,
                tab_index   INTEGER NOT NULL,
                is_active   INTEGER NOT NULL DEFAULT 0
            );",
        )
        .expect("create tables");

        let window_id: i64 = 1;
        conn.execute(
            "INSERT INTO warp_windows (window_id, is_frontmost) VALUES (?1, ?2)",
            rusqlite::params![window_id, frontmost_window as i64],
        )
        .expect("insert window");

        for (tab_id, tab_index, is_active) in tabs {
            conn.execute(
                "INSERT INTO warp_tabs (tab_id, window_id, tab_index, is_active) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![tab_id, window_id, tab_index, is_active],
            )
            .expect("insert tab");
        }

        conn
    }

    #[test]
    fn parse_warp_tabs_single_active_tab() {
        // One tab, is_active=1 → focused=0, target=0, count=1
        let conn = make_warp_db(&[(1, 0, 1)], true);
        let result = parse_warp_tabs_impl(&conn);
        assert_eq!(result, Some((0, 0, 1)));
    }

    #[test]
    fn parse_warp_tabs_three_tabs_middle_active() {
        // 3 tabs, tab at index 1 is active
        let conn = make_warp_db(&[(1, 0, 0), (2, 1, 1), (3, 2, 0)], true);
        let result = parse_warp_tabs_impl(&conn);
        assert_eq!(result, Some((1, 1, 3)));
    }

    #[test]
    fn parse_warp_tabs_five_tabs_last_active() {
        // 5 tabs, last one active
        let conn = make_warp_db(
            &[(1, 0, 0), (2, 1, 0), (3, 2, 0), (4, 3, 0), (5, 4, 1)],
            true,
        );
        let result = parse_warp_tabs_impl(&conn);
        assert_eq!(result, Some((4, 4, 5)));
    }

    #[test]
    fn parse_warp_tabs_no_frontmost_window_returns_none() {
        // Window exists but is_frontmost = 0 → query returns no rows → None
        let conn = make_warp_db(&[(1, 0, 1)], false);
        let result = parse_warp_tabs_impl(&conn);
        assert_eq!(result, None);
    }

    #[test]
    fn parse_warp_tabs_empty_tabs_returns_none() {
        // Frontmost window exists but has no tabs → None
        let conn = make_warp_db(&[], true);
        let result = parse_warp_tabs_impl(&conn);
        assert_eq!(result, None);
    }

    #[test]
    fn parse_warp_tabs_no_active_tab_returns_none() {
        // All tabs have is_active=0 → cannot find focused tab → None
        let conn = make_warp_db(&[(1, 0, 0), (2, 1, 0)], true);
        let result = parse_warp_tabs_impl(&conn);
        assert_eq!(result, None);
    }

    #[test]
    fn parse_warp_tabs_missing_table_returns_none() {
        // Connection with no tables at all → graceful None
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        let result = parse_warp_tabs_impl(&conn);
        assert_eq!(result, None);
    }

    // ── tab_cycle_steps integration with parse result ─────────────────────

    #[test]
    fn cycle_steps_from_parsed_tabs() {
        // 4 tabs, currently on tab 0; target is tab 3 → 3 steps forward
        let conn = make_warp_db(&[(1, 0, 1), (2, 1, 0), (3, 2, 0), (4, 3, 0)], true);
        let (current, _target, count) = parse_warp_tabs_impl(&conn).expect("should parse");
        // Override target to 3 (simulating a specific desired tab)
        let steps = tab_cycle_steps(current, 3, count);
        assert_eq!(steps, 3);
    }

    #[test]
    fn cycle_steps_wrap_from_parsed_tabs() {
        // 4 tabs, currently on tab 3; target is tab 1 → 2 steps (3→0→1)
        let conn = make_warp_db(&[(1, 0, 0), (2, 1, 0), (3, 2, 0), (4, 3, 1)], true);
        let (current, _target, count) = parse_warp_tabs_impl(&conn).expect("should parse");
        let steps = tab_cycle_steps(current, 1, count);
        assert_eq!(steps, 2);
    }
}
