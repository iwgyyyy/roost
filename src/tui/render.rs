//! Rendering functions for the TUI.
//!
//! All rendering targets a ratatui `Buffer` via `Widget` impls or direct cell writes.
//! Uses `TestBackend` in tests for headless validation.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use crate::jump::{descriptor_for, normalize_host_name};
use crate::protocol::SessionView;
use crate::tui::{
    anim::Anim,
    layout::{self, ColumnPlan, HeightProfile},
    theme::{self, Theme},
    viewmodel::{Group, Selection},
};

// ── Row renderer ─────────────────────────────────────────────────────────────

/// Render one session row into `buf` at `area` (single-line Rect).
///
/// `flat_idx` is the row's index in the flat list (used to determine selection).
#[allow(clippy::too_many_arguments)]
pub fn render_row(
    buf: &mut Buffer,
    area: Rect,
    session: &SessionView,
    plan: &ColumnPlan,
    flat_idx: usize,
    selection: &Selection,
    theme: &Theme,
    anim: &Anim,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let selected = selection.is_selected(flat_idx);
    let y = area.y;
    let mut x = area.x;
    let right = area.x + area.width;

    // ── Selection bar ─────────────────────────────────────────────────────
    if let Some(w) = plan.selbar
        && x < right
    {
        let glyph = if selected { "▌" } else { " " };
        // Selection bar takes the selected row's family colour, matching the
        // dynamic `roost` title accent.
        let style = Style::default().fg(theme::family_color(&session.family));
        buf.set_string(x, y, glyph, style);
        x += w;
    }

    // ── Family icon ───────────────────────────────────────────────────────
    if let Some(w) = plan.icon
        && x < right
    {
        let (icon, color) = theme::family_icon(&session.family);
        buf.set_string(x, y, icon, Style::default().fg(color));
        x += w;
    }

    // ── Family label ──────────────────────────────────────────────────────
    if let Some(fw) = plan.family
        && x < right
    {
        // fw includes 1 trailing space gap; the text portion is fw - 1 columns.
        let text_w = (fw as usize).saturating_sub(1);
        let label = session.family.short_label();
        let label_str = layout::truncate_to_width(label, text_w);
        let label_str = layout::pad_to_width(&label_str, text_w);
        let (_, color) = theme::family_icon(&session.family);
        buf.set_string(x, y, &label_str, Style::default().fg(color));
        x += fw; // fw already includes the 1-space gap
    }

    // ── Name ──────────────────────────────────────────────────────────────
    {
        let name_w = plan.name as usize;
        let name_str = layout::truncate_to_width(&session.name, name_w);
        let name_str = layout::pad_to_width(&name_str, name_w);
        // Name uses the terminal's default foreground (no explicit colour) so it stays
        // clearly readable on both light and dark terminals — the dark-theme `fg` grey
        // washes out on light backgrounds.
        let mut style = Style::default();
        if selected {
            style = style.add_modifier(Modifier::BOLD);
        }
        if x < right {
            buf.set_string(x, y, &name_str, style);
            x += plan.name;
        }
    }

    // 1-space gap between name and glyph
    if x < right {
        buf.set_string(x, y, " ", Style::default());
        x += 1;
    }

    // ── State glyph ───────────────────────────────────────────────────────
    {
        let (glyph, color) = glyph_for_session(session, anim);
        if x < right {
            buf.set_string(x, y, glyph, Style::default().fg(color));
            x += plan.glyph;
        }
    }

    // ── Status label ─────────────────────────────────────────────────────
    if let Some(lw) = plan.label
        && x < right
    {
        x += 1; // gap
        let label_str = layout::truncate_to_width(&session.state, lw as usize);
        let label_str = layout::pad_to_width(&label_str, lw as usize);
        let color = theme::state_color(&session.state);
        buf.set_string(x, y, &label_str, Style::default().fg(color));
        x += lw;
    }

    // ── Time (right-aligned, reserve before mid) ──────────────────────────
    // We'll draw mid first then time, but we need to know the time column position.
    // So: mid spans from current x to (right - time_w - 1 gap), time fills the end.

    let time_reserve = plan.time.map(|tw| tw as usize + 1).unwrap_or(0); // +1 gap
    let mid_end = right as usize - time_reserve;
    let mid_start = x as usize;

    // ── Mid column ───────────────────────────────────────────────────────
    if plan.mid.is_some() && mid_start < mid_end {
        x += 2; // gap before mid (breathing room before activity text)
        let mid_content = mid_text(session);
        let avail = (mid_end as i32 - x as i32).max(0) as usize;
        let mid_str = layout::truncate_to_width(&mid_content, avail);
        let mid_str = layout::pad_to_width(&mid_str, avail);
        // Working: use default terminal fg (active, important text).
        // Idle/Done/others: dim grey (secondary, low priority text).
        let mid_color = if session.state == "working" {
            Color::Reset
        } else {
            Color::DarkGray
        };
        let style = Style::default().fg(mid_color);
        if x < right {
            buf.set_string(x, y, &mid_str, style);
            x += avail as u16;
        }
    }

    // ── Time ─────────────────────────────────────────────────────────────
    if let Some(tw) = plan.time
        && x < right
    {
        let time_str = crate::tui::anim::relative_time(session.busy_secs);
        let time_str = layout::truncate_to_width(&time_str, tw as usize);
        // Colour: Approval rows show time in amber, Question in cyan
        let color = match session.state.as_str() {
            "approval" => theme::COLOR_APPROVAL,
            "question" => theme::COLOR_QUESTION,
            _ => theme.dim,
        };
        // Right-align within available space
        let time_x = right.saturating_sub(tw);
        buf.set_string(time_x, y, &time_str, Style::default().fg(color));
    }

    let _ = x; // suppress unused warning
}

/// Get the live glyph for a session, including animated spinner for working state.
fn glyph_for_session<'a>(
    session: &SessionView,
    anim: &'a Anim,
) -> (&'a str, ratatui::style::Color) {
    if session.state == "working" {
        return (anim.spinner(), theme::COLOR_WORKING);
    }
    if session.state == "approval" {
        // Breathing animation: interpolate colour
        let t = anim.breathe();
        let color = theme::approval_breathe_color(t);
        return ("▲", color);
    }
    let (g, c) = theme::state_glyph(&session.state);
    (g, c)
}

