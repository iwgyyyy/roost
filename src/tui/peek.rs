//! Peek detail panel — renders a rounded-corner box overlaying the list.
//!
//! Design spec §7:
//!
//! ```text
//! ╭─ peek · payments-api ──────────────────────────────────╮
//! │  agent    ✻ Claude Code                                │
//! │  status   ▲ Needs input · Approval                     │
//! │  source   @local  · via Cursor                         │
//! │  path     ~/dev/acme/payments-api                      │
//! │  step     run: git push --force origin main            │
//! │  since    waiting for your approval · 8s               │
//! │                                                        │
//! │  recent                                                │
//! │      8s  ▲ asked to run git push --force               │
//! │     40s  ✎ edited 3 files in src/                      │
//! │      1m  ⚙ ran test suite · 142 passed                 │
//! ╰─ esc close ────────────────────────────────────────────╯
//! ```
//!
//! Offline branch (stale=true):
//!   status  → `⊘ offline · remote disconnected`
//!   since   → `last seen · <duration>`
//!   timeline contains "lost connection"
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
    theme::{self, Theme},
};

// ── render_peek ───────────────────────────────────────────────────────────────

/// Render the peek detail panel into `buf` using `area`.
///
/// `detail` is the session detail to display.
/// If `area` is too small for the minimum layout, renders nothing (caller
/// should switch to full-screen mode).
/// Number of recent-event rows that fit in a peek panel of the given height.
/// `has_step` accounts for the optional `step` field taking one extra row.
/// Mirrors the fixed "chrome" rows rendered by [`render_peek`].
///
/// Fixed rows: header(1) + agent + status + source + path + since(5) +
///             step(0|1) + blank(1) + recent-label(1) + footer(1) = 9 + step.
pub fn recent_rows_capacity(area_height: u16, has_step: bool) -> usize {
    // header(1) + agent/status/source/path/since(5) + step(0|1) + blank(1) + recent-label(1) + footer(1)
    let chrome = 1 + 5 + has_step as u16 + 1 + 1 + 1;
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
        // "peek" keyword in the session's family colour (matches the list accent).
        buf.set_string(
            x0 + UnicodeWidthStr::width("╭─ ") as u16,
            y0,
            "peek",
            Style::default().fg(theme::family_color(&detail.family)),
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
    // Offline: family colour stays (icon identity doesn't change when stale).
    if row < footer_y {
        let (_, icon_color) = theme::family_icon(&detail.family);
        let agent_style = if detail.stale {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(icon_color)
        };
        render_field_row(buf, row, "agent   ", &detail.agent_display, agent_style);
        row += 1;
    }

    // ── Field: status ──
    // Offline branch: "⊘ offline · remote disconnected" in COLOR_OFFLINE (warm grey).
    // Online branch: glyph + full label in state colour.
    if row < footer_y {
        let (status_value, status_style) = if detail.stale {
            (
                "⊘ offline · remote disconnected".to_string(),
                Style::default().fg(theme::COLOR_OFFLINE),
            )
        } else {
            let (glyph, color) = theme::state_glyph(&detail.state);
            let label = state_full_label(&detail.state);
            (
                format!("{glyph} {label}"),
                Style::default().fg(color),
            )
        };
        render_field_row(buf, row, "status  ", &status_value, status_style);
        row += 1;
    }

    // ── Field: source ──
    // Format: `@local` (dim) or `@<device>` (cold blue) + `  · via <App>` (dim).
    // The device segment and via-app segment are written separately so that
    // the device name gets the remote cold-blue colour while the rest stays dim.
    if row < footer_y {
        let is_local = detail.origin == "local" || detail.origin.is_empty();
        let device_str = format!("@{}", detail.origin);
        let via_str = detail
            .host_app
            .as_deref()
            .map(capitalise)
            .map(|app| format!("  · via {app}"))
            .unwrap_or_default();
        let source_value = format!("{device_str}{via_str}");
        let device_color = if is_local || detail.stale {
            Color::DarkGray
        } else {
            theme::COLOR_REMOTE
        };
        render_field_row(
            buf,
            row,
            "source  ",
            &source_value,
            Style::default().fg(device_color),
        );
        row += 1;
    }

    // ── Field: path ──
    // Label in DarkGray; value in terminal default fg (dim when stale).
    if row < footer_y {
        let path_style = if detail.stale {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::Reset)
        };
        render_field_row(buf, row, "path    ", &detail.cwd, path_style);
        row += 1;
    }

    // ── Field: step ──
    // Shows the current action text. Label changed from "action" to "step" per §7.
    // Offline: show last-known step (the action field carries it from the daemon).
    if row < footer_y
        && let Some(step) = &detail.action
    {
        let step_style = if detail.stale {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::Reset)
        };
        render_field_row(buf, row, "step    ", step, step_style);
        row += 1;
    }

    // ── Field: since ──
    // Offline: "last seen · <duration>".
    // Online with waiting reason: "<reason> · <duration>".
    // Otherwise: plain duration.
    if row < footer_y {
        let time_str = anim::relative_time(detail.since_secs);
        let since_value = if detail.stale {
            format!("last seen · {time_str}")
        } else if let Some(reason) = &detail.waiting_reason {
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

// ── Capitalise first letter of a &str (ASCII-only, sufficient for app names) ──

fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
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
            origin: "local".to_string(),
            host_app: Some("cursor".to_string()),
            stale: false,
        }
    }

    fn make_offline_detail() -> SessionDetail {
        SessionDetail {
            id: "data-pipeline".to_string(),
            family: AgentFamily::Claude,
            name: "data-pipeline".to_string(),
            agent_display: "✻ Claude Code".to_string(),
            cwd: "~/ml/data-pipeline".to_string(),
            state: "working".to_string(),
            action: Some("Training run · epoch 4/10".to_string()),
            waiting_reason: None,
            since_secs: 120,
            recent_events: vec![
                Event {
                    ts_secs: 120,
                    summary: "lost connection".to_string(),
                    duration_secs: None,
                },
                Event {
                    ts_secs: 240,
                    summary: "⚙ started training epoch 4".to_string(),
                    duration_secs: Some(120),
                },
            ],
            origin: "gpu-rig".to_string(),
            host_app: Some("cursor".to_string()),
            stale: true,
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
        // chrome = header(1) + agent/status/source/path/since(5) + blank(1) +
        //          recent-label(1) + footer(1) = 9 (no step)
        assert_eq!(recent_rows_capacity(20, false), 11);
        // +1 for the step row
        assert_eq!(recent_rows_capacity(20, true), 10);
        // Too small for any recent rows
        assert_eq!(recent_rows_capacity(6, false), 0);
    }

    #[test]
    fn peek_scroll_reveals_older_events() {
        // 8 events, a small panel: scroll=0 shows the newest, scrolling reveals
        // older ones and drops the newest from view.
        // chrome=9 (no step), so capacity(12) = 12-9 = 3 recent rows.
        let detail = make_detail_with_events(8);
        let (w, h) = (60u16, 12u16); // capacity = 12 - 9 = 3 recent rows

        let top = render_peek_scrolled(w, h, &detail, 0);
        let top_text = buf_text(&top, w, h);
        assert!(top_text.contains("EV00"), "scroll=0 should show newest EV00");
        assert!(
            !top_text.contains("EV05"),
            "scroll=0 should not yet show old EV05"
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
            cwd: String::new(),
            state: "approval".to_string(),
            activity: Some("run: git push --force".to_string()),
            last_activity_secs: 8,
            busy_secs: 0,
            last_prompt: None,
            last_prompt_age_secs: None,
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
                accent: crate::tui::theme::COLOR_ACCENT,
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

    // ── §7 field-order & new fields ────────────────────────────────────────────

    /// Fields must appear in §7 order: agent, status, source, path, step, since.
    /// We scan the buffer row-by-row and record the y-coordinate of the first
    /// row that contains each label, then assert the ordering.
    #[test]
    fn peek_field_order_agent_status_source_path_step_since() {
        let detail = make_detail();
        let buf = render_peek_to_buf(70, 18, &detail);

        let mut agent_y: Option<u16> = None;
        let mut status_y: Option<u16> = None;
        let mut source_y: Option<u16> = None;
        let mut path_y: Option<u16> = None;
        let mut step_y: Option<u16> = None;
        let mut since_y: Option<u16> = None;

        for y in 0..18u16 {
            let row = row_string(&buf, 70, y);
            if row.contains("agent") && agent_y.is_none() {
                agent_y = Some(y);
            }
            if row.contains("status") && status_y.is_none() {
                status_y = Some(y);
            }
            if row.contains("source") && source_y.is_none() {
                source_y = Some(y);
            }
            if row.contains("path") && path_y.is_none() {
                path_y = Some(y);
            }
            if row.contains("step") && step_y.is_none() {
                step_y = Some(y);
            }
            if row.contains("since") && since_y.is_none() {
                since_y = Some(y);
            }
        }

        assert!(agent_y.is_some(), "agent field missing");
        assert!(status_y.is_some(), "status field missing");
        assert!(source_y.is_some(), "source field missing");
        assert!(path_y.is_some(), "path field missing");
        assert!(step_y.is_some(), "step field missing");
        assert!(since_y.is_some(), "since field missing");

        let a = agent_y.unwrap();
        let st = status_y.unwrap();
        let so = source_y.unwrap();
        let p = path_y.unwrap();
        let sp = step_y.unwrap();
        let si = since_y.unwrap();

        assert!(a < st, "agent({a}) must appear before status({st})");
        assert!(st < so, "status({st}) must appear before source({so})");
        assert!(so < p, "source({so}) must appear before path({p})");
        assert!(p < sp, "path({p}) must appear before step({sp})");
        assert!(sp < si, "step({sp}) must appear before since({si})");
    }

    /// source row for a local session: `@local  · via <app>`.
    #[test]
    fn peek_source_local_renders() {
        let detail = make_detail(); // origin="local", host_app="cursor"
        let buf = render_peek_to_buf(70, 18, &detail);
        let content: String = (0..18u16)
            .flat_map(|y| (0..70u16).map(move |x| (x, y)))
            .map(|(x, y)| cell_symbol(&buf, x, y))
            .collect();
        assert!(
            content.contains("@local"),
            "source row should contain '@local'"
        );
        assert!(
            content.contains("via Cursor") || content.contains("via cursor"),
            "source row should contain 'via Cursor'"
        );
    }

    /// source row for a remote session: `@<device> · via <app>` (device in cold blue).
    #[test]
    fn peek_source_remote_renders() {
        let mut detail = make_detail();
        detail.origin = "gpu-rig".to_string();
        detail.host_app = Some("cursor".to_string());
        let buf = render_peek_to_buf(70, 18, &detail);
        let content: String = (0..18u16)
            .flat_map(|y| (0..70u16).map(move |x| (x, y)))
            .map(|(x, y)| cell_symbol(&buf, x, y))
            .collect();
        assert!(
            content.contains("@gpu-rig"),
            "source row should contain '@gpu-rig' for remote"
        );
    }

    /// Offline status text: `⊘ offline · remote disconnected`.
    #[test]
    fn peek_offline_status_text() {
        let detail = make_offline_detail(); // stale=true
        let buf = render_peek_to_buf(70, 18, &detail);
        let content: String = (0..18u16)
            .flat_map(|y| (0..70u16).map(move |x| (x, y)))
            .map(|(x, y)| cell_symbol(&buf, x, y))
            .collect();
        assert!(
            content.contains("offline"),
            "offline session should show 'offline' in status, got snippet of content"
        );
        assert!(
            content.contains("remote disconnected"),
            "offline session should show 'remote disconnected'"
        );
        assert!(
            content.contains("⊘"),
            "offline session should show '⊘' glyph"
        );
    }

    /// Offline since shows `last seen` prefix.
    #[test]
    fn peek_offline_since_shows_last_seen() {
        let detail = make_offline_detail();
        let buf = render_peek_to_buf(70, 18, &detail);
        let content: String = (0..18u16)
            .flat_map(|y| (0..70u16).map(move |x| (x, y)))
            .map(|(x, y)| cell_symbol(&buf, x, y))
            .collect();
        assert!(
            content.contains("last seen"),
            "offline since field should say 'last seen'"
        );
    }

    /// Offline timeline must contain a "lost connection" event.
    #[test]
    fn peek_offline_timeline_has_lost_connection() {
        let detail = make_offline_detail();
        let buf = render_peek_to_buf(70, 18, &detail);
        let content: String = (0..18u16)
            .flat_map(|y| (0..70u16).map(move |x| (x, y)))
            .map(|(x, y)| cell_symbol(&buf, x, y))
            .collect();
        assert!(
            content.contains("lost connection"),
            "offline timeline should contain 'lost connection' event"
        );
    }

    /// Recent events are shown in reverse-chronological order (newest first).
    /// The event with ts_secs=8 (newest) must appear above ts_secs=60 (oldest).
    #[test]
    fn peek_recent_events_reverse_chron() {
        let detail = make_detail();
        let buf = render_peek_to_buf(70, 18, &detail);

        // Find y-coordinate of each event's summary text.
        let mut y_push: Option<u16> = None; // "git push" — newest (ts=8)
        let mut y_edited: Option<u16> = None; // "edited 3 files" — middle (ts=40)
        let mut y_suite: Option<u16> = None; // "ran test suite" — oldest (ts=60)

        for y in 0..18u16 {
            let row = row_string(&buf, 70, y);
            if row.contains("git push") && y_push.is_none() {
                y_push = Some(y);
            }
            if row.contains("edited 3 files") && y_edited.is_none() {
                y_edited = Some(y);
            }
            if row.contains("ran test suite") && y_suite.is_none() {
                y_suite = Some(y);
            }
        }

        assert!(y_push.is_some(), "git push event not found");
        assert!(y_edited.is_some(), "edited 3 files event not found");
        assert!(y_suite.is_some(), "ran test suite event not found");

        let yp = y_push.unwrap();
        let ye = y_edited.unwrap();
        let ys = y_suite.unwrap();

        assert!(
            yp < ye,
            "newest 'git push'({yp}) should appear above 'edited'({ye})"
        );
        assert!(
            ye < ys,
            "'edited'({ye}) should appear above oldest 'ran test suite'({ys})"
        );
    }

    /// `step` label must appear (spec §7 renames `action` → `step` in the panel).
    #[test]
    fn peek_step_field_label() {
        let detail = make_detail();
        let buf = render_peek_to_buf(70, 18, &detail);
        let content: String = (0..18u16)
            .flat_map(|y| (0..70u16).map(move |x| (x, y)))
            .map(|(x, y)| cell_symbol(&buf, x, y))
            .collect();
        assert!(
            content.contains("step"),
            "peek panel should show 'step' field label per §7"
        );
        // The old label "action" must not be present
        assert!(
            !content.contains("action"),
            "peek panel should NOT show old 'action' label, use 'step'"
        );
    }
}
