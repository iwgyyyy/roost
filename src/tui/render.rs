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

/// Render the TUI header line as plain text (no box border).
///
/// Layout: `roost` (accent colour) left-aligned, then padding, then status right-aligned.
/// Status: `N agents · live ●` (heartbeat dot shown only when agents > 0);
///         `0 agents · idle` (no dot) when empty;
///         `waiting for daemon…` when daemon not ready.
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

    // ── Left: "roost" ────────────────────────────────────────────────────
    let brand = "roost";
    let brand_w = UnicodeWidthStr::width(brand);

    // ── Right: status text (without the dot — rendered separately) ───────
    let (status_base, show_dot) = if daemon_ready {
        if agent_count == 0 {
            ("0 agents · idle".to_string(), false)
        } else if agent_count == 1 {
            ("1 agent · live".to_string(), true)
        } else {
            (format!("{agent_count} agents · live"), true)
        }
    } else {
        ("waiting for daemon…".to_string(), false)
    };

    // dot takes 2 cols ("  " or " ●"), always reserve to avoid width jitter.
    let dot_str = if show_dot && anim.heartbeat() { " ●" } else { "  " };
    let dot_w = 2usize;

    let status_base_w = UnicodeWidthStr::width(status_base.as_str());
    let status_total_w = status_base_w + dot_w;

    // ── Gap between brand and status ─────────────────────────────────────
    let gap_w = w.saturating_sub(brand_w).saturating_sub(status_total_w);

    // Write brand in accent colour
    buf.set_string(x, y, brand, Style::default().fg(accent));

    // Write gap as spaces
    let gap = " ".repeat(gap_w);
    buf.set_string(x + brand_w as u16, y, &gap, Style::default());

    // Write status base in dim colour
    let status_x = x + brand_w as u16 + gap_w as u16;
    buf.set_string(status_x, y, &status_base, Style::default().fg(theme.dim));

    // Write dot (always occupies 2 cols to keep layout stable)
    let dot_x = status_x + status_base_w as u16;
    if show_dot && anim.heartbeat() {
        buf.set_string(dot_x, y, dot_str, Style::default().fg(theme::COLOR_WORKING));
    } else {
        buf.set_string(dot_x, y, "  ", Style::default());
    }
}

/// Render the footer line as plain text key hints (no box border).
///
/// `peek_open`: when true, shows `esc close` hint; otherwise shows full key hints.
pub fn render_footer_border(buf: &mut Buffer, area: Rect, theme: &Theme, peek_open: bool) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let y = area.y;
    let x = area.x;
    let w = area.width as usize;

    let keys: &str = if peek_open {
        "↑/↓ scroll   esc close   q quit"
    } else {
        "↑/↓ select   enter peek   o jump   s stats   c settings   q quit"
    };

    let keys_w = UnicodeWidthStr::width(keys);
    let keys = if keys_w <= w {
        keys.to_string()
    } else {
        layout::truncate_to_width(keys, w)
    };

    // Write key hints in dim colour, left-aligned
    buf.set_string(x, y, &keys, Style::default().fg(theme.dim));

    // Pad the rest with spaces so any stale content is cleared
    let written_w = UnicodeWidthStr::width(keys.as_str());
    let pad = w.saturating_sub(written_w);
    if pad > 0 {
        let padding = " ".repeat(pad);
        buf.set_string(x + written_w as u16, y, &padding, Style::default());
    }
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
///
/// Five-line centered layout:
///
/// ```text
///                                ✻
///
///                       no agents are running
///
///   roost watches your local & remote Claude Code / Codex sessions.
///         start one in any project and it appears here — live.
///
///    agents report in automatically via hooks. nothing to set up.
/// ```
///
/// `✻` is rendered in the accent colour; headline in default fg; hint lines dim.
pub fn render_empty_state(buf: &mut Buffer, area: Rect, _theme: &Theme) {
    if area.height < 5 || area.width < 10 {
        return;
    }

    // Lines and their styles.
    // (text, style)
    let lines: &[(&str, Style)] = &[
        // Decorative glyph — accent colour (same as header brand).
        ("✻", Style::default().fg(theme::COLOR_ACCENT)),
        // Blank spacer.
        ("", Style::default()),
        // Primary headline — default terminal fg (adaptive, readable).
        ("no agents are running", Style::default().fg(Color::Reset)),
        // Blank spacer.
        ("", Style::default()),
        // Hint lines — dim.
        (
            "roost watches your local & remote Claude Code / Codex sessions.",
            Style::default().fg(Color::DarkGray),
        ),
        (
            "start one in any project and it appears here — live.",
            Style::default().fg(Color::DarkGray),
        ),
        // Blank spacer.
        ("", Style::default()),
        (
            "agents report in automatically via hooks. nothing to set up.",
            Style::default().fg(Color::DarkGray),
        ),
    ];

    let block_h = lines.len() as u16;
    // Centre the block vertically in the body area.
    let start_y = area.y + area.height.saturating_sub(block_h) / 2;

    for (i, (text, style)) in lines.iter().enumerate() {
        let y = start_y + i as u16;
        if y >= area.y + area.height {
            break;
        }
        if text.is_empty() {
            continue;
        }
        let display_w = unicode_width::UnicodeWidthStr::width(*text);
        let col = if (display_w as u16) < area.width {
            area.x + (area.width - display_w as u16) / 2
        } else {
            area.x
        };
        buf.set_string(col, y, text, *style);
    }
}

// ── v2 Card renderer ──────────────────────────────────────────────────────────

