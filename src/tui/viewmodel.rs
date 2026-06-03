//! Pure view-model: grouping, sorting, selection.
//!
//! Depends only on `SessionView` slices — no I/O, no clocks, fully testable.

use ratatui::style::Color;

use crate::protocol::SessionView;
use crate::tui::theme;

// ── Group ─────────────────────────────────────────────────────────────────────

/// A display group with its ordered rows.
#[derive(Debug, Clone)]
pub struct Group {
    /// Display title for the group header (e.g. "NEEDS INPUT").
    pub title: &'static str,
    /// The colour used to render the group count.
    pub state_color: Color,
    /// Ordered rows within the group.
    pub rows: Vec<SessionView>,
}

impl Group {
    /// Total number of sessions in this group.
    pub fn count(&self) -> usize {
        self.rows.len()
    }
}

// ── group_and_sort ─────────────────────────────────────────────────────────────

/// Group and sort sessions per spec §6:
///   NEEDS INPUT (Approval > Question) > WORKING > IDLE (Done, Idle) > OFFLINE
///
/// Sessions with `stale == true` (remote tunnel disconnected) are always placed
/// in the OFFLINE group regardless of their state, and the OFFLINE group is
/// rendered last. Within each sub-group, rows are sorted by
/// `last_prompt_age_secs` ascending (most-recently-active first).
pub fn group_and_sort(sessions: &[SessionView]) -> Vec<Group> {
    let mut approvals: Vec<SessionView> = Vec::new();
    let mut questions: Vec<SessionView> = Vec::new();
    let mut working: Vec<SessionView> = Vec::new();
    let mut idle_done: Vec<SessionView> = Vec::new();
    let mut offline: Vec<SessionView> = Vec::new();

    for s in sessions {
        if s.stale {
            offline.push(s.clone());
            continue;
        }
        match s.state.as_str() {
            "approval" => approvals.push(s.clone()),
            "question" => questions.push(s.clone()),
            "working" => working.push(s.clone()),
            _ => idle_done.push(s.clone()), // "done", "idle", unknown
        }
    }

    // Within each bucket: most recently active first (smallest secs = most recent)
    sort_by_recency(&mut approvals);
    sort_by_recency(&mut questions);
    sort_by_recency(&mut working);
    sort_by_recency(&mut idle_done);
    sort_by_recency(&mut offline);

    let mut groups: Vec<Group> = Vec::new();

    // NEEDS INPUT: approval rows then question rows in one group
    let mut needs_input_rows = approvals;
    needs_input_rows.extend(questions);
    if !needs_input_rows.is_empty() {
        groups.push(Group {
            title: "NEEDS INPUT",
            state_color: theme::COLOR_APPROVAL, // amber for the group header
            rows: needs_input_rows,
        });
    }

    if !working.is_empty() {
        groups.push(Group {
            title: "WORKING",
            state_color: theme::COLOR_WORKING,
            rows: working,
        });
    }

    if !idle_done.is_empty() {
        groups.push(Group {
            title: "IDLE",
            state_color: theme::COLOR_IDLE,
            rows: idle_done,
        });
    }

    // OFFLINE: stale sessions (remote tunnel disconnected) — always last.
    if !offline.is_empty() {
        groups.push(Group {
            title: "OFFLINE",
            state_color: theme::COLOR_OFFLINE,
            rows: offline,
        });
    }

    groups
}

fn sort_by_recency(rows: &mut [SessionView]) {
    // Order by the user's most recent prompt (smallest age = newest on top),
    // sessions without a prompt last, then id as a stable final tiebreaker.
    // Keyed on prompt time — NOT last_activity — so an agent firing tool hooks
    // every second doesn't constantly jump to the top of its group.
    rows.sort_by(|a, b| {
        a.last_prompt_age_secs
            .unwrap_or(u64::MAX)
            .cmp(&b.last_prompt_age_secs.unwrap_or(u64::MAX))
            .then_with(|| a.id.cmp(&b.id))
    });
}

// ── Selection ─────────────────────────────────────────────────────────────────

/// Tracks which row (flat index across all groups) is currently selected.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Flat index of the currently selected row. `None` when there are no rows.
    pub index: Option<usize>,
}

impl Selection {
    pub fn new() -> Self {
        Selection::default()
    }

