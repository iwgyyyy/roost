//! Peek detail panel — renders a rounded-corner box overlaying the list.
//!
//! Design spec §7 (design_handoff README §7):
//!
//! ```text
//! ╭─ peek · payments-api ──────────────────────────────────────╮
//! │  agent    ✻ Claude Code                                     │
//! │  path     ~/dev/acme/payments-api                           │
//! │  status   ▲ Needs input · Approval                          │
//! │  action   run: git push --force origin main                 │
//! │  since    waiting for your approval · 8s                    │
//! │                                                             │
//! │  recent                                                     │
//! │      8s  ▲ asked to run git push --force                    │
//! │     40s  ✎ edited 3 files in src/                           │
//! │      1m  ⚙ ran test suite · 142 passed                      │
//! ╰─ esc close ────────────────────────────────────────────────╯
//! ```
//!
//! All text truncation uses unicode-width to handle CJK correctly.
//! Right border is placed at exactly `area.x + area.width - 1`.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use crate::protocol::SessionDetail;
use crate::tui::{
    anim,
    layout::{pad_to_width, truncate_to_width},
    theme::{self, COLOR_ACCENT, Theme},
};

// ── render_peek ───────────────────────────────────────────────────────────────

/// Render the peek detail panel into `buf` using `area`.
///
/// `detail` is the session detail to display.
/// If `area` is too small for the minimum layout, renders nothing (caller
/// should switch to full-screen mode).
/// Number of recent-event rows that fit in a peek panel of the given height.
/// `has_action` accounts for the optional `action` field taking one extra row.
/// Mirrors the fixed "chrome" rows rendered by [`render_peek`].
pub fn recent_rows_capacity(area_height: u16, has_action: bool) -> usize {
    // header(1) + agent/path/status/since(4) + action(0|1) + blank(1) + recent-label(1) + footer(1)
    let chrome = 1 + 4 + has_action as u16 + 1 + 1 + 1;
    area_height.saturating_sub(chrome) as usize
}