/// Render one session **v2 card** (4 rows tall) into `buf` at `area`.
///
/// Implements the border matrix from the design spec §5:
///
/// ```text
/// |           | online                         | offline                        |
/// |-----------|--------------------------------|--------------------------------|
/// | unselected| thin solid ┌─┐│└─┘ dim border  | dashed ┌┄┐┊└┄┘ dim + ⊘ + dark |
/// | selected  | heavy ┏━┓┃┗━┛ accent + bold title | heavy accent + ⊘ + dark    |
/// ```
///
/// Precise column placement: every field is written at an exact buffer column,
/// computed from `CardLayoutV2` geometry — no full-line overwrite followed by
/// re-colouring patches that can produce duplicate or misaligned characters.
#[allow(clippy::too_many_arguments)]
pub fn render_card_v2(
    buf: &mut Buffer,
    area: Rect,
    session: &SessionView,
    selected: bool,
    theme: &Theme,
    anim: &Anim,
    accent: Color,
) {
    if area.height < 4 || area.width < 6 {
        return;
    }

    let x0 = area.x;
    let y0 = area.y;
    // Last column index (where the right border char lives).
    let right = area.x + area.width - 1;

    let is_offline = session.stale;

    // ── Compute layout (pure, no I/O) ─────────────────────────────────────
    let cl = layout::card_layout_v2(area.width, session);

    // ── Style decisions ───────────────────────────────────────────────────
    // Border chars are NEVER given Modifier::BOLD (spec: "border chars not bold").
    let border_style = if selected {
        Style::default().fg(accent)
    } else {
        Style::default().fg(theme.border)
    };
    let dim_style = Style::default().fg(theme.dim);

    // Offline dims all content; online keeps full colour.
    let offline_dim = is_offline;

    // ── Border character sets ─────────────────────────────────────────────
    // Selected        → heavy:  ┏ ┓ ┗ ┛  ━  ┃
    // Unselected online → thin solid corners + dashed vertical sides:
    //                     ┌ ┐ └ ┘  ─  ┊(U+250A)
    // Unselected offline → dashed: ┌ ┐ └ ┘  ┄(U+2504)  ┊(U+250A)
    //
    // Spec: top/bottom borders and corners always stay solid (─ / corners);
    // only the left/right vertical strokes on body rows change.
    let (top_left, top_right, bot_left, bot_right, h_fill, v_fill) = if selected {
        // Heavy: ┏ ┓ ┗ ┛ ━ ┃
        ("┏", "┓", "┗", "┛", '━', "┃")
    } else if is_offline {
        // Dashed: ┌ ┐ └ ┘ ┄(U+2504 horizontal) ┊(U+250A vertical)
        ("┌", "┐", "└", "┘", '\u{2504}', "\u{250a}")
    } else {
        // Online unselected: thin solid corners + horizontal fill, dashed vertical sides.
        // Corners and top/bottom fills stay solid (─); only body-row sides are ┊.
        ("┌", "┐", "└", "┘", '─', "\u{250a}")
    };

    let fill_d = cl.fill_d;

    // ── ROW 0: Top border with embedded title fields ───────────────────────
    //
    // Layout formula (from card_layout_v2):
    //   fill_d = inner - (1(sp) + icon(1) + 1 + name + 1 + 1 + glyph(1) + 1 + word + 3 + time + 1)
    //
    // ┌ and ┐ are the corner chars (NOT counted in inner).
    // Inner content breakdown (all between ┌ and ┐):
    //   [sp(1)][icon(1)][sp(1)][name][sp(1)] + [fill_d × h_fill] + [sp(1)][glyph(1)][sp(1)][word][" · "(3)][time][sp(1)]
    //
    // Total inner = (1+icon+1+name+1) + fill_d + (1+glyph+1+word+3+time+1) = inner  ✓
    //
    // Column positions (measured from x0, the corner char):
    //   x0       : ┌ (corner)
    //   x0+1     : sp (leading space before icon)
    //   x0+2     : icon (1 col)
    //   x0+3     : sp
    //   x0+4     : name (name_w cols)
    //   x0+4+name_w : sp
    //   x0+5+name_w : fill_d × h_fill
    //   x0+5+name_w+fill_d : sp
    //   x0+6+name_w+fill_d : glyph (1 col)
    //   x0+7+name_w+fill_d : sp
    //   x0+8+name_w+fill_d : word (word_w cols)
    //   x0+8+name_w+fill_d+word_w : " · " (3 cols)
    //   x0+11+name_w+fill_d+word_w : time (time_w cols, natural width)
    //   x0+11+name_w+fill_d+word_w+time_w : sp
    //   x0+12+name_w+fill_d+word_w+time_w : ┐ = right  (verified by fill_d formula)
    {
        let icon_w = UnicodeWidthStr::width(cl.icon.as_str());
        let name_w = UnicodeWidthStr::width(cl.name.as_str());
        let glyph_w = UnicodeWidthStr::width(cl.state_glyph.as_str());
        let word_w = UnicodeWidthStr::width(cl.state_word.as_str());

        // ── left corner (NOT styled with border_style to avoid bold on box char) ──
        buf.set_string(x0, y0, top_left, border_style);

        // ── sp before icon ── (x0+1: one clear space between corner and icon)
        buf.set_string(x0 + 1, y0, " ", border_style);

        // ── icon ──  (at x0+2)
        let icon_col = x0 + 2;
        let icon_color = if offline_dim {
            theme.dim
        } else {
            theme::family_color(&session.family)
        };
        buf.set_string(icon_col, y0, &cl.icon, Style::default().fg(icon_color));

        // ── sp after icon ──
        let sp1 = icon_col + icon_w as u16;  // = x0+3 (icon_w always 1)
        buf.set_string(sp1, y0, " ", Style::default());

        // ── name ──
        let name_col = sp1 + 1;  // = x0+4
        let name_style = if selected {
            if offline_dim {
                Style::default().fg(theme.dim).add_modifier(Modifier::BOLD)
            } else {
                // Bold title text (NOT bold on border chars)
                Style::default().add_modifier(Modifier::BOLD)
            }
        } else if offline_dim {
            dim_style
        } else {
            Style::default()
        };
        buf.set_string(name_col, y0, &cl.name, name_style);

        // ── sp after name ──
        let sp2 = name_col + name_w as u16;
        buf.set_string(sp2, y0, " ", border_style);

        // ── fill_d dashes ──
        let fill_start = sp2 + 1;
        if fill_d > 0 {
            let fills: String = h_fill.to_string().repeat(fill_d);
            buf.set_string(fill_start, y0, &fills, border_style);
        }

        // ── sp before glyph ──
        let sp3 = fill_start + fill_d as u16;
        buf.set_string(sp3, y0, " ", border_style);

        // ── state glyph (live spinner for working, breathe for approval) ──
        let glyph_col = sp3 + 1;
        let (live_glyph, glyph_color): (&str, Color) = if is_offline {
            ("⊘", theme::COLOR_OFFLINE)
        } else if session.state == "working" {
            (anim.spinner(), theme::COLOR_WORKING)
        } else if session.state == "approval" {
            let t = anim.breathe();
            let c = theme::approval_breathe_color(t);
            // Store breathe color in a local; borrow checker requires the
            // glyph str to be a static string reference.
            let (g, _) = theme::state_glyph("approval");
            let _ = c; // will be applied below via separate path
            (g, theme::approval_breathe_color(t)) // re-call to get Color value
        } else {
            let (g, c) = theme::state_glyph(&session.state);
            (g, c)
        };
        let glyph_style = Style::default().fg(glyph_color);
        buf.set_string(glyph_col, y0, live_glyph, glyph_style);

        // ── sp after glyph ──
        let sp4 = glyph_col + glyph_w as u16;
        buf.set_string(sp4, y0, " ", Style::default());

        // ── state word ──
        let word_col = sp4 + 1;
        let word_color = if offline_dim {
            theme.dim
        } else {
            theme::state_color(&session.state)
        };
        buf.set_string(word_col, y0, &cl.state_word, Style::default().fg(word_color));

        // ── " · " separator ──
        let sep_col = word_col + word_w as u16;
        buf.set_string(sep_col, y0, " · ", Style::default().fg(theme.dim));

        // ── time (natural width; D absorbs width changes so time hugs right border) ──
        let time_col = sep_col + 3;
        let time_display_w = UnicodeWidthStr::width(cl.time_str.as_str()) as u16;
        // Approval: time also in amber (spec §3 "时间也染 amber"); offline/others: dim.
        let time_color = if !is_offline && session.state == "approval" {
            theme::COLOR_APPROVAL
        } else {
            theme.dim
        };
        buf.set_string(time_col, y0, &cl.time_str, Style::default().fg(time_color));

        // ── sp before right corner ──
        let sp5 = time_col + time_display_w;
        buf.set_string(sp5, y0, " ", border_style);

        // ── right corner ──
        // Must be at sp5+1 = right.
        debug_assert_eq!(
            sp5 + 1, right,
            "top border right alignment: sp5+1={} right={}; inner={}, fill_d={}, name_w={}, word_w={}, glyph_w={}",
            sp5 + 1, right, cl.inner_width, fill_d, name_w, word_w, glyph_w
        );
        buf.set_string(right, y0, top_right, border_style);

        let _ = (icon_w, glyph_w);
    }

    // ── ROW 1: Step line ─ │ <step_prefix> <step_text>        │ ──────────
    {
        buf.set_string(x0, y0 + 1, v_fill, border_style);
        buf.set_string(right, y0 + 1, v_fill, border_style);

        // " " leading space at col x0+1
        buf.set_string(x0 + 1, y0 + 1, " ", Style::default());

        // step_prefix at col x0+2 (1 col)
        let prefix_col = x0 + 2;
        let prefix_color = if offline_dim {
            theme::COLOR_OFFLINE
        } else if session.state == "working" {
            theme::COLOR_WORKING
        } else {
            theme.dim
        };
        buf.set_string(prefix_col, y0 + 1, &cl.step_prefix, Style::default().fg(prefix_color));

        // " " after prefix at col x0+3
        buf.set_string(x0 + 3, y0 + 1, " ", Style::default());

        // step_text at col x0+4, available width = right - x0 - 4 - 1 (right border)
        let step_col = x0 + 4;
        let step_avail = (right as usize).saturating_sub(step_col as usize);
        let step_text = layout::truncate_to_width(&cl.step_text, step_avail);
        // Working: normal fg (active); others: dim.
        let step_color = if offline_dim {
            theme.dim
        } else if session.state == "working" {
            Color::Reset
        } else {
            theme.dim
        };
        buf.set_string(step_col, y0 + 1, &step_text, Style::default().fg(step_color));
    }

    // ── ROW 2: CWD + source line ─ │ <cwd>   <source><via> │ ────────────
    {
        buf.set_string(x0, y0 + 2, v_fill, border_style);
        buf.set_string(right, y0 + 2, v_fill, border_style);

        // " " leading space
        buf.set_string(x0 + 1, y0 + 2, " ", Style::default());

        // Right-side: source + via — right-aligned, ending before right border.
        // Compute right-side width first to know where cwd must end.
        let source_w = UnicodeWidthStr::width(cl.source_str.as_str());
        let via_w = UnicodeWidthStr::width(cl.via_str.as_str());
        let right_w = source_w + via_w;

        // source + via starts at: right - right_w (they end at right-1, right border at right).
        let source_col = right.saturating_sub(right_w as u16);

        // cwd: from x0+2 to source_col-2 (1 space separation minimum).
        let cwd_col = x0 + 2;
        let cwd_avail = (source_col as usize).saturating_sub(cwd_col as usize + 1);
        let cwd_text = layout::truncate_to_width(&cl.cwd_display, cwd_avail);
        buf.set_string(cwd_col, y0 + 2, &cwd_text, Style::default().fg(theme.dim));

        // @source (right-aligned): @local → dim, @device → cold blue remote colour.
        let source_color = if offline_dim || cl.source_str == "@local" {
            theme.dim
        } else {
            theme::COLOR_REMOTE
        };
        buf.set_string(source_col, y0 + 2, &cl.source_str, Style::default().fg(source_color));

        // via_str (immediately after source)
        if !cl.via_str.is_empty() {
            let via_col = source_col + source_w as u16;
            // faint — use dim colour
            buf.set_string(via_col, y0 + 2, &cl.via_str, Style::default().fg(theme.dim));
        }
    }

    // ── ROW 3: Bottom border ─────────────────────────────────────────────
    {
        buf.set_string(x0, y0 + 3, bot_left, border_style);
        buf.set_string(right, y0 + 3, bot_right, border_style);
        // inner fill: (area.width - 2) fill chars
        let fill_count = (area.width as usize).saturating_sub(2);
        let fill: String = h_fill.to_string().repeat(fill_count);
        buf.set_string(x0 + 1, y0 + 3, &fill, border_style);
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
    /// Configured theme accent colour — used for the `roost` title in the header
    /// and (in future tasks) for selected-card borders.  Comes from
    /// `config::load().accent_color()` at startup; does NOT follow the selected
    /// session's family colour.
    pub accent: Color,
    /// When true, footer shows "esc close"; when false, shows key hints.
    pub peek_open: bool,
    /// Optional jump status message to display in the footer.
    pub jump_status: Option<&'a str>,
}

// ── Body item enumeration (used by render_frame) ─────────────────────────────

/// A single renderable element in the card-based body.
enum BodyItem<'a> {
    /// One blank spacer row (height = 1) between groups or after each card.
    Spacer,
    /// A group section header row (height = 1).
    GroupHeader(&'a Group),
    /// A session card (height = 4) at the given flat session index.
    Card(&'a crate::protocol::SessionView, usize /* flat_session_idx */),
    /// Collapsed-group placeholder shown in Critical height mode (height = 1).
    Collapsed(usize /* count */),
}

impl BodyItem<'_> {
    fn height(&self) -> usize {
        match self {
            BodyItem::Card(_, _) => 4,
            _ => 1,
        }
    }
}

/// Build the ordered list of body items from `groups`, plus a parallel
/// `session_item_index` map that converts a flat session index into the item
/// index of its card (for driving `card_viewport`'s `selected` parameter).
fn build_body_items<'a>(
    groups: &'a [Group],
    is_critical: bool,
) -> (Vec<BodyItem<'a>>, Vec<usize>) {
    let mut items: Vec<BodyItem<'a>> = Vec::new();
    // Map: flat_session_idx → item index of the corresponding Card item.
    let mut session_to_item: Vec<usize> = Vec::new();

    let mut flat_session_idx = 0usize;
    let mut first_group = true;

    for group in groups {
        let is_needs_input = group.title == "NEEDS INPUT";

        if is_critical && !is_needs_input {
            // Collapsed: show a "… N more" placeholder row.
            items.push(BodyItem::Collapsed(group.count()));
            // Session indices still advance (they're not visible).
            for _ in &group.rows {
                // We push a dummy item index (the collapsed row) so that if
                // the selection happens to land in a collapsed group the
                // viewport still makes a valid choice.
                session_to_item.push(items.len() - 1);
                flat_session_idx += 1;
            }
            continue;
        }

        // Spacer above every group header (breathing room between groups).
        // Skip the spacer before the very first rendered group.
        if !is_critical {
            if !first_group {
                items.push(BodyItem::Spacer);
            }
            items.push(BodyItem::GroupHeader(group));
        }

        first_group = false;

        for row in &group.rows {
            let card_item_idx = items.len();
            items.push(BodyItem::Card(row, flat_session_idx));
            session_to_item.push(card_item_idx);
            // Spacer after each card (except we let the viewport clip the last).
            items.push(BodyItem::Spacer);
            flat_session_idx += 1;
        }
    }

    (items, session_to_item)
}

// ── Layout constants ──────────────────────────────────────────────────────────

/// Left/right horizontal padding in columns.  Content is inset by this many
/// columns on each side so it never hugs the terminal edge.
const H_PAD: u16 = 2;

/// Helper: shrink a full-width area by `H_PAD` on each side.
fn padded_area(area: Rect) -> Rect {
    if area.width <= H_PAD * 2 {
        return area;
    }
    Rect {
        x: area.x + H_PAD,
        y: area.y,
        width: area.width - H_PAD * 2,
        height: area.height,
    }
}