    /// Call when the row list changes; clamp or clear the selection.
    pub fn clamp(&mut self, total_rows: usize) {
        self.index = match self.index {
            Some(i) if total_rows > 0 => Some(i.min(total_rows - 1)),
            _ if total_rows > 0 => Some(0),
            _ => None,
        };
    }

    /// Move selection down (wrap at last row).
    pub fn move_down(&mut self, total_rows: usize) {
        if total_rows == 0 {
            self.index = None;
            return;
        }
        self.index = Some(match self.index {
            Some(i) => (i + 1).min(total_rows - 1),
            None => 0,
        });
    }

    /// Move selection up (clamp at 0).
    pub fn move_up(&mut self, total_rows: usize) {
        if total_rows == 0 {
            self.index = None;
            return;
        }
        self.index = Some(match self.index {
            Some(i) => i.saturating_sub(1),
            None => 0,
        });
    }

    /// Move to the next row, wrapping from the last back to the first.
    pub fn cycle_next(&mut self, total_rows: usize) {
        if total_rows == 0 {
            self.index = None;
            return;
        }
        self.index = Some(match self.index {
            Some(i) => (i + 1) % total_rows,
            None => 0,
        });
    }

    /// Move to the previous row, wrapping from the first back to the last.
    pub fn cycle_prev(&mut self, total_rows: usize) {
        if total_rows == 0 {
            self.index = None;
            return;
        }
        self.index = Some(match self.index {
            Some(0) | None => total_rows - 1,
            Some(i) => i - 1,
        });
    }

    /// Returns true if the given flat index is currently selected.
    pub fn is_selected(&self, idx: usize) -> bool {
        self.index == Some(idx)
    }
}