/// Mid column text content per spec §7.2.
fn mid_text(session: &SessionView) -> String {
    let content = match session.state.as_str() {
        "working" => session
            .activity
            .clone()
            .unwrap_or_else(|| "thinking…".to_string()),
        "approval" | "question" => session
            .activity
            .clone()
            .unwrap_or_else(|| session.name.clone()),
        _ => {
            // Idle / Done: show last_prompt if available, otherwise nothing.
            // The name is already shown in the name column — don't repeat it here.
            session.last_prompt.clone().unwrap_or_default()
        }
    };
    // Append host label if known (low priority — truncation at render time handles narrow screens)
    if let Some(host_raw) = session.host_app.as_deref() {
        let canonical = normalize_host_name(host_raw);
        if let Some(desc) = descriptor_for(&canonical) {
            // Only show label if host is not "unknown"
            if canonical != "unknown" {
                // 3-space gap before the host label so it reads clearly
                // separated from the activity text (e.g. "thinking…   Zed").
                return format!("{content}   {}", desc.tui_label.trim_start());
            }
        }
    }
    content
}

// ── Group header ──────────────────────────────────────────────────────────────

/// Render a group header line (e.g. "  NEEDS INPUT  2").
pub fn render_group_header(buf: &mut Buffer, area: Rect, group: &Group, _theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let y = area.y;
    let x = area.x;
    // Title label in terminal default fg (adaptive, readable on light & dark).
    let title = format!("  {}  ", group.title);
    buf.set_string(x, y, &title, Style::default().fg(Color::Reset));
    // Count in semantic state colour (RGB — always identifiable).
    let count_str = format!("{}", group.count());
    let count_x = x + title.len() as u16;
    if count_x < area.x + area.width {
        buf.set_string(
            count_x,
            y,
            &count_str,
            Style::default().fg(group.state_color),
        );
    }
}

// ── Header ───────────────────────────────────────────────────────────────────

/// Render the TUI header line: `╭─ roost ── … N agents · live ● ─╮`
pub fn render_header(
    buf: &mut Buffer,
    area: Rect,
    agent_count: usize,
    daemon_ready: bool,
    anim: &Anim,
    theme: &Theme,
    accent: Color,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let y = area.y;
    let x = area.x;
    let w = area.width as usize;

    // Brand + agent count text
    let heartbeat = if daemon_ready && anim.heartbeat() {
        " ●"
    } else {
        "  "
    };
    let status = if daemon_ready {
        if agent_count == 1 {
            format!("1 agent · live{heartbeat}")
        } else {
            format!("{agent_count} agents · live{heartbeat}")
        }
    } else {
        "waiting for daemon…".to_string()
    };

    // Build a single line: "╭─ roost ── … <status> ─╮"
    // Left side
    let left = "╭─ roost ";
    let right = " ─╮";
    let fill_avail = w
        .saturating_sub(UnicodeWidthStr::width(left))
        .saturating_sub(UnicodeWidthStr::width(right))
        .saturating_sub(UnicodeWidthStr::width(status.as_str()));
    let fill = "─".repeat(fill_avail);

    let line = format!("{left}{fill}{status}{right}");
    buf.set_string(x, y, &line, Style::default().fg(theme.border));
    // Colour "roost" in accent
    let roost_x = x + 3; // after "╭─ "
    buf.set_string(roost_x, y, "roost", Style::default().fg(accent));
    // Colour the heartbeat dot
    if daemon_ready && agent_count > 0 {
        // dot_pos is measured in display columns (not bytes)
        let dot_pos =
            UnicodeWidthStr::width(left) + fill_avail + UnicodeWidthStr::width(status.as_str())
                - UnicodeWidthStr::width(heartbeat.trim());
        if anim.heartbeat() {
            buf.set_string(
                x + dot_pos as u16,
                y,
                "●",
                Style::default().fg(theme::COLOR_WORKING),
            );
        }
    }
}

/// Render the bottom border line.
///
/// `peek_open`: when true, shows `esc close` hint; otherwise shows `enter open`.
pub fn render_footer_border(buf: &mut Buffer, area: Rect, theme: &Theme, peek_open: bool) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let y = area.y;
    let x = area.x;
    let w = area.width as usize;

    let keys: String = if peek_open {
        "↑/↓ scroll · tab switch · esc close · q quit".to_string()
    } else {
        "↑/↓ j/k select · enter peek · o jump · q quit".to_string()
    };

    // "╰── <keys> ──╯"
    let left = "╰── ";
    let right = " ──╯";
    let fill_avail = w
        .saturating_sub(UnicodeWidthStr::width(left))
        .saturating_sub(UnicodeWidthStr::width(keys.as_str()))
        .saturating_sub(UnicodeWidthStr::width(right));
    let fill = "─".repeat(fill_avail);
    let line = format!("{left}{keys}{fill}{right}");
    buf.set_string(x, y, &line, Style::default().fg(theme.border));
    // keys in dim — start column in display columns (not bytes)
    buf.set_string(
        x + UnicodeWidthStr::width(left) as u16,
        y,
        keys.as_str(),
        Style::default().fg(theme.dim),
    );
}

// ── Status line ────────────────────────────────────────────────────────────────

/// Render a transient status message (e.g. jump result) on a single line.
///
/// Rendered as an overlay on the row directly above the footer border so it
/// doesn't consume a body row or shift the scroll position. The line is cleared
/// first so it never mixes with whatever row was underneath it.
pub fn render_status_line(buf: &mut Buffer, area: Rect, theme: &Theme, msg: &str) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let y = area.y;
    let w = area.width as usize;

    // Clear the line first (overlay — there may be an agent row underneath).
    let blank = " ".repeat(w);
    buf.set_string(area.x, y, &blank, Style::default());

    // Align the text with the footer hint indent ("╰── " == 4 cols) for a tidy look.
    let indent = 4u16;
    let text_x = area.x + indent;
    let avail = w.saturating_sub(indent as usize + 1);
    let text = layout::truncate_to_width(msg, avail);
    // Default terminal fg so it stays readable on both light and dark terminals.
    buf.set_string(text_x, y, &text, Style::default().fg(Color::Reset));
    let _ = theme;
}