/// `recent_scroll` is the number of recent events to skip from the top of the
/// timeline (0 = newest at top). The caller clamps it to a valid range.
pub fn render_peek(
    buf: &mut Buffer,
    area: Rect,
    detail: &SessionDetail,
    theme: &Theme,
    recent_scroll: usize,
) {
    if area.height < 4 || area.width < 20 {
        return;
    }

    let w = area.width as usize;
    let x0 = area.x;
    let y0 = area.y;
    let right = area.x + area.width - 1; // last column (right border)

    // ── Header line: ╭─ peek · <name> ───╮ ────────────────────────────────
    {
        let left = "╭─ peek · ";
        // One trailing space after the label keeps style consistent with the
        // main list header ("╭─ roost ───") which also has a space before fill.
        let label_suffix = " ";
        let right_cap = "─╮";
        // Title shows the repo / cwd basename (detail.name)
        let session_label = &detail.name;
        let overhead = UnicodeWidthStr::width(left)
            + UnicodeWidthStr::width(label_suffix)
            + UnicodeWidthStr::width(right_cap);
        let label_max = w.saturating_sub(overhead);
        let label = truncate_to_width(session_label, label_max);
        let fill_avail = w
            .saturating_sub(UnicodeWidthStr::width(left))
            .saturating_sub(UnicodeWidthStr::width(label.as_str()))
            .saturating_sub(UnicodeWidthStr::width(label_suffix))
            .saturating_sub(UnicodeWidthStr::width(right_cap));
        let fill = "─".repeat(fill_avail);
        let header = format!("{left}{label}{label_suffix}{fill}{right_cap}");
        buf.set_string(x0, y0, &header, Style::default().fg(theme.border));
        // "peek" keyword in accent (semantic — brand colour).
        buf.set_string(
            x0 + UnicodeWidthStr::width("╭─ ") as u16,
            y0,
            "peek",
            Style::default().fg(COLOR_ACCENT),
        );
        // Repo name in terminal default fg (adaptive, readable on light & dark).
        buf.set_string(
            x0 + UnicodeWidthStr::width(left) as u16,
            y0,
            label.as_str(),
            Style::default().fg(Color::Reset),
        );
    }

    // ── Body rows ──────────────────────────────────────────────────────────
    // inner content width: area.width - 4  (│ + space + … + space + │)
    let inner_w = (w as i32 - 4).max(0) as usize;
    let inner_x = x0 + 2; // after "│ "

    let mut row = y0 + 1; // current row being written
    let footer_y = area.y + area.height - 1;

    // Helper: render one body row — uniform style for entire content.
    let render_body_row = |buf: &mut Buffer, y: u16, content: &str, style: Style| {
        if y >= footer_y {
            return;
        }
        buf.set_string(x0, y, "│", Style::default().fg(theme.border));
        buf.set_string(right, y, "│", Style::default().fg(theme.border));
        let truncated = truncate_to_width(content, inner_w);
        let padded = pad_to_width(&truncated, inner_w);
        buf.set_string(inner_x, y, &padded, style);
    };

    // Helper: render a labelled field row with split colours.
    //   label   — fixed-width prefix, rendered in DarkGray (structural).
    //   value   — rest of the row, rendered in `value_style` (semantic or Reset).
    // The combined width is constrained to inner_w and padded with spaces.
    let render_field_row =
        |buf: &mut Buffer, y: u16, label: &str, value: &str, value_style: Style| {
            if y >= footer_y {
                return;
            }
            buf.set_string(x0, y, "│", Style::default().fg(theme.border));
            buf.set_string(right, y, "│", Style::default().fg(theme.border));
            let label_w = UnicodeWidthStr::width(label);
            let val_avail = inner_w.saturating_sub(label_w);
            let val_trunc = truncate_to_width(value, val_avail);
            let val_padded = pad_to_width(&val_trunc, val_avail);
            // Write label in dim grey (structural chrome).
            buf.set_string(inner_x, y, label, Style::default().fg(Color::DarkGray));
            // Write value in caller-supplied style (semantic or terminal default).
            buf.set_string(inner_x + label_w as u16, y, &val_padded, value_style);
        };

    // ── Field: agent ──
    // Label in DarkGray; value (icon + agent name) in family colour (semantic RGB).
    if row < footer_y {
        let (_, icon_color) = theme::family_icon(&detail.family);
        render_field_row(
            buf,
            row,
            "agent   ",
            &detail.agent_display,
            Style::default().fg(icon_color),
        );
        row += 1;
    }

    // ── Field: path ──
    // Label in DarkGray; value in terminal default fg (adaptive).
    if row < footer_y {
        render_field_row(
            buf,
            row,
            "path    ",
            &detail.cwd,
            Style::default().fg(Color::Reset),
        );
        row += 1;
    }

    // ── Field: status ──
    // Label in DarkGray; value (glyph + label) in state colour (semantic RGB).
    if row < footer_y {
        let (glyph, color) = theme::state_glyph(&detail.state);
        let label = state_full_label(&detail.state);
        let value = format!("{glyph} {label}");
        render_field_row(buf, row, "status  ", &value, Style::default().fg(color));
        row += 1;
    }

    // ── Field: action ──
    // Label in DarkGray; value in terminal default fg.
    if row < footer_y
        && let Some(action) = &detail.action
    {
        render_field_row(
            buf,
            row,
            "action  ",
            action,
            Style::default().fg(Color::Reset),
        );
        row += 1;
    }

    // ── Field: since (waiting reason + duration) ──
    // Label in DarkGray; value in terminal default fg.
    if row < footer_y {
        let time_str = anim::relative_time(detail.since_secs);
        let since_value = if let Some(reason) = &detail.waiting_reason {
            format!("{reason} · {time_str}")
        } else {
            time_str
        };
        render_field_row(
            buf,
            row,
            "since   ",
            &since_value,
            Style::default().fg(Color::Reset),
        );
        row += 1;
    }

    // ── Blank spacer ──
    if row < footer_y {
        render_body_row(buf, row, "", Style::default());
        row += 1;
    }

    // ── Field: recent events label ──
    // Section heading: DarkGray + Bold. When the timeline overflows the panel,
    // append a "first–last/total" position so scrolling has a visible anchor.
    if row < footer_y {
        let total = detail.recent_events.len();
        let cap = recent_rows_capacity(area.height, detail.action.is_some());
        let label = if total > cap && cap > 0 {
            let first = recent_scroll + 1;
            let last = (recent_scroll + cap).min(total);
            format!("recent  {first}–{last}/{total}")
        } else {
            "recent".to_string()
        };
        render_body_row(
            buf,
            row,
            &label,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
        row += 1;
    }

    // ── Recent events timeline (reverse-chron, newest first) ──
    // Completed steps show their FROZEN duration (how long the step ran). The
    // most-recent event (index 0) has no successor yet, so its duration is not
    // determined — it shows no time prefix (blank, kept aligned) rather than a
    // live counter that would tick every second. The "since" field above
    // already tracks the current step's time live.
    for ev in detail.recent_events.iter().skip(recent_scroll) {
        if row >= footer_y {
            break;
        }
        let summary = &ev.summary;
        let content = if let Some(dur) = ev.duration_secs {
            // Completed step: FROZEN duration (how long the step ran).
            let dur_str = anim::relative_time(dur);
            format!("{dur_str:>6}  {summary}")
        } else {
            // Newest step: duration not yet known — no time prefix (blank pad).
            format!("{:>6}  {summary}", "")
        };
        render_body_row(buf, row, &content, Style::default().fg(Color::DarkGray));
        row += 1;
    }

    // ── Fill remaining body rows with empty border lines ──
    while row < footer_y {
        render_body_row(buf, row, "", Style::default());
        row += 1;
    }

    // ── Footer: ╰─ esc close ───╯ ─────────────────────────────────────────
    {
        let left = "╰─ ";
        let keys = "↑/↓ scroll · tab next · esc close";
        // One space after keys before fill, matching header style.
        let keys_suffix = " ";
        let right_cap = "─╯";
        let overhead = UnicodeWidthStr::width(left)
            + UnicodeWidthStr::width(keys)
            + UnicodeWidthStr::width(keys_suffix)
            + UnicodeWidthStr::width(right_cap);
        let fill_avail = w.saturating_sub(overhead);
        let fill = "─".repeat(fill_avail);
        let footer = format!("{left}{keys}{keys_suffix}{fill}{right_cap}");
        buf.set_string(x0, footer_y, &footer, Style::default().fg(theme.border));
        // Key hint in dim grey (structural chrome).
        buf.set_string(
            x0 + UnicodeWidthStr::width(left) as u16,
            footer_y,
            keys,
            Style::default().fg(Color::DarkGray),
        );
    }
}

// ── State label ────────────────────────────────────────────────────────────────

fn state_full_label(state: &str) -> &'static str {
    match state {
        "approval" => "Needs input · Approval",
        "question" => "Needs input · Question",
        "working" => "Working",
        "done" => "Done",
        "idle" => "Idle",
        _ => "Unknown",
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{AgentFamily, Event};
    use ratatui::{Terminal, backend::TestBackend};

    fn make_detail() -> SessionDetail {
        SessionDetail {
            id: "payments-api".to_string(),
            family: AgentFamily::Claude,
            name: "payments-api".to_string(),
            agent_display: "✻ Claude Code".to_string(),
            cwd: "~/dev/acme/payments-api".to_string(),
            state: "approval".to_string(),
            action: Some("run: git push --force origin main".to_string()),
            waiting_reason: Some("waiting for your approval".to_string()),
            since_secs: 8,
            recent_events: vec![
                Event {
                    ts_secs: 8,
                    summary: "▲ asked to run git push --force".to_string(),
                    duration_secs: None,
                },
                Event {
                    ts_secs: 40,
                    summary: "✎ edited 3 files in src/".to_string(),
                    duration_secs: Some(32),
                },
                Event {
                    ts_secs: 60,
                    summary: "⚙ ran test suite".to_string(),
                    duration_secs: Some(20),
                },
            ],
        }
    }

    /// Build a detail with N uniquely-named recent events and no `action` row.
    fn make_detail_with_events(n: usize) -> SessionDetail {
        let mut d = make_detail();
        d.action = None;
        d.recent_events = (0..n)
            .map(|i| Event {
                ts_secs: i as u64,
                summary: format!("EV{i:02}"),
                duration_secs: if i == 0 { None } else { Some(i as u64) },
            })
            .collect();
        d
    }

    fn cell_symbol(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> String {
        buf.cell((x, y))
            .map(|c| c.symbol().to_string())
            .unwrap_or_default()
    }

    fn row_string(buf: &ratatui::buffer::Buffer, width: u16, y: u16) -> String {
        (0..width).map(|x| cell_symbol(buf, x, y)).collect()
    }

    fn render_peek_to_buf(
        width: u16,
        height: u16,
        detail: &SessionDetail,
    ) -> ratatui::buffer::Buffer {
        let theme = Theme::classic();
        let backend = TestBackend::new(width, height);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let area = frame.area();
            render_peek(frame.buffer_mut(), area, detail, &theme, 0);
        })
        .unwrap();
        term.backend().buffer().clone()
    }

    fn render_peek_scrolled(
        width: u16,
        height: u16,
        detail: &SessionDetail,
        scroll: usize,
    ) -> ratatui::buffer::Buffer {
        let theme = Theme::classic();
        let backend = TestBackend::new(width, height);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|frame| {
            let area = frame.area();
            render_peek(frame.buffer_mut(), area, detail, &theme, scroll);
        })
        .unwrap();
        term.backend().buffer().clone()
    }

    fn buf_text(buf: &ratatui::buffer::Buffer, width: u16, height: u16) -> String {
        (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .map(|(x, y)| cell_symbol(buf, x, y))
            .collect()
    }

    // ── Scrolling ─────────────────────────────────────────────────────────────

    #[test]
    fn recent_rows_capacity_accounts_for_chrome() {
        // chrome = header + 4 fields + blank + recent-label + footer = 8 (no action)
        assert_eq!(recent_rows_capacity(20, false), 12);
        // +1 for the action row
        assert_eq!(recent_rows_capacity(20, true), 11);
        // Too small for any recent rows
        assert_eq!(recent_rows_capacity(6, false), 0);
    }

    #[test]
    fn peek_scroll_reveals_older_events() {
        // 8 events, a small panel: scroll=0 shows the newest, scrolling reveals
        // older ones and drops the newest from view.
        let detail = make_detail_with_events(8);
        let (w, h) = (60u16, 12u16); // capacity = 12 - 8 = 4 recent rows

        let top = render_peek_scrolled(w, h, &detail, 0);
        let top_text = buf_text(&top, w, h);
        assert!(top_text.contains("EV00"), "scroll=0 should show newest EV00");
        assert!(
            !top_text.contains("EV06"),
            "scroll=0 should not yet show old EV06"
        );

        let scrolled = render_peek_scrolled(w, h, &detail, 4);
        let scrolled_text = buf_text(&scrolled, w, h);
        assert!(
            !scrolled_text.contains("EV00"),
            "after scrolling 4, the newest EV00 should be out of view"
        );
        assert!(
            scrolled_text.contains("EV04"),
            "after scrolling 4, EV04 should be visible"
        );
    }

    // ── Border alignment ────────────────────────────────────────────────────

    #[test]
    fn peek_header_right_border_at_last_col() {
        let width = 60u16;
        let height = 14u16;
        let detail = make_detail();
        let buf = render_peek_to_buf(width, height, &detail);

        let last_col = width - 1;
        let sym = cell_symbol(&buf, last_col, 0);
        assert_eq!(
            sym,
            "╮",
            "header right border ╮ should be at col {last_col} (width={width}), got {sym:?}\nrow: {:?}",
            row_string(&buf, width, 0)
        );
    }

    #[test]
    fn peek_footer_right_border_at_last_col() {
        let width = 60u16;
        let height = 14u16;
        let detail = make_detail();
        let buf = render_peek_to_buf(width, height, &detail);

        let last_col = width - 1;
        let footer_y = height - 1;
        let sym = cell_symbol(&buf, last_col, footer_y);
        assert_eq!(
            sym,
            "╯",
            "footer right border ╯ should be at col {last_col} (width={width}), got {sym:?}\nrow: {:?}",
            row_string(&buf, width, footer_y)
        );
    }

    #[test]
    fn peek_header_right_border_at_73_cols() {
        let width = 73u16;
        let height = 16u16;
        let detail = make_detail();
        let buf = render_peek_to_buf(width, height, &detail);

        let last_col = width - 1;
        let sym = cell_symbol(&buf, last_col, 0);
        assert_eq!(
            sym,
            "╮",
            "header ╮ at col {last_col} (width=73), got {sym:?}\nrow: {:?}",
            row_string(&buf, width, 0)
        );
    }

    // ── Content ─────────────────────────────────────────────────────────────

    #[test]
    fn peek_contains_session_id_in_header() {
        let detail = make_detail();
        let buf = render_peek_to_buf(70, 16, &detail);
        let header = row_string(&buf, 70, 0);
        assert!(
            header.contains("payments-api"),
            "header should contain session id, got: {header:?}"
        );
    }

    // ── Task-1: name semantics ───────────────────────────────────────────────

    /// Header title must show the repo name (detail.name), not the agent label.
    #[test]
    fn peek_title_shows_repo_name() {
        let mut detail = make_detail();
        detail.name = "my-service".to_string();
        detail.agent_display = "✻ Claude Code".to_string();
        let buf = render_peek_to_buf(70, 16, &detail);
        let header = row_string(&buf, 70, 0);
        assert!(
            header.contains("my-service"),
            "header should contain repo name 'my-service', got: {header:?}"
        );
        assert!(
            !header.contains("Claude Code"),
            "header should NOT contain agent label, got: {header:?}"
        );
    }

    /// Agent row must show the icon + full label (detail.agent_display).
    #[test]
    fn peek_agent_row_shows_icon_and_label() {
        let mut detail = make_detail();
        detail.name = "my-service".to_string();
        detail.agent_display = "✻ Claude Code".to_string();
        let buf = render_peek_to_buf(70, 16, &detail);
        let content: String = (0..16)
            .flat_map(|y| (0..70u16).map(move |x| (x, y)))
            .map(|(x, y)| cell_symbol(&buf, x, y))
            .collect();
        assert!(
            content.contains("Claude Code"),
            "agent row should contain 'Claude Code', got content snippet"
        );
        assert!(content.contains("✻"), "agent row should contain '✻' icon");
    }

    #[test]
    fn peek_contains_path_field() {
        let detail = make_detail();
        let buf = render_peek_to_buf(70, 16, &detail);
        let content: String = (0..16)
            .flat_map(|y| (0..70u16).map(move |x| (x, y)))
            .map(|(x, y)| cell_symbol(&buf, x, y))
            .collect();
        assert!(
            content.contains("path"),
            "peek should contain 'path' field, content: {content:?}"
        );
        assert!(
            content.contains("acme/payments-api"),
            "peek should contain cwd path"
        );
    }

    #[test]
    fn peek_contains_recent_events() {
        let detail = make_detail();
        let buf = render_peek_to_buf(70, 18, &detail);
        let content: String = (0..18)
            .flat_map(|y| (0..70u16).map(move |x| (x, y)))
            .map(|(x, y)| cell_symbol(&buf, x, y))
            .collect();
        assert!(
            content.contains("recent"),
            "peek should contain 'recent' label"
        );
        assert!(
            content.contains("git push"),
            "peek should contain event summary text"
        );
    }

    #[test]
    fn peek_contains_esc_close_in_footer() {
        let detail = make_detail();
        let buf = render_peek_to_buf(60, 14, &detail);
        let footer = row_string(&buf, 60, 13);
        assert!(
            footer.contains("esc close"),
            "footer should contain 'esc close', got: {footer:?}"
        );
    }

    #[test]
    fn peek_left_border_on_every_row() {
        let width = 60u16;
        let height = 12u16;
        let detail = make_detail();
        let buf = render_peek_to_buf(width, height, &detail);

        // Every row should start with a border character
        for y in 0..height {
            let sym = cell_symbol(&buf, 0, y);
            assert!(
                sym == "╭" || sym == "│" || sym == "╰",
                "row {y} left border should be border char, got: {sym:?}"
            );
        }
    }

    #[test]
    fn peek_right_border_on_every_row() {
        let width = 60u16;
        let height = 12u16;
        let detail = make_detail();
        let buf = render_peek_to_buf(width, height, &detail);

        let last_col = width - 1;
        for y in 0..height {
            let sym = cell_symbol(&buf, last_col, y);
            assert!(
                sym == "╮" || sym == "│" || sym == "╯",
                "row {y} right border (col {last_col}) should be border char, got: {sym:?}"
            );
        }
    }

    /// Visual dump test — run with `-- --nocapture` to inspect.
    #[test]
    fn dump_peek_inline_ascii() {
        let width = 70u16;
        let height = 16u16;
        let detail = make_detail();
        let buf = render_peek_to_buf(width, height, &detail);
        println!("\n=== peek inline dump ({}×{}) ===", width, height);
        for y in 0..height {
            println!("|{}|", row_string(&buf, width, y));
        }
        println!("=== end peek inline dump ===\n");
    }

    /// Visual dump for full-screen peek.
    #[test]
    fn dump_peek_fullscreen_ascii() {
        let width = 80u16;
        let height = 24u16;
        let detail = make_detail();
        let buf = render_peek_to_buf(width, height, &detail);
        println!("\n=== peek full-screen dump ({}×{}) ===", width, height);
        for y in 0..height {
            println!("|{}|", row_string(&buf, width, y));
        }
        println!("=== end peek full-screen dump ===\n");
    }

    /// Combined dump: main list (upper half) + inline peek (lower half).
    /// Simulates the inline split layout used when peek is open.
    #[test]
    fn dump_combined_list_plus_inline_peek() {
        use crate::protocol::{AgentFamily, SessionView};
        use crate::tui::{
            anim::Anim,
            layout::split_for_inline_peek,
            render::{RenderState, render_frame},
            viewmodel::{Selection, group_and_sort},
        };
        use ratatui::layout::Rect;

        let width = 80u16;
        let height = 24u16;
        let detail = make_detail();
        let theme = Theme::classic();

        // Build a one-session list
        let sessions = vec![SessionView {
            id: "payments-api".to_string(),
            family: AgentFamily::Claude,
            name: "payments-api".to_string(),
            state: "approval".to_string(),
            activity: Some("run: git push --force".to_string()),
            last_activity_secs: 8,
            busy_secs: 0,
            last_prompt: None,
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
        }];
        let groups = group_and_sort(&sessions);
        let selection = Selection { index: Some(0) };
        let anim = Anim::new();

        let backend = TestBackend::new(width, height);
        let mut term = Terminal::new(backend).unwrap();

        term.draw(|frame| {
            let full_area = Rect::new(0, 0, width, height);
            let (list_area, peek_area) = split_for_inline_peek(full_area);

            // Render list in upper half
            let list_state = RenderState {
                groups: &groups,
                selection: &selection,
                daemon_ready: true,
                anim: &anim,
                theme: &theme,
                scroll_offset: 0,
                peek_open: true,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), list_area, &list_state);

            // Render peek in lower half
            render_peek(frame.buffer_mut(), peek_area, &detail, &theme, 0);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        println!(
            "\n=== combined list+inline peek dump ({}×{}) ===",
            width, height
        );
        for y in 0..height {
            println!("|{}|", row_string(&buf, width, y));
        }
        println!("=== end combined dump ===\n");
    }
}