/// Count total selectable rows across all groups.
pub fn total_rows(groups: &[Group]) -> usize {
    groups.iter().map(|g| g.rows.len()).sum()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AgentFamily;

    #[test]
    fn cycle_next_wraps_from_last_to_first() {
        let mut s = Selection { index: Some(2) };
        s.cycle_next(3); // last → first
        assert_eq!(s.index, Some(0));
        s.cycle_next(3);
        assert_eq!(s.index, Some(1));
    }

    #[test]
    fn cycle_prev_wraps_from_first_to_last() {
        let mut s = Selection { index: Some(0) };
        s.cycle_prev(3); // first → last
        assert_eq!(s.index, Some(2));
    }

    #[test]
    fn cycle_handles_empty_and_none() {
        let mut s = Selection { index: Some(0) };
        s.cycle_next(0);
        assert_eq!(s.index, None);
        let mut s2 = Selection { index: None };
        s2.cycle_next(3);
        assert_eq!(s2.index, Some(0));
    }

    fn sv(state: &str, secs: u64) -> SessionView {
        SessionView {
            id: format!("{state}-{secs}"),
            family: AgentFamily::Claude,
            name: format!("repo-{state}"),
            cwd: String::new(),
            state: state.to_string(),
            activity: None,
            last_activity_secs: secs,
            busy_secs: 0,
            last_prompt: None,
            // `secs` represents prompt-recency in these tests (the sort key).
            last_prompt_age_secs: Some(secs),
            host_app: None,
            host_bundle_id: None,
            terminal_session_id: None,
            terminal_tty: None,
            tmux_target: None,
            wezterm_pane: None,
            zellij_session: None,
            workspace_path: None,
            codex_thread_id: None,
            pane_title: None,
            origin: "local".to_string(),
            stale: false,
        }
    }

    // ── group_and_sort ──────────────────────────────────────────────────────

    #[test]
    fn empty_input_yields_no_groups() {
        let groups = group_and_sort(&[]);
        assert!(groups.is_empty());
    }

    #[test]
    fn single_working_session() {
        let sessions = vec![sv("working", 5)];
        let groups = group_and_sort(&sessions);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].title, "WORKING");
        assert_eq!(groups[0].count(), 1);
    }

    #[test]
    fn group_order_needs_input_first() {
        let sessions = vec![sv("idle", 10), sv("working", 5), sv("approval", 2)];
        let groups = group_and_sort(&sessions);
        // NEEDS INPUT (approval) → WORKING → IDLE
        assert_eq!(groups[0].title, "NEEDS INPUT");
        assert_eq!(groups[1].title, "WORKING");
        assert_eq!(groups[2].title, "IDLE");
    }

    #[test]
    fn approval_before_question_in_needs_input() {
        let sessions = vec![sv("question", 1), sv("approval", 3), sv("approval", 2)];
        let groups = group_and_sort(&sessions);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].title, "NEEDS INPUT");
        // First two should be approvals (sorted by recency ascending)
        assert_eq!(groups[0].rows[0].state, "approval");
        assert_eq!(groups[0].rows[1].state, "approval");
        assert_eq!(groups[0].rows[2].state, "question");
    }

    #[test]
    fn within_group_sorted_by_recency_ascending() {
        // smaller last_prompt_age_secs = more recent prompt = comes first
        let sessions = vec![sv("working", 30), sv("working", 5), sv("working", 60)];
        let groups = group_and_sort(&sessions);
        assert_eq!(groups[0].rows[0].last_prompt_age_secs, Some(5));
        assert_eq!(groups[0].rows[1].last_prompt_age_secs, Some(30));
        assert_eq!(groups[0].rows[2].last_prompt_age_secs, Some(60));
    }

    #[test]
    fn order_ignores_last_activity_uses_prompt_time() {
        // A busy agent (last_activity = 0s, firing tool hooks constantly) but with
        // an OLD prompt must NOT jump above a session whose prompt is more recent.
        let mut busy = sv("working", 0); // last_activity_secs = 0 (just fired a hook)
        busy.id = "busy".to_string();
        busy.last_prompt_age_secs = Some(300); // but prompted 5 min ago
        let mut fresh = sv("working", 50); // last_activity older
        fresh.id = "fresh".to_string();
        fresh.last_prompt_age_secs = Some(10); // prompted 10s ago
        let groups = group_and_sort(&[busy, fresh]);
        assert_eq!(groups[0].rows[0].id, "fresh", "most recent prompt on top");
        assert_eq!(groups[0].rows[1].id, "busy");
    }

    #[test]
    fn no_prompt_sessions_sort_last() {
        let mut prompted = sv("working", 100);
        prompted.id = "prompted".to_string();
        let mut never = sv("working", 0);
        never.id = "never".to_string();
        never.last_prompt_age_secs = None; // never prompted
        let groups = group_and_sort(&[never, prompted]);
        assert_eq!(groups[0].rows[0].id, "prompted");
        assert_eq!(groups[0].rows[1].id, "never", "no-prompt session sorts last");
    }

    #[test]
    fn done_and_idle_both_land_in_idle_group() {
        let sessions = vec![sv("done", 10), sv("idle", 20)];
        let groups = group_and_sort(&sessions);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].title, "IDLE");
        assert_eq!(groups[0].count(), 2);
    }

    #[test]
    fn correct_counts() {
        let sessions = vec![
            sv("approval", 1),
            sv("question", 2),
            sv("working", 3),
            sv("working", 4),
            sv("done", 5),
        ];
        let groups = group_and_sort(&sessions);
        let needs = groups.iter().find(|g| g.title == "NEEDS INPUT").unwrap();
        let work = groups.iter().find(|g| g.title == "WORKING").unwrap();
        let idle = groups.iter().find(|g| g.title == "IDLE").unwrap();
        assert_eq!(needs.count(), 2); // approval + question
        assert_eq!(work.count(), 2);
        assert_eq!(idle.count(), 1);
    }

    #[test]
    fn group_state_color_needs_input_is_approval_amber() {
        let sessions = vec![sv("approval", 1)];
        let groups = group_and_sort(&sessions);
        assert_eq!(groups[0].state_color, theme::COLOR_APPROVAL);
    }

    // ── Selection ───────────────────────────────────────────────────────────

    #[test]
    fn selection_empty_list() {
        let mut sel = Selection::new();
        sel.clamp(0);
        assert_eq!(sel.index, None);
    }

    #[test]
    fn selection_clamp_initialises_to_zero() {
        let mut sel = Selection::new();
        sel.clamp(5);
        assert_eq!(sel.index, Some(0));
    }

    #[test]
    fn selection_move_down() {
        let mut sel = Selection::new();
        sel.clamp(3);
        sel.move_down(3);
        assert_eq!(sel.index, Some(1));
        sel.move_down(3);
        assert_eq!(sel.index, Some(2));
        // Clamp at last
        sel.move_down(3);
        assert_eq!(sel.index, Some(2));
    }

    #[test]
    fn selection_move_up() {
        let mut sel = Selection { index: Some(2) };
        sel.move_up(3);
        assert_eq!(sel.index, Some(1));
        sel.move_up(3);
        assert_eq!(sel.index, Some(0));
        // Clamp at 0
        sel.move_up(3);
        assert_eq!(sel.index, Some(0));
    }

    #[test]
    fn selection_move_on_empty_list() {
        let mut sel = Selection::new();
        sel.move_down(0);
        assert_eq!(sel.index, None);
        sel.move_up(0);
        assert_eq!(sel.index, None);
    }

    #[test]
    fn selection_is_selected() {
        let sel = Selection { index: Some(2) };
        assert!(sel.is_selected(2));
        assert!(!sel.is_selected(0));
        assert!(!sel.is_selected(3));
    }

    #[test]
    fn total_rows_counts_across_groups() {
        let sessions = vec![sv("working", 1), sv("done", 2), sv("approval", 3)];
        let groups = group_and_sort(&sessions);
        assert_eq!(total_rows(&groups), 3);
    }

    // ── origin / stale defaults ─────────────────────────────────────────────

    #[test]
    fn session_view_defaults_origin_local_and_not_stale() {
        let s = sv("working", 10);
        assert_eq!(s.origin, "local", "origin should default to local");
        assert!(!s.stale, "stale should default to false");
    }

    // ── OFFLINE group ────────────────────────────────────────────────────────

    fn sv_stale(state: &str, secs: u64) -> SessionView {
        let mut s = sv(state, secs);
        s.stale = true;
        s
    }

    #[test]
    fn stale_session_goes_to_offline_group_not_original_state_group() {
        // A stale working session must NOT appear in WORKING — only in OFFLINE.
        let sessions = vec![sv_stale("working", 5)];
        let groups = group_and_sort(&sessions);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].title, "OFFLINE");
        assert_eq!(groups[0].count(), 1);
    }

    #[test]
    fn stale_approval_goes_to_offline_not_needs_input() {
        let sessions = vec![sv_stale("approval", 2)];
        let groups = group_and_sort(&sessions);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].title, "OFFLINE");
    }

    #[test]
    fn offline_group_is_last_after_all_normal_groups() {
        let sessions = vec![
            sv("approval", 1),
            sv("working", 2),
            sv("idle", 3),
            sv_stale("working", 4),
        ];
        let groups = group_and_sort(&sessions);
        let titles: Vec<&str> = groups.iter().map(|g| g.title).collect();
        assert_eq!(titles, vec!["NEEDS INPUT", "WORKING", "IDLE", "OFFLINE"]);
    }

    #[test]
    fn no_stale_sessions_means_no_offline_group() {
        let sessions = vec![sv("working", 1), sv("idle", 2)];
        let groups = group_and_sort(&sessions);
        assert!(
            groups.iter().all(|g| g.title != "OFFLINE"),
            "OFFLINE group must not appear when there are no stale sessions"
        );
    }

    #[test]
    fn offline_group_state_color_is_warm_grey() {
        let sessions = vec![sv_stale("working", 5)];
        let groups = group_and_sort(&sessions);
        let offline = groups.iter().find(|g| g.title == "OFFLINE").unwrap();
        assert_eq!(offline.state_color, theme::COLOR_OFFLINE);
        assert_eq!(offline.state_color, ratatui::style::Color::Rgb(0xc8, 0x9a, 0x6a));
    }

    #[test]
    fn stale_working_does_not_appear_in_working_group() {
        let sessions = vec![sv("working", 1), sv_stale("working", 2)];
        let groups = group_and_sort(&sessions);
        let working = groups.iter().find(|g| g.title == "WORKING").unwrap();
        // Normal working session is here; stale one must not be.
        assert_eq!(working.count(), 1);
        assert!(
            working.rows.iter().all(|r| !r.stale),
            "WORKING group must not contain stale sessions"
        );
    }

    #[test]
    fn offline_group_sorted_by_recency() {
        let sessions = vec![sv_stale("working", 30), sv_stale("idle", 5)];
        let groups = group_and_sort(&sessions);
        let offline = groups.iter().find(|g| g.title == "OFFLINE").unwrap();
        assert_eq!(offline.count(), 2);
        // smaller last_prompt_age_secs (5) should be first
        assert_eq!(offline.rows[0].last_prompt_age_secs, Some(5));
        assert_eq!(offline.rows[1].last_prompt_age_secs, Some(30));
    }

    #[test]
    fn empty_input_still_yields_no_offline_group() {
        let groups = group_and_sort(&[]);
        assert!(groups.iter().all(|g| g.title != "OFFLINE"));
    }
}