// ── Empty state ───────────────────────────────────────────────────────────────

/// Render the empty state (no agents running) in the list area.
pub fn render_empty_state(buf: &mut Buffer, area: Rect, _theme: &Theme) {
    if area.height < 2 || area.width < 10 {
        return;
    }
    // Centre vertically
    let mid_y = area.y + area.height / 2 - 1;

    let line1 = "✻  no agents are running";
    let line2 = "roost watches Claude Code & Codex sessions";
    let line3 = "   (they report in automatically via hooks)";

    for (i, line) in [line1, line2, line3].iter().enumerate() {
        let display_w = unicode_width::UnicodeWidthStr::width(*line);
        let col = if (display_w as u16) < area.width {
            area.x + (area.width - display_w as u16) / 2
        } else {
            area.x
        };
        let y = mid_y + i as u16;
        if y < area.y + area.height {
            let style = if i == 0 {
                // Primary headline: default terminal fg (adaptive).
                Style::default().fg(Color::Reset)
            } else {
                // Secondary / hint text: dim grey.
                Style::default().fg(Color::DarkGray)
            };
            buf.set_string(col, y, line, style);
        }
    }
}

// ── Side border ───────────────────────────────────────────────────────────────

/// Draw `│` borders for the left and right sides of the content area.
pub fn render_side_borders(buf: &mut Buffer, area: Rect, theme: &Theme) {
    let style = Style::default().fg(theme.border);
    for y in area.y..area.y + area.height {
        buf.set_string(area.x, y, "│", style);
        let rx = area.x + area.width - 1;
        if rx > area.x {
            buf.set_string(rx, y, "│", style);
        }
    }
}

// ── "Too small" notice ────────────────────────────────────────────────────────

pub fn render_too_small(buf: &mut Buffer, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let msg = "↔ ≥40 cols";
    buf.set_string(area.x, area.y, msg, Style::default().fg(theme::COLOR_DIM));
}

// ── Full-screen layout ────────────────────────────────────────────────────────

/// Represents the full TUI state for one render pass.
pub struct RenderState<'a> {
    pub groups: &'a [Group],
    pub selection: &'a Selection,
    pub daemon_ready: bool,
    pub anim: &'a Anim,
    pub theme: &'a Theme,
    pub scroll_offset: usize,
    /// When true, footer shows "esc close"; when false, shows key hints.
    pub peek_open: bool,
    /// Optional jump status message to display in the footer.
    pub jump_status: Option<&'a str>,
}

/// Family of the currently selected session, if any — drives the dynamic accent.
fn selected_family<'a>(state: &'a RenderState<'_>) -> Option<&'a crate::protocol::AgentFamily> {
    let mut flat = 0usize;
    for g in state.groups {
        for row in &g.rows {
            if state.selection.is_selected(flat) {
                return Some(&row.family);
            }
            flat += 1;
        }
    }
    None
}