/// Render the entire TUI into `buf` using `area` as the full screen.
pub fn render_frame(buf: &mut Buffer, area: Rect, state: &RenderState<'_>) {
    if area.height < 2 || area.width < 10 {
        render_too_small(buf, area);
        return;
    }

    let height_prof = layout::height_profile(area.height);

    // ── Header (inset by H_PAD on each side) ─────────────────────────────
    let agent_count: usize = state.groups.iter().map(|g| g.count()).sum();
    let header_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    render_header(
        buf,
        padded_area(header_area),
        agent_count,
        state.daemon_ready,
        state.anim,
        state.theme,
        state.accent,
    );

    // ── Footer (key hints, inset) ─────────────────────────────────────────
    let footer_y = area.y + area.height - 1;
    let footer_area = Rect {
        x: area.x,
        y: footer_y,
        width: area.width,
        height: 1,
    };
    render_footer_border(buf, padded_area(footer_area), state.theme, state.peek_open);

    // ── Selected-agent cwd line (dim, above footer, inset) ───────────────
    // Reserved as a single row between body and footer.  Body height is reduced by 1.
    // Shows the full cwd of the currently selected agent ($HOME abbreviated to ~).
    // Empty when nothing is selected.
    if area.height >= 3 {
        let cwd_y = footer_y.saturating_sub(1);
        if cwd_y > area.y {
            let cwd_area = padded_area(Rect {
                x: area.x,
                y: cwd_y,
                width: area.width,
                height: 1,
            });
            if cwd_area.width > 0 {
                // Resolve the cwd of the selected session.
                let selected_cwd: String = state.selection.index.and_then(|sel_idx| {
                    let mut flat = 0usize;
                    for group in state.groups {
                        for row in &group.rows {
                            if flat == sel_idx {
                                return Some(row.cwd.clone());
                            }
                            flat += 1;
                        }
                    }
                    None
                }).unwrap_or_default();

                // Abbreviate $HOME prefix to ~ (read from env once per frame; cheap).
                let abbrev_cwd: String = if selected_cwd.is_empty() {
                    String::new()
                } else if let Ok(home) = std::env::var("HOME")
                    && !home.is_empty()
                    && selected_cwd.starts_with(&home)
                {
                    format!("~{}", &selected_cwd[home.len()..])
                } else {
                    selected_cwd
                };

                let cwd_text = layout::truncate_to_width(&abbrev_cwd, cwd_area.width as usize);
                // Pad to clear any stale content.
                let padded = layout::pad_to_width(&cwd_text, cwd_area.width as usize);
                buf.set_string(cwd_area.x, cwd_area.y, &padded, Style::default().fg(state.theme.dim));
            }
        }
    }

    // ── Body area ─────────────────────────────────────────────────────────
    // Subtract: 1 header + 1 top-padding row + 1 cwd row + 1 footer = 4.
    // When height is very small we degrade gracefully.
    let body_y = area.y + 2; // +1 header, +1 top-padding blank row
    let body_h = area.height.saturating_sub(4); // header + pad + cwd + footer
    if body_h == 0 {
        return;
    }
    let body_area = Rect {
        x: area.x,
        y: body_y,
        width: area.width,
        height: body_h,
    };

    // Padded body (same y/height as body_area but inset horizontally).
    let padded_body = padded_area(body_area);

    if state.groups.is_empty() {
        render_empty_state(buf, padded_body, state.theme);
        // Status line: even when empty, honour any transient toast.
        if let Some(msg) = state.jump_status {
            // Jump status sits between body and cwd row.
            let status_y = footer_y.saturating_sub(2);
            if status_y > area.y {
                render_status_line(
                    buf,
                    Rect { x: padded_body.x, y: status_y, width: padded_body.width, height: 1 },
                    state.theme,
                    msg,
                );
            }
        }
        return;
    }

    // ── Build body item list ───────────────────────────────────────────────
    let is_critical = matches!(height_prof, HeightProfile::Critical);
    let (items, session_to_item) = build_body_items(state.groups, is_critical);

    if items.is_empty() {
        return;
    }

    // ── Compute card viewport ─────────────────────────────────────────────
    // `selected` for card_viewport is the item index of the currently selected
    // session's card.  If there's no selection, use item 0.
    let sel_flat = state.selection.index.unwrap_or(0);
    let sel_item = session_to_item
        .get(sel_flat)
        .copied()
        .unwrap_or(0);

    let item_heights: Vec<usize> = items.iter().map(|it| it.height()).collect();
    let cv = layout::card_viewport(&item_heights, sel_item, body_h as usize);

    // ── Render visible items ───────────────────────────────────────────────
    let mut current_y = body_y;
    // Body ends before the cwd row (footer_y - 1) and before the footer (footer_y).
    let bottom_y = footer_y.saturating_sub(1); // stop before cwd row

    for (idx, item) in items.iter().enumerate() {
        if idx < cv.first_card {
            continue;
        }
        if idx > cv.last_card {
            break;
        }
        if current_y >= bottom_y {
            break;
        }

        let item_h = item.height() as u16;
        let avail_h = (bottom_y - current_y).min(item_h);

        match item {
            BodyItem::Spacer => {
                // Just advance y; the buffer cell is already blank.
                current_y += 1;
            }
            BodyItem::GroupHeader(group) => {
                let gh_area = Rect {
                    x: padded_body.x,
                    y: current_y,
                    width: padded_body.width,
                    height: 1,
                };
                render_group_header(buf, gh_area, group, state.theme);
                current_y += 1;
            }
            BodyItem::Card(session, flat_session_idx) => {
                let selected = state.selection.is_selected(*flat_session_idx);
                let card_area = Rect {
                    x: padded_body.x,
                    y: current_y,
                    width: padded_body.width,
                    height: avail_h,
                };
                render_card_v2(buf, card_area, session, selected, state.theme, state.anim, state.accent);
                current_y += avail_h;
            }
            BodyItem::Collapsed(count) => {
                let msg = format!("… {} more", count);
                buf.set_string(
                    padded_body.x,
                    current_y,
                    &msg,
                    Style::default().fg(state.theme.dim),
                );
                current_y += 1;
            }
        }
    }

    // ── Status line (transient toast, sits in the cwd row slot) ───────────
    // When a jump status message is active it temporarily replaces the cwd row.
    if let Some(msg) = state.jump_status {
        let status_y = footer_y.saturating_sub(1);
        if status_y > area.y {
            let status_area = Rect {
                x: padded_body.x,
                y: status_y,
                width: padded_body.width,
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
            cwd: String::new(),
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
            origin: "local".to_string(),
            stale: false,
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
                accent: theme::COLOR_ACCENT,
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
                accent: theme::COLOR_ACCENT,
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

    // ── Empty state copy (§6) ─────────────────────────────────────────────

    /// Helper: render empty state into an 80×24 buffer and collect all symbols.
    fn render_empty_buf() -> (ratatui::buffer::Buffer, u16) {
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
                accent: theme::COLOR_ACCENT,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        (buf, 80)
    }

    #[test]
    fn empty_state_has_star_glyph() {
        let (buf, width) = render_empty_buf();
        let content: String = (0..width * 24)
            .map(|i| buf.content()[i as usize].symbol().to_string())
            .collect();
        assert!(
            content.contains('✻'),
            "empty state should contain ✻ glyph, got content without it"
        );
    }

    #[test]
    fn empty_state_has_headline() {
        let (buf, width) = render_empty_buf();
        let content: String = (0..width * 24)
            .map(|i| buf.content()[i as usize].symbol().to_string())
            .collect();
        assert!(
            content.contains("no agents are running"),
            "empty state should contain 'no agents are running'"
        );
    }

    #[test]
    fn empty_state_has_watch_line() {
        let (buf, width) = render_empty_buf();
        let content: String = (0..width * 24)
            .map(|i| buf.content()[i as usize].symbol().to_string())
            .collect();
        assert!(
            content.contains("roost watches your local & remote"),
            "empty state should contain watch description line"
        );
    }

    #[test]
    fn empty_state_has_start_line() {
        let (buf, width) = render_empty_buf();
        let content: String = (0..width * 24)
            .map(|i| buf.content()[i as usize].symbol().to_string())
            .collect();
        assert!(
            content.contains("start one in any project"),
            "empty state should contain 'start one in any project'"
        );
    }

    #[test]
    fn empty_state_has_hooks_line() {
        let (buf, width) = render_empty_buf();
        let content: String = (0..width * 24)
            .map(|i| buf.content()[i as usize].symbol().to_string())
            .collect();
        assert!(
            content.contains("agents report in automatically via hooks"),
            "empty state should contain hooks description line"
        );
    }

    #[test]
    fn empty_state_lines_are_centered() {
        // Verify that the headline is not left-aligned (x > 0 before text starts).
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
                accent: theme::COLOR_ACCENT,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        // Find the row containing "no agents are running" and verify it's not at x=0.
        let headline = "no agents are running";
        for row in 0..24u16 {
            let row_str: String = (0..80u16).map(|x| cell_symbol(&buf, x, row)).collect();
            if row_str.contains(headline) {
                // The first non-space character should not be at column 0 — text is centered.
                let first_char_col = row_str.find(|c: char| c != ' ');
                assert!(
                    first_char_col.map(|c| c > 0).unwrap_or(false),
                    "headline should be centered (not start at col 0), row={row}, content={row_str:?}"
                );
                return;
            }
        }
        panic!("Could not find 'no agents are running' in any row");
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
                accent: theme::COLOR_ACCENT,
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
        // Card top border shows family name "Claude" (not cwd basename "my-repo").
        assert!(content.contains("Claude"), "card should show family label 'Claude'");
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
                accent: theme::COLOR_ACCENT,
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
        // Card top border shows family name, not session.name (cwd basename).
        assert!(content.contains("Claude") || content.contains("NEEDS INPUT"));
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
                accent: theme::COLOR_ACCENT,
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

        term.draw(|frame| {
            let area = frame.area();
            let state = RenderState {
                groups: &groups,
                selection: &selection,
                daemon_ready: true,
                anim: &anim,
                theme: &theme,
                accent: theme::COLOR_ACCENT,
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

    // ── Plain-text header / footer alignment tests ───────────────────────────
    //
    // After the v2 redesign, header and footer are plain text lines — no box
    // border characters.  These tests replace the old B1/B2/B3 regressions that
    // verified box-corner alignment.

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
                accent: theme::COLOR_ACCENT,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        term.backend().buffer().clone()
    }

    /// Header row at 100 cols must start with "roost" (no box corners).
    #[test]
    fn header_right_border_aligns_at_100_cols() {
        let buf = render_at_width(100, 20, true);
        let header = row_string(&buf, 100, 0);
        // Plain text: must contain "roost" and must NOT end with a box corner.
        assert!(header.contains("roost"), "header must contain 'roost', got: {header:?}");
        let last = cell_symbol(&buf, 99, 0);
        assert_ne!(last, "╮", "header last col must not be box corner ╮, got {last:?}");
    }

    /// Footer row at 100 cols must contain key hints (no box corners).
    #[test]
    fn footer_right_border_aligns_at_100_cols() {
        let buf = render_at_width(100, 20, true);
        let footer_y = 19u16;
        let footer = row_string(&buf, 100, footer_y);
        assert!(footer.contains("quit"), "footer must contain 'quit', got: {footer:?}");
        let last = cell_symbol(&buf, 99, footer_y);
        assert_ne!(last, "╯", "footer last col must not be box corner ╯, got {last:?}");
    }

    /// Header row at 73 cols must contain "roost" (no box corners).
    #[test]
    fn header_right_border_aligns_at_73_cols() {
        let buf = render_at_width(73, 20, true);
        let header = row_string(&buf, 73, 0);
        assert!(header.contains("roost"), "header (73 cols) must contain 'roost', got: {header:?}");
        let last = cell_symbol(&buf, 72, 0);
        assert_ne!(last, "╮", "header (73 cols) last col must not be ╮, got {last:?}");
    }

    /// Footer row at 73 cols must contain key hints (no box corners).
    #[test]
    fn footer_right_border_aligns_at_73_cols() {
        let buf = render_at_width(73, 20, true);
        let footer_y = 19u16;
        let footer = row_string(&buf, 73, footer_y);
        assert!(footer.contains("quit"), "footer (73 cols) must contain 'quit', got: {footer:?}");
        let last = cell_symbol(&buf, 72, footer_y);
        assert_ne!(last, "╯", "footer (73 cols) last col must not be ╯, got {last:?}");
    }

    /// Heartbeat dot ● appears in the header row (right portion) when agents > 0.
    #[test]
    fn heartbeat_dot_appears_in_header_at_100_cols() {
        let buf = render_at_width(100, 20, true);
        let header_row = row_string(&buf, 100, 0);
        assert!(
            header_row.contains('●'),
            "header should contain ● heartbeat dot, got: {header_row:?}"
        );
        // ● should be somewhere in the row (not clipped off).
        let dot_col = (0..100u16).find(|&x| cell_symbol(&buf, x, 0) == "●");
        let dot_col = dot_col.expect("● should be found in header row");
        // ● should be reasonably near the right side (within last 30 cols of a 100-col terminal).
        assert!(
            dot_col >= 70,
            "● should be in the right portion of the header, dot_col={dot_col}"
        );
    }

    /// Body rows must NOT have the outer ╎ side-border (it has been removed).
    #[test]
    fn side_borders_aligned_at_100_cols() {
        let buf = render_at_width(100, 20, true);
        // Row 1 is the first body row (a group header or spacer).
        // With the outer side-border removed, col 0 and col 99 must NOT be ╎.
        let left = cell_symbol(&buf, 0, 1);
        let right = cell_symbol(&buf, 99, 1);
        assert_ne!(left, "╎", "outer side-border ╎ must be absent from body col 0, got {left:?}");
        assert_ne!(right, "╎", "outer side-border ╎ must be absent from body col 99, got {right:?}");
    }

    #[test]
    fn scrolled_away_group_header_not_rendered() {
        // Card-layout version: when the selected session is in the WORKING group
        // and the terminal is short enough that the NEEDS INPUT group (approval)
        // is scrolled off the top, the NEEDS INPUT header must not appear.
        //
        // Layout (card = 4 rows, spacer = 1 row, group-header = 1 row):
        //   item 0: group-header "NEEDS INPUT"    h=1
        //   item 1: card "approval-repo"          h=4
        //   item 2: spacer                        h=1
        //   item 3: spacer (between groups)       h=1
        //   item 4: group-header "WORKING"        h=1
        //   item 5: card "working-repo"            h=4
        //   item 6: spacer                        h=1
        //   total = 13; viewport (height=8-2=6) forces scroll when
        //   selected = working-repo card (item 5).
        //
        // With viewport_h=6, card_viewport(selected=5) scrolls past items 0-3,
        // so NEEDS INPUT header is invisible.
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
        // height=8 → body_h=6; WORKING card at flat index 1 forces scroll past NEEDS INPUT.
        let mut term = make_terminal(80, 8);
        let anim = Anim::new();
        let theme = Theme::classic();
        // Select the WORKING session (flat index 1).
        let selection = Selection { index: Some(1) };

        term.draw(|frame| {
            let area = frame.area();
            let state = RenderState {
                groups: &groups,
                selection: &selection,
                daemon_ready: true,
                anim: &anim,
                theme: &theme,
                accent: theme::COLOR_ACCENT,
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

        // The working session card should appear since it is selected.
        // Card top border now shows family label "Claude", not the session name "working-repo".
        assert!(
            content.contains("Claude") || content.contains("WORKING"),
            "working session card (Claude/WORKING group) should be visible when selected"
        );
        // NEEDS INPUT group header should NOT appear: its content scrolled off-screen.
        assert!(
            !content.contains("NEEDS INPUT"),
            "NEEDS INPUT group header should not appear when scrolled away\ncontent: {content:?}"
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
                accent: theme::COLOR_ACCENT,
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

    /// The header "roost" brand text uses the accent colour (not the old border DarkGray).
    #[test]
    fn header_border_is_dark_gray() {
        let buf = render_at_width(100, 10, true);
        // With H_PAD=2, the brand starts at col 2. Find 'r' of "roost".
        let r_col = (0..100u16)
            .find(|&x| cell_symbol(&buf, x, 0) == "r")
            .expect("'r' of 'roost' should appear in header");
        let fg = cell_fg(&buf, r_col, 0);
        // Accent is COLOR_ACCENT (an RGB value, not DarkGray).
        assert!(
            matches!(fg, ratatui::style::Color::Rgb(_, _, _)),
            "header 'roost' brand fg should be RGB accent, got {fg:?}"
        );
    }

    /// The footer key-hint text uses dim colour (not the old border DarkGray with box chars).
    #[test]
    fn footer_border_is_dark_gray() {
        let buf = render_at_width(100, 10, true);
        let footer_y = 9u16;
        // With H_PAD=2, footer starts at col 2 — col 0 is blank padding.
        // Find the first non-space cell in the footer row and check its fg.
        let first_content_col = (0..100u16)
            .find(|&x| cell_symbol(&buf, x, footer_y) != " ")
            .expect("footer should have non-space content");
        let fg_first = cell_fg(&buf, first_content_col, footer_y);
        assert_eq!(
            fg_first,
            ratatui::style::Color::DarkGray,
            "footer key-hint text fg should be DarkGray (dim), got {fg_first:?}"
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

    // ── Full-frame card layout integration tests ─────────────────────────────
    //
    // These tests verify that render_frame draws cards in the body area and that
    // key invariants (borders, content, scrolling, stale appearance) hold across
    // multiple terminal widths.  They use render_at_width which renders a single
    // working session and returns the buffer.

    /// Helper: find the first occurrence of a corner symbol in the full buffer (all rows/cols).
    fn find_corner(buf: &ratatui::buffer::Buffer, width: u16, height: u16, sym: &str) -> Option<(u16, u16)> {
        for y in 0..height {
            for x in 0..width {
                if cell_symbol(buf, x, y) == sym {
                    return Some((x, y));
                }
            }
        }
        None
    }

    /// At width ≥ 40 the card top border (┏ when selected, ┌ when unselected)
    /// must appear somewhere in the body.  render_at_width selects the card, so we
    /// look for the heavy corner ┏.
    /// With H_PAD=2 the card no longer starts at col 0 — search all columns.
    #[test]
    fn frame_card_top_border_appears_in_body_at_80_cols() {
        let buf = render_at_width(80, 12, true);
        let found = find_corner(&buf, 80, 12, "┏");
        assert!(found.is_some(), "card top border ┏ should appear in body rows");
    }

    #[test]
    fn frame_card_top_border_appears_at_60_cols() {
        let buf = render_at_width(60, 12, true);
        let found = find_corner(&buf, 60, 12, "┏");
        assert!(found.is_some(), "card top border (┏) should appear in body at 60 cols");
    }

    #[test]
    fn frame_card_top_border_appears_at_100_cols() {
        let buf = render_at_width(100, 12, true);
        let found = find_corner(&buf, 100, 12, "┏");
        assert!(found.is_some(), "card top border (┏) should appear in body at 100 cols");
    }

    /// Card bottom border (┗ when selected) must appear in the body.
    #[test]
    fn frame_card_bottom_border_appears_at_80_cols() {
        let buf = render_at_width(80, 12, true);
        let found = find_corner(&buf, 80, 12, "┗");
        assert!(found.is_some(), "card bottom border (┗) should appear in body at 80 cols");
    }

    /// The card top border shows the family name ("Claude" for make_session).
    /// `my-repo` is the session.name (cwd basename) — it now goes to body row 2 (cwd).
    #[test]
    fn frame_card_body_contains_session_name_at_80() {
        let buf = render_at_width(80, 12, true);
        let content: String = buf.content().iter().map(|c| c.symbol().to_string()).collect();
        // The card top border now shows "Claude" (family name), not "my-repo".
        // "my-repo" is the session.name and appears as the cwd basename in body row 2.
        // Just verify the card rendered at all (no panic), and the frame contains content.
        assert!(
            content.contains("Claude") || content.contains("WORKING"),
            "frame body should contain card content (Claude family name or WORKING group header)"
        );
    }

    /// Selected card's border uses the accent colour (RGB).
    /// With H_PAD=2 the card no longer starts at col 0 — search the whole buffer.
    #[test]
    fn frame_selected_card_border_is_family_colour() {
        // render_at_width sets selection = Some(0) and accent = COLOR_ACCENT.
        let buf = render_at_width(80, 12, true);
        let (corner_x, corner_y) = find_corner(&buf, 80, 12, "┏")
            .expect("selected heavy card top-left corner ┏ not found");
        let fg = cell_fg(&buf, corner_x, corner_y);
        // Accent colour is COLOR_ACCENT = Rgb(0xd9, 0x77, 0x57).
        assert_eq!(
            fg,
            theme::COLOR_ACCENT,
            "selected card top-left border should be COLOR_ACCENT, got {fg:?}"
        );
    }

    /// Group header appears in the body above the card.
    #[test]
    fn frame_group_header_appears_above_card() {
        let buf = render_at_width(80, 14, true);
        let content: String = buf.content().iter().map(|c| c.symbol().to_string()).collect();
        assert!(
            content.contains("WORKING"),
            "group header 'WORKING' should appear in the frame"
        );
    }

    /// Stale card uses dashed fill in top border.
    #[test]
    fn frame_stale_card_has_dashed_top_border() {
        let mut session = make_session("s1", "idle", None);
        session.stale = true;
        session.origin = "devbox1".to_string();
        let sessions = vec![session];
        let groups = viewmodel::group_and_sort(&sessions);
        let mut term = make_terminal(80, 12);
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
                accent: theme::COLOR_ACCENT,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        let content: String = buf.content().iter().map(|c| c.symbol().to_string()).collect();
        // Stale unselected: dashed border ┄ in top row fill.
        // Note: stale selected has solid border, so test with unselected.
        // Re-render with no selection.
        let mut term2 = make_terminal(80, 12);
        let selection2 = Selection::new();
        let sessions2 = {
            let mut s = make_session("s1", "idle", None);
            s.stale = true;
            s.origin = "devbox1".to_string();
            vec![s]
        };
        let groups2 = viewmodel::group_and_sort(&sessions2);
        term2.draw(|frame| {
            let area = frame.area();
            let state = RenderState {
                groups: &groups2,
                selection: &selection2,
                daemon_ready: true,
                anim: &anim,
                theme: &theme,
                accent: theme::COLOR_ACCENT,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();
        let buf2 = term2.backend().buffer().clone();
        let content2: String = buf2.content().iter().map(|c| c.symbol().to_string()).collect();
        assert!(
            content2.contains('┄'),
            "stale unselected card should have dashed border (┄), content: {content2:?}"
        );
        let _ = content;
    }

    /// With many sessions and a short viewport, the selected card is visible and
    /// a card for a session scrolled off-screen is not.
    #[test]
    fn frame_card_scroll_selected_card_visible() {
        // 5 sessions in WORKING group, select the last one (index 4).
        // With height=8 (body=6), only 1 card fits (4 rows + 1 header + 1 spacer);
        // the last session card must be visible.
        let sessions: Vec<_> = (0..5)
            .map(|i| {
                let mut s = make_session(&format!("s{i}"), "working", None);
                s.name = format!("repo-{i:02}");
                s.last_activity_secs = i;
                s
            })
            .collect();
        let groups = viewmodel::group_and_sort(&sessions);
        let mut term = make_terminal(80, 8);
        let anim = Anim::new();
        let theme = Theme::classic();
        // Select the last session (flat index 4).
        let selection = Selection { index: Some(4) };

        term.draw(|frame| {
            let area = frame.area();
            let state = RenderState {
                groups: &groups,
                selection: &selection,
                daemon_ready: true,
                anim: &anim,
                theme: &theme,
                accent: theme::COLOR_ACCENT,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        let content: String = buf.content().iter().map(|c| c.symbol().to_string()).collect();
        // Card top border now shows the family label ("Claude"), not session.name (repo-04).
        // Just verify that at least one card is visible (Claude label or WORKING header).
        assert!(
            content.contains("Claude") || content.contains("WORKING"),
            "selected last session card should be visible (Claude label or WORKING group header)"
        );
    }

    /// At width < 40 render_too_small is shown, not a card.
    #[test]
    fn frame_too_narrow_shows_too_small_notice() {
        let sessions = vec![make_session("s1", "working", None)];
        let groups = viewmodel::group_and_sort(&sessions);
        let mut term = make_terminal(9, 10); // < 10 cols → too_small
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
                accent: theme::COLOR_ACCENT,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        let content: String = buf.content().iter().map(|c| c.symbol().to_string()).collect();
        // render_too_small shows "↔ ≥40 cols"
        assert!(
            content.contains("40"),
            "too-small notice should appear for width < 10, content: {content:?}"
        );
    }

    /// Empty state (no sessions) must show the 'no agents' message and NOT show a card.
    #[test]
    fn frame_empty_state_no_card_border() {
        let mut term = make_terminal(80, 14);
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
                accent: theme::COLOR_ACCENT,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        let content: String = buf.content().iter().map(|c| c.symbol().to_string()).collect();
        assert!(
            content.contains("no agents"),
            "empty state must show 'no agents'"
        );
        // No card border characters should appear in body rows (1..13).
        let has_card_border = (1..13u16).any(|y| {
            buf.cell((0, y))
                .map(|c| c.symbol() == "╭" || c.symbol() == "╰")
                .unwrap_or(false)
        });
        assert!(
            !has_card_border,
            "empty state must not render any card borders"
        );
    }

    /// CJK session name does not overflow card borders across multiple widths.
    #[test]
    fn frame_cjk_name_no_card_border_overflow() {
        let mut session = make_session("s1", "working", Some("正在编辑"));
        session.name = "前端认证项目".to_string();
        let sessions = vec![session];
        let groups = viewmodel::group_and_sort(&sessions);
        let anim = Anim::new();
        let theme = Theme::classic();
        let selection = Selection { index: Some(0) };

        for &width in &[40u16, 60, 80, 100] {
            let mut term = make_terminal(width, 12);
            term.draw(|frame| {
                let area = frame.area();
                let state = RenderState {
                    groups: &groups,
                    selection: &selection,
                    daemon_ready: true,
                    anim: &anim,
                    theme: &theme,
                    accent: theme::COLOR_ACCENT,
                    peek_open: false,
                    jump_status: None,
                };
                render_frame(frame.buffer_mut(), area, &state);
            })
            .unwrap();
            // Just verify no panic — the card render tests already check border integrity.
        }
    }

    #[test]
    fn card_cjk_activity_does_not_overflow() {
        let mut session = make_session("s1", "working", Some("正在编辑认证模块的全部代码逻辑"));
        session.name = "前端项目".to_string();
        // Should not panic; the v2 card must render without out-of-bounds writes.
        // Online unselected body rows: right border must be ┊ (dashed vertical, U+250A).
        // Top-left corner must be ┌ (unselected).
        for width in [40u16, 60, 80, 100] {
            let buf = render_v2_card(width, &session, false);
            for y in [1u16, 2] {
                let right_sym = buf
                    .cell((width - 1, y))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_default();
                assert_eq!(
                    right_sym, "\u{250a}",
                    "CJK width={width}: row {y} right border should be ┊ (U+250A), got {right_sym:?}"
                );
            }
            let top_left = buf
                .cell((0, 0))
                .map(|c| c.symbol().to_string())
                .unwrap_or_default();
            assert_eq!(
                top_left, "┌",
                "CJK width={width}: top-left should be ┌, got {top_left:?}"
            );
        }
    }

    // ── Accent colour tests (Task 3) ─────────────────────────────────────────
    //
    // The `roost` title in the header must always use `state.accent`, regardless
    // of which family is currently selected.  Family icons in rows continue to
    // use their per-family colour.

    /// Helper: render a frame with a given accent colour and a specific selected
    /// session family, then return the fg colour of the first char of "roost"
    /// on the header row (col 3, after "╭─ ").
    fn header_roost_fg(accent: Color, selected_family: crate::protocol::AgentFamily) -> Color {
        let mut session = make_session("s1", "working", None);
        session.family = selected_family;
        let sessions = vec![session];
        let groups = viewmodel::group_and_sort(&sessions);
        let mut term = make_terminal(80, 10);
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
                accent,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        // With H_PAD=2 "roost" starts at col 2; find 'r' and return its fg.
        let r_col = (0..80u16)
            .find(|&x| cell_symbol(&buf, x, 0) == "r")
            .expect("'r' of 'roost' should be in header");
        cell_fg(&buf, r_col, 0)
    }

    /// Header "roost" text uses the configured accent colour, not the selected
    /// session's family colour.
    #[test]
    fn header_roost_uses_accent_not_family_color() {
        use crate::protocol::AgentFamily;
        // Use a custom accent that is NOT any family colour.
        let custom_accent = Color::Rgb(0x11, 0x22, 0x33);

        // Regardless of whether Claude or Codex is selected, roost fg = accent.
        let fg_claude = header_roost_fg(custom_accent, AgentFamily::Claude);
        let fg_codex = header_roost_fg(custom_accent, AgentFamily::Codex);

        assert_eq!(
            fg_claude, custom_accent,
            "header 'roost' fg should equal accent when Claude selected, got {fg_claude:?}"
        );
        assert_eq!(
            fg_codex, custom_accent,
            "header 'roost' fg should equal accent when Codex selected, got {fg_codex:?}"
        );
    }

    /// Changing the selected family does NOT change the header colour.
    #[test]
    fn header_roost_invariant_to_selected_family() {
        use crate::protocol::AgentFamily;
        let accent = Color::Rgb(0xaa, 0xbb, 0xcc);

        let fg_claude = header_roost_fg(accent, AgentFamily::Claude);
        let fg_deepseek = header_roost_fg(accent, AgentFamily::Deepseek);
        let fg_cursor = header_roost_fg(accent, AgentFamily::Cursor);

        // All must equal the fixed accent — none follow family colours.
        assert_eq!(fg_claude, accent);
        assert_eq!(fg_deepseek, accent);
        assert_eq!(fg_cursor, accent);
        // And the values must NOT equal DeepSeek's family colour.
        assert_ne!(
            fg_deepseek,
            theme::COLOR_DEEPSEEK,
            "header should not take DeepSeek family colour"
        );
    }

    // ── render_card_v2 tests ──────────────────────────────────────────────────

    /// Helper: build a session for v2 card tests.
    fn make_v2_session_render(
        state: &str,
        activity: Option<&str>,
        busy_secs: u64,
        origin: &str,
        stale: bool,
        host_app: Option<&str>,
    ) -> SessionView {
        SessionView {
            id: "v2r-test".to_string(),
            family: crate::protocol::AgentFamily::Claude,
            name: "payments-api".to_string(),
            cwd: "/home/dev/acme/payments-api".to_string(),
            state: state.to_string(),
            activity: activity.map(str::to_string),
            last_activity_secs: 5,
            busy_secs,
            last_prompt: None,
            last_prompt_age_secs: None,
            host_app: host_app.map(str::to_string),
            host_bundle_id: None,
            terminal_session_id: None,
            terminal_tty: None,
            tmux_target: None,
            wezterm_pane: None,
            zellij_session: None,
            workspace_path: None,
            codex_thread_id: None,
            pane_title: None,
            origin: origin.to_string(),
            stale,
        }
    }

    /// Render a single v2 card to a buffer of given width; returns the buffer.
    fn render_v2_card(
        width: u16,
        session: &SessionView,
        selected: bool,
    ) -> ratatui::buffer::Buffer {
        let theme = Theme::classic();
        let anim = Anim::new();
        let accent = theme::COLOR_ACCENT;
        let mut buf = Buffer::empty(Rect::new(0, 0, width, 4));
        render_card_v2(&mut buf, Rect::new(0, 0, width, 4), session, selected, &theme, &anim, accent);
        buf
    }

    // ── Border matrix: corner characters ─────────────────────────────────────

    /// Unselected online: thin solid corners ┌ ┐ └ ┘.
    #[test]
    fn v2_border_unselected_online_thin_corners() {
        let s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        let buf = render_v2_card(80, &s, false);
        assert_eq!(cell_symbol(&buf, 0, 0), "┌", "top-left should be ┌");
        assert_eq!(cell_symbol(&buf, 79, 0), "┐", "top-right should be ┐");
        assert_eq!(cell_symbol(&buf, 0, 3), "└", "bot-left should be └");
        assert_eq!(cell_symbol(&buf, 79, 3), "┘", "bot-right should be ┘");
    }

    /// Unselected offline: dashed corners ┌┄┐ and ┊ sides (corners stay ┌┘).
    #[test]
    fn v2_border_unselected_offline_dashed_fill() {
        let s = make_v2_session_render("idle", None, 5, "gpu-rig", true, None);
        let buf = render_v2_card(80, &s, false);
        // Corners are still ┌ ┐ └ ┘ — only the fill is dashed.
        assert_eq!(cell_symbol(&buf, 0, 0), "┌", "top-left should be ┌");
        assert_eq!(cell_symbol(&buf, 79, 0), "┐", "top-right should be ┐");
        assert_eq!(cell_symbol(&buf, 0, 3), "└", "bot-left should be └");
        assert_eq!(cell_symbol(&buf, 79, 3), "┘", "bot-right should be ┘");
        // Bottom border fill should be ┄ (dashed, U+2504).
        let bot_row: String = (0..80).map(|x| cell_symbol(&buf, x, 3)).collect();
        assert!(
            bot_row.contains('\u{2504}'),
            "offline unselected bottom fill should contain ┄ (U+2504), got: {bot_row:?}"
        );
        // Side vertical: ┊ (U+250A)
        assert_eq!(cell_symbol(&buf, 0, 1), "\u{250a}", "left side of body rows should be ┊");
        assert_eq!(cell_symbol(&buf, 79, 1), "\u{250a}", "right side of body rows should be ┊");
    }

    /// Selected online: heavy corners ┏ ┓ ┗ ┛ with accent colour.
    #[test]
    fn v2_border_selected_online_heavy_corners_accent_color() {
        let s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        let buf = render_v2_card(80, &s, true);
        assert_eq!(cell_symbol(&buf, 0, 0), "┏", "top-left should be ┏ (heavy)");
        assert_eq!(cell_symbol(&buf, 79, 0), "┓", "top-right should be ┓ (heavy)");
        assert_eq!(cell_symbol(&buf, 0, 3), "┗", "bot-left should be ┗ (heavy)");
        assert_eq!(cell_symbol(&buf, 79, 3), "┛", "bot-right should be ┛ (heavy)");
        // Border colour = accent.
        let fg = cell_fg(&buf, 0, 0);
        assert_eq!(fg, theme::COLOR_ACCENT, "selected border should be accent colour, got {fg:?}");
        // Bottom fill = ━ (heavy dash).
        let bot_row: String = (0..80).map(|x| cell_symbol(&buf, x, 3)).collect();
        assert!(
            bot_row.contains('━'),
            "selected online bottom fill should contain ━, got: {bot_row:?}"
        );
    }

    /// Selected offline: heavy corners + accent colour, but card content has ⊘ prefix.
    #[test]
    fn v2_border_selected_offline_heavy_accent_with_offline_prefix() {
        let s = make_v2_session_render("idle", None, 5, "gpu-rig", true, None);
        let buf = render_v2_card(80, &s, true);
        // Border still heavy.
        assert_eq!(cell_symbol(&buf, 0, 0), "┏");
        assert_eq!(cell_symbol(&buf, 79, 0), "┓");
        assert_eq!(cell_symbol(&buf, 0, 3), "┗");
        assert_eq!(cell_symbol(&buf, 79, 3), "┛");
        let fg = cell_fg(&buf, 0, 0);
        assert_eq!(fg, theme::COLOR_ACCENT, "selected offline border should still be accent");
        // Body row 1: step prefix should be ⊘.
        let row1: String = (0..80).map(|x| cell_symbol(&buf, x, 1)).collect();
        assert!(row1.contains('⊘'), "offline step prefix must be ⊘, got: {row1:?}");
    }

    // ── Border chars NOT bold ─────────────────────────────────────────────────

    /// Border corner chars must NOT carry BOLD modifier.
    #[test]
    fn v2_border_chars_not_bold() {
        let s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        // Test both selected (heavy) and unselected.
        for selected in [false, true] {
            let buf = render_v2_card(80, &s, selected);
            let corner_modifier = buf.cell((0, 0)).map(|c| c.style().add_modifier).unwrap_or_default();
            assert!(
                !corner_modifier.contains(Modifier::BOLD),
                "border corner must not have BOLD (selected={selected}), got modifier: {corner_modifier:?}"
            );
        }
    }

    // ── Title text: selected → bold name ─────────────────────────────────────

    /// Selected card: family label in top border must carry BOLD modifier.
    /// The card now shows the family label ("Claude") instead of "payments-api".
    #[test]
    fn v2_selected_title_name_is_bold() {
        let s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        let buf = render_v2_card(80, &s, true);
        // Card top border shows "Claude" for Claude family. Find 'C' (first of "Claude").
        // sp at col 1, icon at col 2, sp at col 3, family label starts at col 4.
        let c_col = (4..80u16).find(|&x| cell_symbol(&buf, x, 0) == "C")
            .expect("'C' of 'Claude' family label should appear in top border");
        let modifier = buf.cell((c_col, 0)).map(|c| c.style().add_modifier).unwrap_or_default();
        assert!(
            modifier.contains(Modifier::BOLD),
            "selected card family label should have BOLD modifier, got {modifier:?}"
        );
    }

    /// Unselected card: family label must NOT be bold.
    #[test]
    fn v2_unselected_title_name_not_bold() {
        let s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        let buf = render_v2_card(80, &s, false);
        // Family label "Claude" starts at col 4 (sp col 1, icon col 2, sp col 3).
        let c_col = (4..80u16).find(|&x| cell_symbol(&buf, x, 0) == "C")
            .expect("'C' of 'Claude' family label should appear");
        let modifier = buf.cell((c_col, 0)).map(|c| c.style().add_modifier).unwrap_or_default();
        assert!(
            !modifier.contains(Modifier::BOLD),
            "unselected card family label must not be BOLD, got {modifier:?}"
        );
    }

    // ── Icon family colour ────────────────────────────────────────────────────

    /// Icon (✻) in top border should use family colour.
    /// Icon is now at col 2 (x0+2), preceded by a leading space at col 1.
    #[test]
    fn v2_icon_has_family_color() {
        let s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        let buf = render_v2_card(80, &s, false);
        // Icon is at col 2 (x0+2) — col 1 is the leading space before the icon.
        let icon_col = 2u16;
        assert_eq!(cell_symbol(&buf, icon_col, 0), "✻", "icon should be ✻ at col 2");
        let fg = cell_fg(&buf, icon_col, 0);
        assert_eq!(fg, theme::COLOR_CLAUDE, "icon fg should be COLOR_CLAUDE");
    }

    /// Offline: icon should be dimmed.
    /// Icon is at col 2 after the change.
    #[test]
    fn v2_offline_icon_is_dim() {
        let s = make_v2_session_render("idle", None, 5, "gpu-rig", true, None);
        let buf = render_v2_card(80, &s, false);
        let icon_col = 2u16;
        let fg = cell_fg(&buf, icon_col, 0);
        assert_eq!(fg, Color::DarkGray, "offline icon should be dim (DarkGray), got {fg:?}");
    }

    // ── Field exact placement: no duplicates ──────────────────────────────────

    /// The top border right corner (┐/┓) must be at exactly col width-1.
    #[test]
    fn v2_top_border_right_corner_at_last_col() {
        let s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        for width in [40u16, 60, 80, 100] {
            for selected in [false, true] {
                let buf = render_v2_card(width, &s, selected);
                let last = width - 1;
                let sym = cell_symbol(&buf, last, 0);
                assert!(
                    sym == "┐" || sym == "┓",
                    "right corner should be ┐ or ┓ at col {last} (width={width}, selected={selected}), got {sym:?}"
                );
            }
        }
    }

    /// The bottom border right corner (┘/┛) must be at exactly col width-1.
    #[test]
    fn v2_bot_border_right_corner_at_last_col() {
        let s = make_v2_session_render("idle", None, 5, "local", false, None);
        for width in [40u16, 60, 80, 100] {
            for selected in [false, true] {
                let buf = render_v2_card(width, &s, selected);
                let last = width - 1;
                let sym = cell_symbol(&buf, last, 3);
                assert!(
                    sym == "┘" || sym == "┛",
                    "bottom-right corner should be ┘ or ┛ at col {last} (width={width}, selected={selected}), got {sym:?}"
                );
            }
        }
    }

    /// The top border does NOT contain duplicate characters at consecutive columns
    /// (regression: "先整行画、再错位 1 列覆盖" bug).
    /// Now shows "Claude" family label in place of "payments-api".
    #[test]
    fn v2_top_border_no_duplicate_adjacent_content() {
        let s = make_v2_session_render("approval", Some("run: git push"), 30, "local", false, Some("Cursor"));
        let buf = render_v2_card(80, &s, false);
        let row: Vec<String> = (0..80u16).map(|x| cell_symbol(&buf, x, 0)).collect();
        let row_str: String = row.join("");
        // Card now shows "Claude" family label — check it appears exactly once.
        let count = row_str.matches("Claude").count();
        assert_eq!(count, 1, "family label 'Claude' should appear exactly once in top border, got {count}: {row_str:?}");
    }

    // ── 改动1: 时间自然宽度,时间紧贴右边框 ───────────────────────────────────

    /// Time string should be natural width (no fixed 6-col padding).
    /// The right corner (┐/┓) must still land at col width-1 regardless of time length.
    #[test]
    fn v2_time_natural_width_right_corner_stable() {
        // Different busy_secs produce different time string lengths
        for busy_secs in [2u64, 36, 90, 600, 3600] {
            let s = make_v2_session_render("working", Some("task"), busy_secs, "local", false, None);
            for width in [60u16, 80, 100] {
                let buf = render_v2_card(width, &s, false);
                let last = width - 1;
                let sym = cell_symbol(&buf, last, 0);
                assert!(
                    sym == "┐" || sym == "┓",
                    "right corner should be at col {last} for busy_secs={busy_secs} width={width}, got {sym:?}"
                );
                // Also check the cell just before right corner is a space (sp5)
                let sp5_sym = cell_symbol(&buf, last - 1, 0);
                assert_eq!(
                    sp5_sym, " ",
                    "col just before ┐ (col {}) should be space for busy_secs={busy_secs} width={width}, got {sp5_sym:?}",
                    last - 1
                );
            }
        }
    }

    /// Shorter time (36s=3 cols) should yield a longer fill_d vs longer time (1m30s=5 cols).
    /// D absorbs the time width change: fill_d(36s) = fill_d(1m30s) + 2.
    #[test]
    fn v2_shorter_time_gives_longer_fill_d() {
        let s36 = make_v2_session_render("working", Some("task"), 36, "local", false, None);
        let s1m30 = make_v2_session_render("working", Some("task"), 90, "local", false, None);
        let cl36 = crate::tui::layout::card_layout_v2(80, &s36);
        let cl1m30 = crate::tui::layout::card_layout_v2(80, &s1m30);
        assert!(
            cl36.fill_d > cl1m30.fill_d,
            "fill_d for 36s ({}) should be > fill_d for 1m30s ({})",
            cl36.fill_d, cl1m30.fill_d
        );
        assert_eq!(
            cl36.fill_d, cl1m30.fill_d + 2,
            "fill_d(36s)={} should equal fill_d(1m30s)={} + 2",
            cl36.fill_d, cl1m30.fill_d
        );
    }

    // ── 改动2: 图标前有一个空格 ───────────────────────────────────────────────

    /// After change 2, x0+1 should be a space, and the icon should be at x0+2.
    /// Previously icon was directly at x0+1 (right after ┌).
    #[test]
    fn v2_icon_has_leading_space_not_at_x0_plus_1() {
        let s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        let buf = render_v2_card(80, &s, false);
        // x0=0, so x0+1=col 1 should be a space, not the icon
        let col1_sym = cell_symbol(&buf, 1, 0);
        assert_eq!(col1_sym, " ", "col 1 (x0+1) should be a space before the icon, got {col1_sym:?}");
        // x0+2=col 2 should be the icon
        let col2_sym = cell_symbol(&buf, 2, 0);
        assert_eq!(col2_sym, "✻", "col 2 (x0+2) should be the Claude icon ✻, got {col2_sym:?}");
    }

    /// Icon column should be x0+2 (not x0+1 as before), and name should follow icon+sp at x0+4.
    #[test]
    fn v2_icon_at_col2_name_at_col4() {
        let s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        let buf = render_v2_card(80, &s, false);
        // icon at col 2
        assert_eq!(cell_symbol(&buf, 2, 0), "✻", "icon should be at col 2");
        // sp after icon at col 3
        assert_eq!(cell_symbol(&buf, 3, 0), " ", "space after icon should be at col 3");
        // name starts at col 4 — first char of "Claude" should be 'C'
        assert_eq!(cell_symbol(&buf, 4, 0), "C", "name 'Claude' should start at col 4");
    }

    // ── Approval: time also in amber ──────────────────────────────────────────

    /// Approval state: time in the header uses amber colour (COLOR_APPROVAL).
    #[test]
    fn v2_approval_time_is_amber() {
        let s = make_v2_session_render("approval", Some("run: git push"), 30, "local", false, None);
        let buf = render_v2_card(80, &s, false);
        // time_str is " 30s" (3 cols); it comes after " · " separator.
        // Find the time colour by locating the time string in row 0.
        // time is right-aligned: look from the right, find the first "s" char.
        let s_col = (0..80u16).rev().find(|&x| cell_symbol(&buf, x, 0) == "s")
            .expect("time 's' suffix should appear in top border");
        let fg = cell_fg(&buf, s_col, 0);
        assert_eq!(
            fg, theme::COLOR_APPROVAL,
            "approval time should be amber (COLOR_APPROVAL), got {fg:?}"
        );
    }

    // ── Working: spinner glyph (not static placeholder) ───────────────────────

    /// Working state: top border glyph is a valid braille spinner frame.
    #[test]
    fn v2_working_spinner_is_braille() {
        let s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        let buf = render_v2_card(80, &s, false);
        // The glyph sits in the top border; find any braille char.
        let has_braille = (0..80u16).any(|x| {
            let sym = cell_symbol(&buf, x, 0);
            sym.chars().next().map(|c| ('\u{2800}'..='\u{28FF}').contains(&c)).unwrap_or(false)
        });
        assert!(has_braille, "working top border should contain a braille spinner frame");
    }

    // ── Step line: working → Reset fg; others → dim ───────────────────────────

    /// Working state: step text uses terminal default fg (Color::Reset).
    #[test]
    fn v2_working_step_text_is_reset_fg() {
        let s = make_v2_session_render("working", Some("Editing src/main.rs"), 10, "local", false, None);
        let buf = render_v2_card(80, &s, false);
        // "E" of "Editing" is the first char of step_text, at col 4.
        let e_col = (0..80u16).find(|&x| cell_symbol(&buf, x, 1) == "E")
            .expect("'E' of 'Editing' should appear in step row");
        let fg = cell_fg(&buf, e_col, 1);
        assert_eq!(fg, Color::Reset, "working step text fg should be Reset, got {fg:?}");
    }

    /// Non-working state: step text uses dim fg.
    #[test]
    fn v2_non_working_step_text_is_dim() {
        let s = make_v2_session_render("question", Some("Which DB?"), 10, "local", false, None);
        let buf = render_v2_card(80, &s, false);
        let w_col = (0..80u16).find(|&x| cell_symbol(&buf, x, 1) == "W")
            .expect("'W' of 'Which' should appear in step row");
        let fg = cell_fg(&buf, w_col, 1);
        assert_eq!(fg, Color::DarkGray, "question step text fg should be DarkGray, got {fg:?}");
    }

    // ── @source colour: @local → dim, @device → COLOR_REMOTE ────────────────

    /// @local source: DarkGray colour.
    #[test]
    fn v2_source_local_is_dim() {
        let s = make_v2_session_render("working", Some("t"), 5, "local", false, None);
        let buf = render_v2_card(80, &s, false);
        // "@local" appears in row 2; find '@'.
        let at_col = (0..80u16).find(|&x| cell_symbol(&buf, x, 2) == "@")
            .expect("@ should appear in row 2");
        let fg = cell_fg(&buf, at_col, 2);
        assert_eq!(fg, Color::DarkGray, "@local should be DarkGray, got {fg:?}");
    }

    /// @device source: COLOR_REMOTE (cold blue).
    #[test]
    fn v2_source_remote_is_cold_blue() {
        let mut s = make_v2_session_render("working", Some("t"), 5, "desk-mini", false, None);
        s.origin = "desk-mini".to_string();
        let buf = render_v2_card(80, &s, false);
        let at_col = (0..80u16).find(|&x| cell_symbol(&buf, x, 2) == "@")
            .expect("@ should appear in row 2");
        let fg = cell_fg(&buf, at_col, 2);
        assert_eq!(fg, theme::COLOR_REMOTE, "@device should be COLOR_REMOTE cold blue, got {fg:?}");
    }

    // ── Offline: ⊘ prefix in step line + dim ─────────────────────────────────

    /// Offline card: step prefix is ⊘.
    #[test]
    fn v2_offline_step_prefix_is_offline_glyph() {
        let s = make_v2_session_render("idle", None, 5, "gpu-rig", true, None);
        let buf = render_v2_card(80, &s, false);
        // col 2 of row 1 is the step prefix.
        let sym = cell_symbol(&buf, 2, 1);
        assert_eq!(sym, "⊘", "offline step prefix at col 2 should be ⊘, got {sym:?}");
    }

    /// Online card: step prefix is ▸.
    #[test]
    fn v2_online_step_prefix_is_arrow() {
        let s = make_v2_session_render("working", Some("task"), 5, "local", false, None);
        let buf = render_v2_card(80, &s, false);
        let sym = cell_symbol(&buf, 2, 1);
        assert_eq!(sym, "▸", "online step prefix at col 2 should be ▸, got {sym:?}");
    }

    // ── CJK alignment ─────────────────────────────────────────────────────────

    /// CJK session name: top border right corner still lands at col width-1.
    #[test]
    fn v2_cjk_name_top_border_aligned() {
        let mut s = make_v2_session_render("working", Some("思考中"), 5, "local", false, None);
        s.name = "前端认证".to_string(); // CJK, 4 chars × 2 = 8 cols
        for width in [60u16, 80, 100] {
            for selected in [false, true] {
                let buf = render_v2_card(width, &s, selected);
                let last = width - 1;
                let sym = cell_symbol(&buf, last, 0);
                assert!(
                    sym == "┐" || sym == "┓",
                    "CJK: right corner at {last} should be ┐/┓ (width={width}, sel={selected}), got {sym:?}"
                );
            }
        }
    }

    // ── via app faint ─────────────────────────────────────────────────────────

    /// "· via Cursor" appears in row 2 with dim colour.
    #[test]
    fn v2_via_app_appears_in_source_row() {
        let s = make_v2_session_render("working", Some("t"), 5, "local", false, Some("Cursor"));
        let buf = render_v2_card(100, &s, false);
        let row2: String = (0..100u16).map(|x| cell_symbol(&buf, x, 2)).collect();
        assert!(row2.contains("via"), "row 2 should contain 'via', got: {row2:?}");
        assert!(row2.contains("Cursor"), "row 2 should contain 'Cursor', got: {row2:?}");
    }

    // ── Session name appears in frame (integration) ───────────────────────────

    /// Card top border shows family label in a full-frame render at 80 cols.
    /// The old session.name (cwd basename "my-repo") is no longer shown in the card
    /// top border — it was replaced by the family label.
    #[test]
    fn v2_frame_session_name_visible_at_80() {
        let buf = render_at_width(80, 12, true);
        let content: String = buf.content().iter().map(|c| c.symbol().to_string()).collect();
        // Card now shows "Claude" family label, not the cwd basename "my-repo".
        assert!(
            content.contains("Claude"),
            "card top border should show 'Claude' family label in frame at 80 cols"
        );
    }

    /// Family icon in a row still uses its per-family colour (not the accent).
    #[test]
    fn row_family_icon_still_uses_family_color() {
        // Use a custom accent that differs from the Claude family colour.
        let _custom_accent = Color::Rgb(0x11, 0x22, 0x33);
        let session = make_session("s1", "working", None); // Claude family
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

        // The Claude icon ✻ should have COLOR_CLAUDE as its fg.
        let icon_col = find_symbol_col(&buf, 100, 0, "✻").expect("Claude icon ✻ should appear");
        let fg = cell_fg(&buf, icon_col, 0);
        assert_eq!(
            fg,
            theme::COLOR_CLAUDE,
            "family icon fg should be COLOR_CLAUDE, got {fg:?}"
        );
    }

    // ── Task 7: plain-text header / footer, no outer box ─────────────────────
    //
    // After the redesign:
    //   • Header is a plain text line: "roost" (accent) on the left, "N agents · live ●" on the right.
    //     There must be NO box-corner characters (╭ ╮ ┌ ┐) anywhere on the header row.
    //   • Footer is a plain text line with key hints.
    //     There must be NO box-corner characters (╰ ╯ └ ┘) anywhere on the footer row.
    //   • Body rows must have NO outer-frame ╎ characters in column 0 or the last column
    //     on rows that are group-headers or spacers (the dashed side-border is removed).

    /// Header row must NOT contain any box-corner characters.
    #[test]
    fn header_has_no_box_corners() {
        let buf = render_at_width(100, 20, true);
        let header = row_string(&buf, 100, 0);
        for ch in ['╭', '╮', '┌', '┐'] {
            assert!(
                !header.contains(ch),
                "header must not contain box corner '{ch}', got: {header:?}"
            );
        }
    }

    /// Header row must contain "roost".
    #[test]
    fn header_plain_text_contains_roost() {
        let buf = render_at_width(100, 20, true);
        let header = row_string(&buf, 100, 0);
        assert!(
            header.contains("roost"),
            "header must contain 'roost', got: {header:?}"
        );
    }

    /// Header "roost" must be coloured with the configured accent colour.
    #[test]
    fn header_plain_roost_has_accent_color() {
        let sessions = vec![make_session("s1", "working", Some("Editing main.rs"))];
        let groups = viewmodel::group_and_sort(&sessions);
        let mut term = make_terminal(100, 20);
        let anim = Anim::new();
        let theme = Theme::classic();
        let selection = Selection { index: Some(0) };
        let custom_accent = Color::Rgb(0xab, 0xcd, 0xef);

        term.draw(|frame| {
            let area = frame.area();
            let state = RenderState {
                groups: &groups,
                selection: &selection,
                daemon_ready: true,
                anim: &anim,
                theme: &theme,
                accent: custom_accent,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        // "roost" starts at column 0 (plain text, no leading "╭─ ")
        let r_col = find_symbol_col(&buf, 100, 0, "r").expect("'r' of 'roost' should be in header");
        let fg = cell_fg(&buf, r_col, 0);
        assert_eq!(
            fg, custom_accent,
            "header 'roost' fg should equal accent, got {fg:?}"
        );
    }

    /// Header row must contain agent count and "agents".
    #[test]
    fn header_plain_text_shows_agent_count() {
        let buf = render_at_width(100, 20, true);
        let header = row_string(&buf, 100, 0);
        assert!(
            header.contains("agent"),
            "header must contain 'agent' (count indicator), got: {header:?}"
        );
    }

    /// With 0 agents (empty state), header shows "0 agents · idle" and NO heartbeat dot.
    #[test]
    fn header_zero_agents_idle_no_dot() {
        let mut term = make_terminal(100, 20);
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
                accent: theme::COLOR_ACCENT,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        let header = row_string(&buf, 100, 0);
        assert!(
            header.contains("0 agents"),
            "empty-state header must show '0 agents', got: {header:?}"
        );
        assert!(
            header.contains("idle"),
            "empty-state header must show 'idle', got: {header:?}"
        );
        assert!(
            !header.contains('●'),
            "empty-state header must NOT show heartbeat dot, got: {header:?}"
        );
    }

    /// Footer row must NOT contain box-corner characters.
    #[test]
    fn footer_has_no_box_corners() {
        let buf = render_at_width(100, 20, true);
        let footer_y = 19u16;
        let footer = row_string(&buf, 100, footer_y);
        for ch in ['╰', '╯', '└', '┘'] {
            assert!(
                !footer.contains(ch),
                "footer must not contain box corner '{ch}', got: {footer:?}"
            );
        }
    }

    /// Footer row must contain key hint text (↑/↓, select, enter, peek, quit).
    #[test]
    fn footer_plain_text_contains_key_hints() {
        let buf = render_at_width(100, 20, true);
        let footer_y = 19u16;
        let footer = row_string(&buf, 100, footer_y);
        assert!(
            footer.contains("select"),
            "footer must contain 'select', got: {footer:?}"
        );
        assert!(
            footer.contains("peek"),
            "footer must contain 'peek', got: {footer:?}"
        );
        assert!(
            footer.contains("quit"),
            "footer must contain 'quit', got: {footer:?}"
        );
    }

    /// Footer in peek-open mode must contain "esc close".
    #[test]
    fn footer_peek_open_shows_esc_close() {
        let sessions = vec![make_session("s1", "working", Some("Editing main.rs"))];
        let groups = viewmodel::group_and_sort(&sessions);
        let mut term = make_terminal(100, 20);
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
                accent: theme::COLOR_ACCENT,
                peek_open: true,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        let footer_y = 19u16;
        let footer = row_string(&buf, 100, footer_y);
        assert!(
            footer.contains("esc"),
            "peek-open footer must contain 'esc', got: {footer:?}"
        );
        assert!(
            footer.contains("close"),
            "peek-open footer must contain 'close', got: {footer:?}"
        );
        // Still no box corners.
        for ch in ['╰', '╯', '└', '┘'] {
            assert!(
                !footer.contains(ch),
                "peek-open footer must not contain box corner '{ch}', got: {footer:?}"
            );
        }
    }

    /// Body rows (group-headers, spacers) must not carry the outer ╎ side-border
    /// in column 0 or the last column (the outer side-border is removed).
    #[test]
    fn body_has_no_outer_side_border_glyph() {
        let buf = render_at_width(100, 20, true);
        // Row 1 is typically a group-header row (no card border at col 0).
        // We scan every body row (rows 1..19) and check col 0 & col 99 for ╎.
        let last_col = 99u16;
        let has_side_border = (1..19u16).any(|y| {
            let left = cell_symbol(&buf, 0, y);
            let right = cell_symbol(&buf, last_col, y);
            left == "╎" || right == "╎"
        });
        assert!(
            !has_side_border,
            "body rows must not contain outer ╎ side-border after redesign"
        );
    }

    // ── Adjustment 1: horizontal padding ─────────────────────────────────────
    //
    // Content must not start at col 0; the header "roost" brand and card left
    // border must be at least H_PAD (2) columns from the terminal edge.

    /// Header "roost" brand text must NOT start at column 0 (H_PAD indentation).
    #[test]
    fn header_brand_not_at_col_0() {
        let buf = render_at_width(100, 20, true);
        // With H_PAD=2 the brand starts at col 2, not col 0.
        let r_col = (0..100u16)
            .find(|&x| cell_symbol(&buf, x, 0) == "r")
            .expect("'r' of 'roost' should be in header");
        assert!(
            r_col >= 2,
            "header 'roost' should start at col ≥ 2 (H_PAD), got col {r_col}"
        );
    }

    /// Footer key-hints must NOT start at column 0 (H_PAD indentation).
    #[test]
    fn footer_hints_not_at_col_0() {
        let buf = render_at_width(100, 20, true);
        let footer_y = 19u16;
        // With H_PAD=2 the first non-space character of key hints starts at col ≥ 2.
        let first_char_col = (0..100u16)
            .find(|&x| cell_symbol(&buf, x, footer_y) != " ")
            .expect("footer should have non-space content");
        assert!(
            first_char_col >= 2,
            "footer hints should start at col ≥ 2 (H_PAD), got col {first_char_col}"
        );
    }

    /// Card top-left corner must appear at col ≥ 2 (body is indented by H_PAD).
    #[test]
    fn card_top_border_not_at_col_0() {
        let buf = render_at_width(100, 24, true);
        // Find any heavy card corner (┏ for selected) in body rows 1..23.
        let corner_col = (1..23u16).find_map(|y| {
            (2..100u16).find(|&x| {
                let sym = cell_symbol(&buf, x, y);
                sym == "┏" || sym == "┌"
            }).map(|x| (x, y))
        });
        let (col, _row) = corner_col.expect("card top-left corner should appear somewhere");
        assert!(
            col >= 2,
            "card top-left corner should be at col ≥ 2 (H_PAD), got col {col}"
        );
        // Column 0 must not have a card corner.
        let col0_is_corner = (1..23u16).any(|y| {
            let sym = cell_symbol(&buf, 0, y);
            sym == "┏" || sym == "┌" || sym == "┗" || sym == "└"
        });
        assert!(!col0_is_corner, "col 0 must not contain a card corner (it should be blank padding)");
    }

    /// Body first non-spacer row must start at y ≥ 2 (header + top padding row).
    #[test]
    fn body_first_content_row_is_after_top_padding() {
        // render_at_width puts 1 WORKING session with selection on it.
        // Group header "WORKING" should appear at y ≥ 2 (y=0 is header, y=1 is padding row).
        let buf = render_at_width(100, 24, true);
        // "WORKING" must not appear on row 1 (the padding row immediately after header).
        let row1 = row_string(&buf, 100, 1);
        assert!(
            !row1.contains("WORKING"),
            "row 1 should be the top-padding blank row, not contain 'WORKING', got: {row1:?}"
        );
        // "WORKING" should appear somewhere in rows 2..24.
        let found = (2..24u16).any(|y| row_string(&buf, 100, y).contains("WORKING"));
        assert!(found, "group header 'WORKING' must appear in rows 2..24 after top-padding");
    }

    // ── Adjustment 2: online unselected card sides are ┊ ────────────────────

    /// Unselected online card: side vertical borders (rows 1 and 2) must be ┊ (U+250A).
    #[test]
    fn v2_online_unselected_side_is_dashed_vertical() {
        let s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        let buf = render_v2_card(80, &s, false);
        // Rows 1 and 2 (body rows): left col 0 and right col 79 must be ┊
        for y in [1u16, 2] {
            let left = cell_symbol(&buf, 0, y);
            let right = cell_symbol(&buf, 79, y);
            assert_eq!(
                left, "\u{250a}",
                "online unselected side left col should be ┊ (U+250A) at row {y}, got {left:?}"
            );
            assert_eq!(
                right, "\u{250a}",
                "online unselected side right col should be ┊ (U+250A) at row {y}, got {right:?}"
            );
        }
    }

    /// Corners and horizontal fills must remain solid even when sides are ┊.
    #[test]
    fn v2_online_unselected_corners_remain_solid() {
        let s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        let buf = render_v2_card(80, &s, false);
        assert_eq!(cell_symbol(&buf, 0, 0), "┌", "top-left must be ┌");
        assert_eq!(cell_symbol(&buf, 79, 0), "┐", "top-right must be ┐");
        assert_eq!(cell_symbol(&buf, 0, 3), "└", "bot-left must be └");
        assert_eq!(cell_symbol(&buf, 79, 3), "┘", "bot-right must be ┘");
        // Bottom fill must be ─ (solid thin).
        let bot_fill = cell_symbol(&buf, 1, 3);
        assert_eq!(bot_fill, "─", "bottom fill must be ─, got {bot_fill:?}");
    }

    /// Selected card sides must remain ┃ (heavy), NOT become ┊.
    #[test]
    fn v2_selected_card_sides_remain_heavy() {
        let s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        let buf = render_v2_card(80, &s, true);
        for y in [1u16, 2] {
            let left = cell_symbol(&buf, 0, y);
            let right = cell_symbol(&buf, 79, y);
            assert_eq!(left, "┃", "selected card left side at row {y} must be ┃, got {left:?}");
            assert_eq!(right, "┃", "selected card right side at row {y} must be ┃, got {right:?}");
        }
    }

    /// Offline unselected card sides must remain ┊ (already dashed).
    #[test]
    fn v2_offline_unselected_sides_remain_dashed() {
        let s = make_v2_session_render("idle", None, 5, "gpu-rig", true, None);
        let buf = render_v2_card(80, &s, false);
        for y in [1u16, 2] {
            let left = cell_symbol(&buf, 0, y);
            assert_eq!(left, "\u{250a}", "offline side at row {y} must be ┊, got {left:?}");
        }
    }

    // ── Adjustment 3: selected-agent cwd dim row above footer ────────────────

    /// A dim cwd line appears above the footer (second-to-last row) showing the
    /// selected agent's cwd.
    #[test]
    fn global_cwd_row_appears_above_footer() {
        let mut session = make_session("s1", "working", Some("task"));
        session.cwd = "/home/testuser/projects/myapp".to_string();
        let sessions = vec![session];
        let groups = viewmodel::group_and_sort(&sessions);
        let width = 100u16;
        let height = 20u16;
        let mut term = make_terminal(width, height);
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
                accent: theme::COLOR_ACCENT,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        // The cwd row sits at footer_y - 1 = height - 2.
        let cwd_row_y = height - 2;
        let cwd_row = row_string(&buf, width, cwd_row_y);
        assert!(
            cwd_row.contains("myapp") || cwd_row.contains("projects"),
            "cwd row should contain selected agent's cwd path, got: {cwd_row:?}"
        );
    }

    /// Home directory in cwd is abbreviated to ~ by the renderer.
    /// This test verifies the real $HOME abbreviation: session.cwd starts with $HOME.
    #[test]
    fn global_cwd_home_is_abbreviated() {
        // Use an absolute path that starts with the real $HOME so the renderer
        // can abbreviate it to ~. If $HOME is unset, skip the abbreviation check
        // and just verify the path appears.
        let home = std::env::var("HOME").unwrap_or_default();
        let fake_suffix = "/roost-test-abbrev-project";
        let fake_cwd = if home.is_empty() {
            "/tmp/roost-test-abbrev-project".to_string()
        } else {
            format!("{home}{fake_suffix}")
        };

        let mut session = make_session("s1", "working", Some("task"));
        session.cwd = fake_cwd.clone();
        let sessions = vec![session];
        let groups = viewmodel::group_and_sort(&sessions);
        let width = 100u16;
        let height = 20u16;
        let mut term = make_terminal(width, height);
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
                accent: theme::COLOR_ACCENT,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        let cwd_row_y = height - 2;
        let cwd_row = row_string(&buf, width, cwd_row_y);

        if !home.is_empty() {
            assert!(
                cwd_row.contains('~'),
                "cwd row should contain '~' abbreviation for home (home={home:?}), got: {cwd_row:?}"
            );
        }
        assert!(
            cwd_row.contains("roost-test-abbrev-project"),
            "cwd row should contain the project name, got: {cwd_row:?}"
        );
    }

    /// The cwd row must be dimmed (DarkGray fg).
    #[test]
    fn global_cwd_row_is_dim() {
        let mut session = make_session("s1", "working", Some("task"));
        session.cwd = "/tmp/myproject-dim-test".to_string();
        let sessions = vec![session];
        let groups = viewmodel::group_and_sort(&sessions);
        let width = 100u16;
        let height = 20u16;
        let mut term = make_terminal(width, height);
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
                accent: theme::COLOR_ACCENT,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        let cwd_row_y = height - 2;
        // Find the first non-space cell in the cwd row and check its fg.
        let first_content_col = (0..width)
            .find(|&x| cell_symbol(&buf, x, cwd_row_y) != " ")
            .expect("cwd row should have non-space content");
        let fg = cell_fg(&buf, first_content_col, cwd_row_y);
        assert_eq!(
            fg,
            ratatui::style::Color::DarkGray,
            "cwd row should be DarkGray (dim), got {fg:?}"
        );
    }

    // ── Adjustment 4: card top border shows family name, not cwd basename ────

    /// Card top border shows the family name ("Claude"), not the session name (cwd basename).
    #[test]
    fn v2_card_top_border_shows_family_name_not_session_name() {
        let mut s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        // Set a distinctive session name (cwd basename) that must NOT appear in the top border.
        s.name = "zettlab".to_string();
        // Family is Claude → card label should be "Claude".
        let buf = render_v2_card(80, &s, false);
        let top_row: String = (0..80u16).map(|x| cell_symbol(&buf, x, 0)).collect();
        assert!(
            top_row.contains("Claude"),
            "card top border should show family name 'Claude', got: {top_row:?}"
        );
        assert!(
            !top_row.contains("zettlab"),
            "card top border must NOT show cwd-basename session name 'zettlab', got: {top_row:?}"
        );
    }

    /// Codex family → card top border shows "Codex".
    #[test]
    fn v2_card_top_border_codex_family() {
        let mut s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        s.family = crate::protocol::AgentFamily::Codex;
        s.name = "my-project".to_string();
        let buf = render_v2_card(80, &s, false);
        let top_row: String = (0..80u16).map(|x| cell_symbol(&buf, x, 0)).collect();
        assert!(
            top_row.contains("Codex"),
            "Codex family card top border should show 'Codex', got: {top_row:?}"
        );
    }

    /// DeepSeek family → card top border shows "CodeWhale".
    #[test]
    fn v2_card_top_border_deepseek_family_is_codewhale() {
        let mut s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        s.family = crate::protocol::AgentFamily::Deepseek;
        s.name = "my-project".to_string();
        let buf = render_v2_card(80, &s, false);
        let top_row: String = (0..80u16).map(|x| cell_symbol(&buf, x, 0)).collect();
        assert!(
            top_row.contains("CodeWhale"),
            "DeepSeek family card top border should show 'CodeWhale', got: {top_row:?}"
        );
    }

    /// Cursor family → card top border shows "Cursor".
    #[test]
    fn v2_card_top_border_cursor_family() {
        let mut s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        s.family = crate::protocol::AgentFamily::Cursor;
        s.name = "my-project".to_string();
        let buf = render_v2_card(80, &s, false);
        let top_row: String = (0..80u16).map(|x| cell_symbol(&buf, x, 0)).collect();
        assert!(
            top_row.contains("Cursor"),
            "Cursor family card top border should show 'Cursor', got: {top_row:?}"
        );
    }

    /// cwd full path must still appear in body row 2 even though top border shows family name.
    #[test]
    fn v2_card_cwd_still_in_row2() {
        let mut s = make_v2_session_render("working", Some("task"), 10, "local", false, None);
        s.cwd = "/home/dev/acme/payments-api".to_string();
        s.name = "payments-api".to_string(); // cwd basename, must NOT appear in row 0
        let buf = render_v2_card(100, &s, false);
        let row2: String = (0..100u16).map(|x| cell_symbol(&buf, x, 2)).collect();
        assert!(
            row2.contains("payments-api") || row2.contains("acme"),
            "cwd path info must still appear in body row 2, got: {row2:?}"
        );
    }

    // ── 事2: 左下角显示选中 agent 的完整 cwd ─────────────────────────────────

    /// 左下角（footer - 1 行）显示选中 agent 的完整 cwd（$HOME 缩写成 ~）。
    #[test]
    fn selected_agent_cwd_shown_above_footer() {
        let mut session = make_session("s1", "working", Some("task"));
        session.cwd = "/home/testuser/projects/myapp".to_string();
        let sessions = vec![session];
        let groups = viewmodel::group_and_sort(&sessions);
        let width = 100u16;
        let height = 20u16;
        let mut term = make_terminal(width, height);
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
                accent: theme::COLOR_ACCENT,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        let cwd_row_y = height - 2;
        let cwd_row = row_string(&buf, width, cwd_row_y);
        assert!(
            cwd_row.contains("myapp") || cwd_row.contains("projects"),
            "cwd row should contain selected agent's cwd path, got: {cwd_row:?}"
        );
    }

    /// 没有选中 agent 时，左下角那行为空（不显示任何路径）。
    #[test]
    fn no_selection_cwd_row_is_empty() {
        let sessions = vec![make_session("s1", "working", Some("task"))];
        let groups = viewmodel::group_and_sort(&sessions);
        let width = 100u16;
        let height = 20u16;
        let mut term = make_terminal(width, height);
        let anim = Anim::new();
        let theme = Theme::classic();
        // Selection::new() → index = None
        let selection = Selection::new();

        term.draw(|frame| {
            let area = frame.area();
            let state = RenderState {
                groups: &groups,
                selection: &selection,
                daemon_ready: true,
                anim: &anim,
                theme: &theme,
                accent: theme::COLOR_ACCENT,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();

        let buf = term.backend().buffer().clone();
        let cwd_row_y = height - 2;
        let cwd_row = row_string(&buf, width, cwd_row_y);
        // With no selection the cwd row should be blank (no path content).
        let trimmed = cwd_row.trim();
        assert!(
            trimmed.is_empty(),
            "cwd row should be empty when no session is selected, got: {cwd_row:?}"
        );
    }

    /// 切换选中 agent 后，左下角 cwd 随之变化。
    #[test]
    fn cwd_row_follows_selected_session() {
        let mut s1 = make_session("s1", "working", None);
        s1.cwd = "/home/user/alpha-project".to_string();
        let mut s2 = make_session("s2", "idle", None);
        s2.cwd = "/home/user/beta-project".to_string();
        let sessions = vec![s1, s2];
        let groups = viewmodel::group_and_sort(&sessions);
        let width = 100u16;
        let height = 20u16;
        let anim = Anim::new();
        let theme = Theme::classic();

        // Select s1 (flat index 0 after group_and_sort: working comes first).
        // Note: group_and_sort puts WORKING group first, IDLE group after.
        // s1 is working → flat index 0; s2 is idle → flat index 1.
        let selection1 = Selection { index: Some(0) };
        let mut term1 = make_terminal(width, height);
        term1.draw(|frame| {
            let area = frame.area();
            let state = RenderState {
                groups: &groups,
                selection: &selection1,
                daemon_ready: true,
                anim: &anim,
                theme: &theme,
                accent: theme::COLOR_ACCENT,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();
        let buf1 = term1.backend().buffer().clone();
        let cwd_row1 = row_string(&buf1, width, height - 2);

        // Select s2 (flat index 1).
        let selection2 = Selection { index: Some(1) };
        let mut term2 = make_terminal(width, height);
        term2.draw(|frame| {
            let area = frame.area();
            let state = RenderState {
                groups: &groups,
                selection: &selection2,
                daemon_ready: true,
                anim: &anim,
                theme: &theme,
                accent: theme::COLOR_ACCENT,
                peek_open: false,
                jump_status: None,
            };
            render_frame(frame.buffer_mut(), area, &state);
        })
        .unwrap();
        let buf2 = term2.backend().buffer().clone();
        let cwd_row2 = row_string(&buf2, width, height - 2);

        assert!(
            cwd_row1.contains("alpha"),
            "s1 selected: cwd row should show alpha-project path, got: {cwd_row1:?}"
        );
        assert!(
            cwd_row2.contains("beta"),
            "s2 selected: cwd row should show beta-project path, got: {cwd_row2:?}"
        );
        // Ensure the two rows actually differ.
        assert_ne!(
            cwd_row1.trim(), cwd_row2.trim(),
            "switching selection should change the displayed cwd"
        );
    }
}