/// Render the entire TUI into `buf` using `area` as the full screen.
pub fn render_frame(buf: &mut Buffer, area: Rect, state: &RenderState<'_>) {
    if area.height < 2 || area.width < 10 {
        render_too_small(buf, area);
        return;
    }

    let plan = layout::columns_for(area.width);
    let height_prof = layout::height_profile(area.height);

    // Header row (y = 0)
    let agent_count: usize = state.groups.iter().map(|g| g.count()).sum();
    let header_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    // Dynamic accent: the `roost` title follows the selected session's family
    // colour. Falls back to the static accent when nothing is selected.
    let accent = selected_family(state)
        .map(|f| theme::family_color(f))
        .unwrap_or(theme::COLOR_ACCENT);
    render_header(
        buf,
        header_area,
        agent_count,
        state.daemon_ready,
        state.anim,
        state.theme,
        accent,
    );

    // Footer row (last row)
    let footer_y = area.y + area.height - 1;
    let footer_area = Rect {
        x: area.x,
        y: footer_y,
        width: area.width,
        height: 1,
    };
    render_footer_border(buf, footer_area, state.theme, state.peek_open);

    // Body area (between header and footer)
    let body_y = area.y + 1;
    let body_h = area.height.saturating_sub(2);
    if body_h == 0 {
        return;
    }
    let body_area = Rect {
        x: area.x,
        y: body_y,
        width: area.width,
        height: body_h,
    };

    if state.groups.is_empty() {
        render_empty_state(buf, body_area, state.theme);
    }

    // Render groups
    let mut flat_idx: usize = 0;
    let mut current_y = body_y;

    for group in state.groups {
        // Skip entire group if it's non-critical in critical mode
        let is_critical = matches!(height_prof, HeightProfile::Critical);
        let is_needs_input = group.title == "NEEDS INPUT";

        if is_critical && !is_needs_input {
            // Show "… N more" for collapsed groups
            if current_y < footer_y {
                let msg = format!("… {} more", group.count());
                buf.set_string(
                    area.x + 2,
                    current_y,
                    &msg,
                    Style::default().fg(state.theme.dim),
                );
                current_y += 1;
            }
            flat_idx += group.count();
            continue;
        }

        // Group header — only render if at least one row of this group is visible.
        // A group's rows span flat indices [flat_idx, flat_idx + group.count()).
        // Visible rows are those with flat_idx >= scroll_offset and current_y < footer_y.
        let group_has_visible_row = flat_idx + group.count() > state.scroll_offset;
        // Blank spacer line above each group header for vertical breathing room
        // (gap under the header, and between groups). Skipped in Critical height
        // mode where every row is precious.
        if !is_critical && group_has_visible_row && current_y < footer_y {
            current_y += 1;
        }
        if group_has_visible_row && current_y < footer_y {
            let gh_area = Rect {
                x: area.x,
                y: current_y,
                width: area.width,
                height: 1,
            };
            render_group_header(buf, gh_area, group, state.theme);
            current_y += 1;
        }

        // Rows (with scroll)
        for row in &group.rows {
            // Apply scroll offset
            if flat_idx < state.scroll_offset {
                flat_idx += 1;
                continue;
            }
            if current_y >= footer_y {
                flat_idx += 1;
                continue;
            }
            let row_area = Rect {
                x: area.x,
                y: current_y,
                width: area.width,
                height: 1,
            };
            render_row(
                buf,
                row_area,
                row,
                &plan,
                flat_idx,
                state.selection,
                state.theme,
                state.anim,
            );
            current_y += 1;
            flat_idx += 1;
        }
    }

    // ── Status line (transient toast above the footer) ─────────────────────
    // Drawn last so it overlays the bottom body row rather than reserving a row
    // (keeps the agent list's scroll position stable). Only the row directly
    // above the footer border is used; the header row is never touched.
    if let Some(msg) = state.jump_status {
        let status_y = footer_y.saturating_sub(1);
        if status_y > area.y {
            let status_area = Rect {
                x: area.x,
                y: status_y,
                width: area.width,
                height: 1,
            };
            render_status_line(buf, status_area, state.theme, msg);
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AgentFamily;
    use crate::tui::viewmodel;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn make_session(id: &str, state: &str, activity: Option<&str>) -> SessionView {
        SessionView {
            id: id.to_string(),
            family: AgentFamily::Claude,
            name: "my-repo".to_string(),
            state: state.to_string(),
            activity: activity.map(str::to_string),
            last_activity_secs: 5,
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
        }
    }

    fn make_terminal(width: u16, height: u16) -> Terminal<TestBackend> {
        let backend = TestBackend::new(width, height);
        Terminal::new(backend).unwrap()
    }

    // ── Empty state ───────────────────────────────────────────────────────

    #[test]
    fn empty_state_renders_without_panic() {
        let mut term = make_terminal(80, 24);
        let anim = Anim::new();
        let theme = Theme::classic();
        let groups = vec![];
        let selection = Selection::new();

        term.draw(|frame| {
            let area = frame.area();
            let state = RenderState {
                groups: &groups,
                selection: &selection,
                daemon_ready: true,
                anim: &anim,
                theme: &theme,
                scroll_offset: 0,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        // Check that "no agents" text is present somewhere
        let content: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(
            content.contains("no agents"),
            "expected 'no agents' in empty state, got buffer content"
        );
    }

    #[test]
    fn empty_state_header_shows_zero() {
        let mut term = make_terminal(80, 24);
        let anim = Anim::new();
        let theme = Theme::classic();
        let groups = vec![];
        let selection = Selection::new();

        term.draw(|frame| {
            let area = frame.area();
            let state = RenderState {
                groups: &groups,
                selection: &selection,
                daemon_ready: true,
                anim: &anim,
                theme: &theme,
                scroll_offset: 0,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        // Header line (row 0) should contain "0 agents"
        let buf = term.backend().buffer().clone();
        let header_row: String = (0..80)
            .map(|x| {
                buf.cell((x, 0))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_default()
            })
            .collect();
        assert!(
            header_row.contains("0 agents"),
            "header should contain '0 agents', got: {header_row:?}"
        );
    }

    // ── Row rendering ─────────────────────────────────────────────────────

    #[test]
    fn working_row_has_spinner() {
        let session = make_session("s1", "working", Some("思考中…"));
        let plan = layout::columns_for(100);
        let selection = Selection { index: Some(0) };
        let theme = Theme::classic();
        let anim = Anim::new();

        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 1));
        render_row(
            &mut buf,
            Rect::new(0, 0, 100, 1),
            &session,
            &plan,
            0,
            &selection,
            &theme,
            &anim,
        );

        let row: String = (0..100)
            .map(|x| {
                buf.cell((x, 0))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_default()
            })
            .collect();
        // Selection bar should be ▌
        assert!(
            row.contains('▌'),
            "selected row should contain ▌, got: {row:?}"
        );
        // Should contain "my-repo"
        assert!(row.contains("my-repo"), "row should contain 'my-repo'");
    }

    #[test]
    fn unselected_row_has_no_selection_bar() {
        let session = make_session("s1", "working", Some("思考中…"));
        let plan = layout::columns_for(100);
        let selection = Selection { index: Some(1) }; // different index
        let theme = Theme::classic();
        let anim = Anim::new();

        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 1));
        render_row(
            &mut buf,
            Rect::new(0, 0, 100, 1),
            &session,
            &plan,
            0,
            &selection,
            &theme,
            &anim,
        );

        // x=0 should be a space (not ▌)
        let first_cell = buf
            .cell((0, 0))
            .map(|c| c.symbol().to_string())
            .unwrap_or_default();
        assert_ne!(first_cell, "▌", "unselected row should not have ▌");
    }

    #[test]
    fn approval_row_renders_triangle_glyph() {
        let session = make_session("s1", "approval", Some("run: git push"));
        let plan = layout::columns_for(100);
        let selection = Selection::new();
        let theme = Theme::classic();
        let anim = Anim::new();

        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 1));
        render_row(
            &mut buf,
            Rect::new(0, 0, 100, 1),
            &session,
            &plan,
            0,
            &selection,
            &theme,
            &anim,
        );

        let row: String = (0..100)
            .map(|x| {
                buf.cell((x, 0))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_default()
            })
            .collect();
        assert!(
            row.contains('▲'),
            "approval row should contain ▲, got: {row:?}"
        );
    }

    #[test]
    fn mid_column_shows_activity_for_working() {
        let session = make_session("s1", "working", Some("Editing src/main.rs"));
        let plan = layout::columns_for(100);
        let selection = Selection::new();
        let theme = Theme::classic();
        let anim = Anim::new();

        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 1));
        render_row(
            &mut buf,
            Rect::new(0, 0, 100, 1),
            &session,
            &plan,
            0,
            &selection,
            &theme,
            &anim,
        );

        let row: String = (0..100)
            .map(|x| {
                buf.cell((x, 0))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_default()
            })
            .collect();
        assert!(
            row.contains("Editing src/main.rs"),
            "mid column should show activity text, got: {row:?}"
        );
    }

    // ── Full frame width tiers ─────────────────────────────────────────────

    #[test]
    fn frame_renders_at_120_cols() {
        let sessions = vec![make_session("s1", "working", Some("Editing main.rs"))];
        let groups = viewmodel::group_and_sort(&sessions);
        let mut term = make_terminal(120, 30);
        let anim = Anim::new();
        let theme = Theme::classic();
        let selection = Selection { index: Some(0) };

        term.draw(|frame| {
            let area = frame.area();
            let state = RenderState {
                groups: &groups,
                selection: &selection,
                daemon_ready: true,
                anim: &anim,
                theme: &theme,
                scroll_offset: 0,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        let content: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(content.contains("my-repo"), "name column should appear");
        assert!(content.contains("roost"), "header should contain 'roost'");
    }

    #[test]
    fn frame_renders_at_80_cols() {
        let sessions = vec![make_session("s1", "approval", Some("run: git push"))];
        let groups = viewmodel::group_and_sort(&sessions);
        let mut term = make_terminal(80, 20);
        let anim = Anim::new();
        let theme = Theme::classic();
        let selection = Selection::new();

        term.draw(|frame| {
            let area = frame.area();
            let state = RenderState {
                groups: &groups,
                selection: &selection,
                daemon_ready: true,
                anim: &anim,
                theme: &theme,
                scroll_offset: 0,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        let content: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(content.contains("my-repo"));
    }

    #[test]
    fn frame_renders_at_40_cols() {
        let sessions = vec![make_session("s1", "idle", None)];
        let groups = viewmodel::group_and_sort(&sessions);
        let mut term = make_terminal(40, 10);
        let anim = Anim::new();
        let theme = Theme::classic();
        let selection = Selection::new();

        term.draw(|frame| {
            let area = frame.area();
            let state = RenderState {
                groups: &groups,
                selection: &selection,
                daemon_ready: false,
                anim: &anim,
                theme: &theme,
                scroll_offset: 0,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        // Should not panic and should contain some content
        let buf = term.backend().buffer().clone();
        let content: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(!content.is_empty());
    }

    #[test]
    fn cjk_name_does_not_overflow_column() {
        let mut session = make_session("s1", "working", Some("思考中…"));
        session.name = "思考项目中心名称".to_string(); // wide CJK name
        let plan = layout::columns_for(100);
        let selection = Selection::new();
        let theme = Theme::classic();
        let anim = Anim::new();

        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 1));
        // Should not panic
        render_row(
            &mut buf,
            Rect::new(0, 0, 100, 1),
            &session,
            &plan,
            0,
            &selection,
            &theme,
            &anim,
        );
    }

    // ── Scrolling ────────────────────────────────────────────────────────

    #[test]
    fn scrolled_frame_shows_correct_rows() {
        let sessions: Vec<SessionView> = (0..10)
            .map(|i| {
                let mut s = make_session(&format!("s{i}"), "working", None);
                s.name = format!("repo-{i:02}");
                s.last_activity_secs = i;
                s
            })
            .collect();
        let groups = viewmodel::group_and_sort(&sessions);
        let mut term = make_terminal(80, 10); // only 8 body rows
        let anim = Anim::new();
        let theme = Theme::classic();
        let selection = Selection { index: Some(7) };
        // scroll so selected row is visible
        let (scroll_offset, _) = layout::visible_window(10, 7, 6);

        term.draw(|frame| {
            let area = frame.area();
            let state = RenderState {
                groups: &groups,
                selection: &selection,
                daemon_ready: true,
                anim: &anim,
                theme: &theme,
                scroll_offset,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();
        // Just verify it doesn't panic and produces output
        let buf = term.backend().buffer().clone();
        let content: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();
        assert!(content.contains("roost"));
    }

    // ── Border alignment regression tests (B1 / B2 / B3) ────────────────────
    //
    // These tests ensure that the right border characters (╮ in the header and
    // ╯ in the footer) land exactly in the last visible column (width - 1), and
    // that the heartbeat dot ● appears at an expected column.  They were
    // deliberately written to FAIL before the unicode-width fixes and PASS after.

    /// Extract the symbol of a single cell from the buffer.
    fn cell_symbol(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> String {
        buf.cell((x, y))
            .map(|c| c.symbol().to_string())
            .unwrap_or_default()
    }

    /// Collect all symbols on row `y` as a single String.
    fn row_string(buf: &ratatui::buffer::Buffer, width: u16, y: u16) -> String {
        (0..width).map(|x| cell_symbol(buf, x, y)).collect()
    }

    /// Render a full frame with one working session at the given width and
    /// return the resulting buffer.
    fn render_at_width(width: u16, height: u16, daemon_ready: bool) -> ratatui::buffer::Buffer {
        let sessions = vec![make_session("s1", "working", Some("Editing main.rs"))];
        let groups = viewmodel::group_and_sort(&sessions);
        let mut term = make_terminal(width, height);
        let anim = Anim::new();
        let theme = Theme::classic();
        let selection = Selection { index: Some(0) };

        term.draw(|frame| {
            let area = frame.area();
            let state = RenderState {
                groups: &groups,
                selection: &selection,
                daemon_ready,
                anim: &anim,
                theme: &theme,
                scroll_offset: 0,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        term.backend().buffer().clone()
    }

    #[test]
    fn header_right_border_aligns_at_100_cols() {
        // B1 regression: ╮ must appear in column 99 (the last column of a 100-wide terminal).
        let buf = render_at_width(100, 20, true);
        let last_col = 99u16;
        let header_last = cell_symbol(&buf, last_col, 0);
        assert_eq!(
            header_last,
            "╮",
            "header right border should be ╮ at col {last_col}, got {header_last:?}\nrow: {:?}",
            row_string(&buf, 100, 0)
        );
    }

    #[test]
    fn footer_right_border_aligns_at_100_cols() {
        // B3 regression: ╯ must appear in column 99 (last column of footer row).
        let buf = render_at_width(100, 20, true);
        let last_col = 99u16;
        let footer_y = 19u16; // height - 1
        let footer_last = cell_symbol(&buf, last_col, footer_y);
        assert_eq!(
            footer_last,
            "╯",
            "footer right border should be ╯ at col {last_col}, got {footer_last:?}\nrow: {:?}",
            row_string(&buf, 100, footer_y)
        );
    }

    #[test]
    fn header_right_border_aligns_at_73_cols() {
        // B1 regression at a deliberately non-round width (73).
        let buf = render_at_width(73, 20, true);
        let last_col = 72u16;
        let header_last = cell_symbol(&buf, last_col, 0);
        assert_eq!(
            header_last,
            "╮",
            "header right border should be ╮ at col {last_col} (width=73), got {header_last:?}\nrow: {:?}",
            row_string(&buf, 73, 0)
        );
    }

    #[test]
    fn footer_right_border_aligns_at_73_cols() {
        // B3 regression at width 73.
        let buf = render_at_width(73, 20, true);
        let last_col = 72u16;
        let footer_y = 19u16;
        let footer_last = cell_symbol(&buf, last_col, footer_y);
        assert_eq!(
            footer_last,
            "╯",
            "footer right border should be ╯ at col {last_col} (width=73), got {footer_last:?}\nrow: {:?}",
            row_string(&buf, 73, footer_y)
        );
    }

    #[test]
    fn heartbeat_dot_appears_in_header_at_100_cols() {
        // B2 regression: with daemon_ready=true the ● dot should be visible
        // somewhere on the header row (not off-screen or at a wrong position).
        // We use an Anim whose heartbeat() is true at tick 0.
        let buf = render_at_width(100, 20, true);
        let header_row = row_string(&buf, 100, 0);
        assert!(
            header_row.contains('●'),
            "header should contain ● heartbeat dot, got: {header_row:?}"
        );
        // ● must not appear past the last column (would be invisible anyway, but
        // this guards against the dot_pos calculation overshooting the line).
        let dot_col = (0..100u16).find(|&x| cell_symbol(&buf, x, 0) == "●");
        let dot_col = dot_col.expect("● should be found in header row");
        assert!(
            dot_col < 99,
            "● should not be in or past the right border column, dot_col={dot_col}"
        );
        // ● should be reasonably near the right side (within last 30 cols of a 100-col terminal).
        assert!(
            dot_col >= 70,
            "● should be in the right portion of the header, dot_col={dot_col}"
        );
    }

    #[test]
    fn side_borders_aligned_at_100_cols() {
        // Verify │ side borders rendered by render_frame body are in col 0 and col 99.
        // (render_side_borders is not called directly from render_frame for the body, but
        // this checks no cell past col 99 is written.)
        let buf = render_at_width(100, 20, true);
        // The header and footer cover rows 0 and 19; check a body row.
        // Row 1 should be the group header "  WORKING  1".
        let row1 = row_string(&buf, 100, 1);
        assert!(!row1.is_empty(), "body row 1 should not be empty");
        // Total rendered string width must not exceed terminal width.
        assert_eq!(
            row1.len(),
            100,
            "buffer row length should equal terminal width in cells"
        );
    }

    #[test]
    fn scrolled_away_group_header_not_rendered() {
        // I5 regression: when scroll_offset pushes all rows of the first group
        // off-screen, the first group's header must not appear.
        //
        // We create two groups by having sessions with different states.
        // The "NEEDS INPUT" group (approval) is group 0; the "WORKING" group is group 1.
        // If we scroll past the approval row, the approval group header should vanish.
        let sessions = vec![
            {
                let mut s = make_session("s_approval", "approval", None);
                s.name = "approval-repo".to_string();
                s
            },
            {
                let mut s = make_session("s_working", "working", Some("doing stuff"));
                s.name = "working-repo".to_string();
                s
            },
        ];
        let groups = viewmodel::group_and_sort(&sessions);
        let mut term = make_terminal(80, 10);
        let anim = Anim::new();
        let theme = Theme::classic();
        let selection = Selection::new();

        // scroll_offset = 1 skips the first session (the approval one),
        // so the "NEEDS INPUT" group header should not be rendered.
        term.draw(|frame| {
            let area = frame.area();
            let state = RenderState {
                groups: &groups,
                selection: &selection,
                daemon_ready: true,
                anim: &anim,
                theme: &theme,
                scroll_offset: 1, // skip past the approval session
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        let content: String = buf
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect();

        // The working session should appear since it is still visible.
        assert!(
            content.contains("working-repo"),
            "working-repo should be visible after scrolling past approval"
        );
        // NEEDS INPUT group header should NOT appear (its only row is scrolled away).
        assert!(
            !content.contains("NEEDS INPUT"),
            "NEEDS INPUT group header should not appear when all its rows are scrolled away\ncontent: {content:?}"
        );
    }

    #[test]
    fn jump_status_renders_above_footer_not_in_border() {
        // The status message must appear on the row directly above the footer,
        // and the footer border row must still show the key hints (not the message).
        let sessions = vec![make_session("s1", "idle", None)];
        let groups = viewmodel::group_and_sort(&sessions);
        let width = 80u16;
        let height = 12u16;
        let mut term = make_terminal(width, height);
        let anim = Anim::new();
        let theme = Theme::classic();
        let selection = Selection { index: Some(0) };
        let msg = "LSCopyApplicationURLsForBundleIdentifier failed for dev.zed.Zed";

        term.draw(|frame| {
            let area = frame.area();
            let state = RenderState {
                groups: &groups,
                selection: &selection,
                daemon_ready: true,
                anim: &anim,
                theme: &theme,
                scroll_offset: 0,
                peek_open: false,
                jump_status: Some(msg),
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        let footer_y = height - 1;
        let footer_row = row_string(&buf, width, footer_y);
        let status_row = row_string(&buf, width, footer_y - 1);

        // Footer keeps the hints, NOT the jump message.
        assert!(
            footer_row.contains("select") && footer_row.contains("quit"),
            "footer should still show key hints, got: {footer_row:?}"
        );
        assert!(
            !footer_row.contains("LSCopy"),
            "footer must not contain the jump status message, got: {footer_row:?}"
        );
        // Status message sits on the row above the footer.
        assert!(
            status_row.contains("LSCopyApplicationURLsForBundleIdentifier"),
            "status row above footer should show the jump message, got: {status_row:?}"
        );
    }

    /// Dump the full 100-column frame to stdout (run with `-- --nocapture`).
    /// Not an assertion test — purely for visual verification.
    #[test]
    fn dump_100_col_frame_ascii() {
        let width: u16 = 100;
        let height: u16 = 10;
        let buf = render_at_width(width, height, true);
        println!("\n=== 100-col frame dump (width={width}, height={height}) ===");
        for y in 0..height {
            let row = row_string(&buf, width, y);
            println!("|{row}|");
        }
        println!("=== end dump ===\n");
    }

    /// Dump a 60-column frame (second tier with family column) to stdout.
    #[test]
    fn dump_60_col_frame_ascii() {
        let width: u16 = 60;
        let height: u16 = 10;
        let buf = render_at_width(width, height, true);
        println!("\n=== 60-col frame dump (width={width}, height={height}) ===");
        for y in 0..height {
            let row = row_string(&buf, width, y);
            println!("|{row}|");
        }
        println!("=== end dump ===\n");
    }

    // ── Family label column tests ─────────────────────────────────────────────

    /// At ≥60 cols, the row should contain the short family label "Claude".
    #[test]
    fn claude_family_label_appears_at_100_cols() {
        let session = make_session("s1", "working", Some("Editing main.rs"));
        let plan = layout::columns_for(100);
        let selection = Selection::new();
        let theme = Theme::classic();
        let anim = Anim::new();

        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 1));
        render_row(
            &mut buf,
            Rect::new(0, 0, 100, 1),
            &session,
            &plan,
            0,
            &selection,
            &theme,
            &anim,
        );

        let row: String = (0..100)
            .map(|x| {
                buf.cell((x, 0))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_default()
            })
            .collect();
        assert!(
            row.contains("Claude"),
            "row at 100 cols should contain 'Claude' family label, got: {row:?}"
        );
    }

    /// At ≥60 cols with Codex family, the row should contain "Codex".
    #[test]
    fn codex_family_label_appears_at_60_cols() {
        let mut session = make_session("s1", "working", Some("running tests"));
        session.family = AgentFamily::Codex;
        let plan = layout::columns_for(60);
        let selection = Selection::new();
        let theme = Theme::classic();
        let anim = Anim::new();

        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 1));
        render_row(
            &mut buf,
            Rect::new(0, 0, 60, 1),
            &session,
            &plan,
            0,
            &selection,
            &theme,
            &anim,
        );

        let row: String = (0..60)
            .map(|x| {
                buf.cell((x, 0))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_default()
            })
            .collect();
        assert!(
            row.contains("Codex"),
            "row at 60 cols should contain 'Codex' family label, got: {row:?}"
        );
    }

    /// Family label should use the family colour (RGB).
    #[test]
    fn family_label_has_family_color() {
        use crate::tui::theme;
        let session = make_session("s1", "working", None);
        let plan = layout::columns_for(100);
        let selection = Selection::new();
        let th = Theme::classic();
        let anim = Anim::new();

        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 1));
        render_row(
            &mut buf,
            Rect::new(0, 0, 100, 1),
            &session,
            &plan,
            0,
            &selection,
            &th,
            &anim,
        );

        // 'C' of "Claude" starts at col 4 (selbar=2 + icon=2)
        let c_col = (0..100u16)
            .find(|&x| cell_symbol(&buf, x, 0) == "C")
            .expect("'C' of 'Claude' should be in buffer");
        let fg = cell_fg(&buf, c_col, 0);
        assert_eq!(
            fg,
            theme::COLOR_CLAUDE,
            "family label 'C' fg should be COLOR_CLAUDE, got {fg:?}"
        );
    }

    /// Below 60 cols (width=40), no family label should appear.
    #[test]
    fn no_family_label_below_60_cols() {
        let session = make_session("s1", "idle", None);
        let plan = layout::columns_for(40);
        let selection = Selection::new();
        let theme = Theme::classic();
        let anim = Anim::new();

        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        render_row(
            &mut buf,
            Rect::new(0, 0, 40, 1),
            &session,
            &plan,
            0,
            &selection,
            &theme,
            &anim,
        );

        let row: String = (0..40)
            .map(|x| {
                buf.cell((x, 0))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_default()
            })
            .collect();
        assert!(
            !row.contains("Claude"),
            "row at 40 cols should NOT contain 'Claude' family label, got: {row:?}"
        );
    }

    // ── Hybrid colour scheme assertions ──────────────────────────────────────
    //
    // These tests verify that structural elements use adaptive colours (Reset /
    // DarkGray) and semantic state elements retain their fixed RGB values.

    /// Helper: cell fg colour.
    fn cell_fg(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> ratatui::style::Color {
        buf.cell((x, y))
            .map(|c| c.style().fg.unwrap_or(ratatui::style::Color::Reset))
            .unwrap_or(ratatui::style::Color::Reset)
    }

    /// Helper: find the first column whose symbol matches `target` on row `y`.
    fn find_symbol_col(
        buf: &ratatui::buffer::Buffer,
        width: u16,
        y: u16,
        target: &str,
    ) -> Option<u16> {
        (0..width).find(|&x| cell_symbol(buf, x, y) == target)
    }

    /// The name column (session name) should have NO explicit fg — i.e. Reset
    /// (terminal default foreground).
    #[test]
    fn name_cell_has_no_explicit_fg() {
        let session = make_session("s1", "working", None);
        let plan = layout::columns_for(100);
        let selection = Selection::new();
        let theme = Theme::classic();
        let anim = Anim::new();

        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 1));
        render_row(
            &mut buf,
            Rect::new(0, 0, 100, 1),
            &session,
            &plan,
            0,
            &selection,
            &theme,
            &anim,
        );

        // "my-repo" starts at column plan.selbar + plan.icon + plan.family = 2 + 2 + 7 = 11
        // (selbar=2, icon=2, family=7 for width≥100).
        // Just find first 'm' of "my-repo" and check its fg.
        let name_col = (0..100u16)
            .find(|&x| cell_symbol(&buf, x, 0) == "m")
            .expect("name 'm' should be in buffer");
        let fg = cell_fg(&buf, name_col, 0);
        assert_eq!(
            fg,
            ratatui::style::Color::Reset,
            "name cell fg should be Reset (terminal default), got {fg:?}"
        );
    }

    /// The approval state glyph (▲) should be coloured with COLOR_APPROVAL RGB.
    #[test]
    fn approval_glyph_has_semantic_rgb() {
        let session = make_session("s1", "approval", None);
        let plan = layout::columns_for(100);
        let selection = Selection::new();
        let theme = Theme::classic();
        let anim = Anim::new();

        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 1));
        render_row(
            &mut buf,
            Rect::new(0, 0, 100, 1),
            &session,
            &plan,
            0,
            &selection,
            &theme,
            &anim,
        );

        let glyph_col = find_symbol_col(&buf, 100, 0, "▲").expect("▲ should appear");
        let fg = cell_fg(&buf, glyph_col, 0);
        // Approval colour is Rgb(0xe3, 0xb3, 0x41) — breathing anim may interpolate,
        // but at t=0 (new Anim) it should be near bright end; just check it's an RGB.
        assert!(
            matches!(fg, ratatui::style::Color::Rgb(_, _, _)),
            "approval glyph fg should be RGB (semantic), got {fg:?}"
        );
    }

    /// The header border characters (╭, ─, ╮) should be DarkGray.
    #[test]
    fn header_border_is_dark_gray() {
        let buf = render_at_width(100, 10, true);
        // First cell on header row is ╭
        let fg = cell_fg(&buf, 0, 0);
        assert_eq!(
            fg,
            ratatui::style::Color::DarkGray,
            "header ╭ should be DarkGray, got {fg:?}"
        );
        // Last cell is ╮
        let fg_last = cell_fg(&buf, 99, 0);
        assert_eq!(
            fg_last,
            ratatui::style::Color::DarkGray,
            "header ╮ should be DarkGray, got {fg_last:?}"
        );
    }

    /// The footer border characters (╰, ─, ╯) should be DarkGray.
    #[test]
    fn footer_border_is_dark_gray() {
        let buf = render_at_width(100, 10, true);
        let footer_y = 9u16;
        let fg_first = cell_fg(&buf, 0, footer_y);
        assert_eq!(
            fg_first,
            ratatui::style::Color::DarkGray,
            "footer ╰ should be DarkGray, got {fg_first:?}"
        );
        let fg_last = cell_fg(&buf, 99, footer_y);
        assert_eq!(
            fg_last,
            ratatui::style::Color::DarkGray,
            "footer ╯ should be DarkGray, got {fg_last:?}"
        );
    }

    /// Working-state mid column text should have Reset fg (active text).
    #[test]
    fn working_mid_text_has_reset_fg() {
        let session = make_session("s1", "working", Some("Editing main.rs"));
        let plan = layout::columns_for(100);
        let selection = Selection::new();
        let theme = Theme::classic();
        let anim = Anim::new();

        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 1));
        render_row(
            &mut buf,
            Rect::new(0, 0, 100, 1),
            &session,
            &plan,
            0,
            &selection,
            &theme,
            &anim,
        );

        // Find first char of "Editing" in the buffer
        let e_col = find_symbol_col(&buf, 100, 0, "E").expect("'E' of 'Editing' should appear");
        let fg = cell_fg(&buf, e_col, 0);
        assert_eq!(
            fg,
            ratatui::style::Color::Reset,
            "working mid text fg should be Reset, got {fg:?}"
        );
    }

    /// Idle-state mid column text (from last_prompt) should have DarkGray fg (secondary).
    #[test]
    fn idle_mid_text_has_dark_gray_fg() {
        // last_prompt contains a unique char 'Q' not present in any other column.
        let mut session = make_session("s1", "idle", None);
        session.last_prompt = Some("Qxyzzy task".to_string());
        let plan = layout::columns_for(100);
        let selection = Selection::new();
        let theme = Theme::classic();
        let anim = Anim::new();

        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 1));
        render_row(
            &mut buf,
            Rect::new(0, 0, 100, 1),
            &session,
            &plan,
            0,
            &selection,
            &theme,
            &anim,
        );

        // 'Q' only appears in "Qxyzzy task" (our last_prompt), not in any other column.
        let col = find_symbol_col(&buf, 100, 0, "Q").expect("'Q' of last_prompt should appear");
        let fg = cell_fg(&buf, col, 0);
        assert_eq!(
            fg,
            ratatui::style::Color::DarkGray,
            "idle mid text fg should be DarkGray, got {fg:?}"
        );
    }

    // ── Fix C: Done/Idle mid column tests ────────────────────────────────────

    /// Done state with last_prompt: mid column shows the prompt text.
    #[test]
    fn done_with_last_prompt_shows_prompt_in_mid() {
        let mut session = make_session("s1", "done", None);
        session.last_prompt = Some("Fix the checkout bug".to_string());
        let plan = layout::columns_for(100);
        let selection = Selection::new();
        let theme = Theme::classic();
        let anim = Anim::new();

        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 1));
        render_row(
            &mut buf,
            Rect::new(0, 0, 100, 1),
            &session,
            &plan,
            0,
            &selection,
            &theme,
            &anim,
        );

        let row: String = (0..100)
            .map(|x| {
                buf.cell((x, 0))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_default()
            })
            .collect();
        assert!(
            row.contains("Fix the checkout bug"),
            "done row with last_prompt should show prompt text in mid column, got: {row:?}"
        );
        // Must NOT show the repo name (my-repo) a second time in the mid column.
        // The name column already shows it; check the row does not have it twice.
        let occurrences = row.matches("my-repo").count();
        assert_eq!(
            occurrences, 1,
            "repo name should appear exactly once (in name column), not duplicated, got: {row:?}"
        );
    }

    /// Done state without last_prompt: mid column is empty (no repo name fallback).
    #[test]
    fn done_without_last_prompt_mid_is_empty_not_name() {
        let session = make_session("s1", "done", None);
        // last_prompt is None (the default from make_session)
        let plan = layout::columns_for(100);
        let selection = Selection::new();
        let theme = Theme::classic();
        let anim = Anim::new();

        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 1));
        render_row(
            &mut buf,
            Rect::new(0, 0, 100, 1),
            &session,
            &plan,
            0,
            &selection,
            &theme,
            &anim,
        );

        let row: String = (0..100)
            .map(|x| {
                buf.cell((x, 0))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_default()
            })
            .collect();
        // Repo name should appear exactly once (name column only), not duplicated in mid.
        let occurrences = row.matches("my-repo").count();
        assert_eq!(
            occurrences, 1,
            "repo name must appear only once (name col); mid col must be empty for done+no last_prompt, got: {row:?}"
        );
    }
}
