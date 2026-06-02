//! The `settings` page (toggled with `c`): interactive toggle for per-stage
//! banner and sound notification switches.
//!
//! The page is fully scrollable: content is built into a flat list of logical
//! lines (`Vec<Line>`), then a window of `height - 2` lines is drawn at the
//! current scroll offset, with `▲`/`▼` affordances. The layout mirrors
//! `stats.rs` so the two pages feel consistent.
//!
//! # Interaction model
//!
//! A `Cursor { row, col }` tracks the currently highlighted toggle cell.
//! Pressing `Space` calls `toggle(&cfg, cursor)` — a pure function that returns
//! a new `Config` with the targeted field flipped — and then saves to disk.
//! `Esc` returns to the main panel; `q` quits the TUI.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use crate::config::{Config, Stage};
use crate::tui::theme::{self, Theme};

// ── Cursor ────────────────────────────────────────────────────────────────────

/// Which toggle cell is currently selected.
///
/// `row` selects the stage:
///   0 = clarify · 1 = approve · 2 = done
///
/// `col` selects the field within a stage:
///   0 = banner · 1 = sound
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
    /// Stage row: 0 = clarify, 1 = approve, 2 = done.
    pub row: usize,
    /// Field column: 0 = banner, 1 = sound.
    pub col: usize,
}

impl Cursor {
    pub const MAX_ROW: usize = 2; // 0..=2 inclusive
    pub const MAX_COL: usize = 1; // 0..=1 inclusive
}

/// Sentinel value used internally when no cursor highlight is needed (e.g.
/// when `build_lines` is called solely to count lines). Using MAX values
/// guarantees `cursor.row == row_idx` is always false for real toggle rows.
const NO_CURSOR: Cursor = Cursor {
    row: usize::MAX,
    col: usize::MAX,
};

/// Returns the logical line index (0-based) of the toggle row for `cursor.row`
/// in the line list produced by `build_lines`.
///
/// `build_lines` layout (fixed offsets):
///   0  blank
///   1  section header
///   2  clarify desc
///   3  approve desc
///   4  done desc
///   5  blank
///   6  toggle row 0 (clarify)
///   7  toggle row 1 (approve)
///   8  toggle row 2 (done)
///   9  blank
///  10  hint
///
/// So toggle row `r` is always at line `6 + r`.
pub fn cursor_line(cursor: Cursor) -> usize {
    6 + cursor.row
}

/// Direction for cursor movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// Move the cursor by one step in `dir`, clamping at boundaries.
pub fn move_cursor(c: Cursor, dir: Dir) -> Cursor {
    match dir {
        Dir::Up => Cursor {
            row: c.row.saturating_sub(1),
            col: c.col,
        },
        Dir::Down => Cursor {
            row: (c.row + 1).min(Cursor::MAX_ROW),
            col: c.col,
        },
        Dir::Left => Cursor {
            row: c.row,
            col: c.col.saturating_sub(1),
        },
        Dir::Right => Cursor {
            row: c.row,
            col: (c.col + 1).min(Cursor::MAX_COL),
        },
    }
}

// ── Toggle ────────────────────────────────────────────────────────────────────

/// Return a new `Config` with the field at `cursor` flipped.
///
/// Pure function — does not write to disk; the caller is responsible for
/// calling `config::save()` if the change should be persisted.
pub fn toggle(cfg: &Config, cursor: Cursor) -> Config {
    let mut new_cfg = cfg.clone();
    let stage: &mut Stage = match cursor.row {
        0 => &mut new_cfg.notify.clarify,
        1 => &mut new_cfg.notify.approve,
        _ => &mut new_cfg.notify.done,
    };
    match cursor.col {
        0 => stage.banner = !stage.banner,
        _ => stage.sound = !stage.sound,
    }
    new_cfg
}

// ── Logical lines ─────────────────────────────────────────────────────────────
//
// The page is rendered into a flat `Vec<Line>` first; scrolling then draws a
// window of those lines. A `Line` is a set of styled segments placed at a
// column offset (relative to the page's left edge).

struct Seg {
    col: u16,
    text: String,
    style: Style,
}
type Line = Vec<Seg>;

/// Number of logical lines the page has (width-independent — used to clamp the
/// scroll offset). Mirrors `stats::line_count`.
pub fn line_count(_cfg: &Config) -> usize {
    build_lines(_cfg, None).len()
}

/// Build all logical lines for the page.
///
/// `cursor` is `Some(c)` during an interactive render so the selected toggle
/// cell is highlighted. Pass `None` when computing `line_count` only.
fn build_lines(cfg: &Config, cursor: Option<Cursor>) -> Vec<Line> {
    let accent = theme::COLOR_ACCENT;
    // Breathing room under the top border (matches stats.rs)
    let mut lines: Vec<Line> = vec![Vec::new()];

    // ── Legend / one-liner per stage ─────────────────────────────────────────
    lines.push(section_line("NOTIFICATION SETTINGS", accent));
    lines.push(desc_line(
        "clarify  — agent is waiting for your input (Question)",
        Color::DarkGray,
    ));
    lines.push(desc_line(
        "approve  — agent needs you to approve a dangerous action",
        Color::DarkGray,
    ));
    lines.push(desc_line(
        "done     — agent has finished its task",
        Color::DarkGray,
    ));
    lines.push(Vec::new());

    // ── Toggle rows ──────────────────────────────────────────────────────────
    let stages: [(&str, Stage); 3] = [
        ("clarify", cfg.notify.clarify),
        ("approve", cfg.notify.approve),
        ("done   ", cfg.notify.done),
    ];

    for (row_idx, (label, stage)) in stages.iter().enumerate() {
        let cur = cursor.unwrap_or(NO_CURSOR);
        lines.push(toggle_row(label, stage, row_idx, cur, accent));
    }

    lines.push(Vec::new());
    lines.push(desc_line(
        "space toggle · ↑/↓ row · ←/→ column",
        Color::DarkGray,
    ));

    lines
}

/// A bold section header line (mirrors `stats::section_line`).
fn section_line(title: &str, accent: Color) -> Line {
    vec![Seg {
        col: 2,
        text: title.to_string(),
        style: Style::default().fg(accent).add_modifier(Modifier::BOLD),
    }]
}

/// A plain dim description line.
fn desc_line(text: &str, color: Color) -> Line {
    vec![Seg {
        col: 4,
        text: text.to_string(),
        style: Style::default().fg(color),
    }]
}

/// One toggle row: `  <label>  [banner: on ]  [sound: on ]`
///
/// The currently-selected cell is rendered with a reversed/bold highlight.
fn toggle_row(label: &str, stage: &Stage, row_idx: usize, cursor: Cursor, accent: Color) -> Line {
    let mut segs = Vec::new();

    // Stage label
    segs.push(Seg {
        col: 4,
        text: label.to_string(),
        style: Style::default().fg(Color::Reset).add_modifier(Modifier::BOLD),
    });

    // banner toggle  — at col 16
    let banner_col: u16 = 16;
    let (banner_text, banner_style) = toggle_cell(stage.banner, row_idx, 0, cursor, accent);
    segs.push(Seg {
        col: banner_col,
        text: banner_text,
        style: banner_style,
    });

    // sound toggle   — at col 32
    let sound_col: u16 = 34;
    let (sound_text, sound_style) = toggle_cell(stage.sound, row_idx, 1, cursor, accent);
    segs.push(Seg {
        col: sound_col,
        text: sound_text,
        style: sound_style,
    });

    segs
}

/// Format a single `[banner: on]` / `[sound: off]` cell and pick its style.
///
/// The currently-selected cell gets a bold accent highlight so it's obvious
/// which toggle `Space` would flip.
fn toggle_cell(
    enabled: bool,
    row_idx: usize,
    col_idx: usize,
    cursor: Cursor,
    accent: Color,
) -> (String, Style) {
    let kind = if col_idx == 0 { "banner" } else { "sound" };
    let state = if enabled { "on " } else { "off" };
    let text = format!("[{kind}: {state}]");

    let selected = cursor.row == row_idx && cursor.col == col_idx;
    let style = if selected {
        Style::default()
            .fg(accent)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else if enabled {
        Style::default().fg(Color::Reset)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    (text, style)
}

// ── Render ────────────────────────────────────────────────────────────────────

/// Render the full-screen, scrollable settings page into `buf`.
///
/// `scroll` is the number of logical lines hidden above the viewport (clamped
/// here defensively, mirroring `render_stats`). `cursor` drives the highlighted
/// toggle cell.
pub fn render_settings(
    buf: &mut Buffer,
    area: Rect,
    cfg: &Config,
    theme: &Theme,
    scroll: usize,
    cursor: Cursor,
) {
    if area.height < 2 || area.width < 10 {
        return;
    }
    let x = area.x;
    let accent = theme::COLOR_ACCENT;

    let lines = build_lines(cfg, Some(cursor));
    let body_h = area.height.saturating_sub(2) as usize; // between top & bottom borders
    let max_scroll = lines.len().saturating_sub(body_h);
    let scroll = scroll.min(max_scroll);

    top_border(buf, x, area.y, area.width, theme, accent, "roost · settings");
    let footer_y = area.y + area.height - 1;
    bottom_border(
        buf,
        x,
        footer_y,
        area.width,
        theme,
        "space toggle · ↑/↓ row · ←/→ col · esc back · q quit",
    );

    let body_y = area.y + 1;
    for (i, line) in lines.iter().skip(scroll).take(body_h).enumerate() {
        let y = body_y + i as u16;
        for seg in line {
            buf.set_string(x + seg.col, y, &seg.text, seg.style);
        }
    }

    // Scroll affordances so content is never silently cut off.
    let dim = Style::default().fg(theme.dim);
    if area.width >= 3 {
        if scroll > 0 {
            buf.set_string(x + area.width - 2, area.y, "▲", dim);
        }
        if scroll + body_h < lines.len() {
            buf.set_string(x + area.width - 2, footer_y, "▼", dim);
        }
    }
}

// ── Border helpers (same pattern as stats.rs) ─────────────────────────────────

fn top_border(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    theme: &Theme,
    accent: Color,
    title: &str,
) {
    let w = width as usize;
    let left = "╭─ ";
    let right = " ─╮";
    let tlen = UnicodeWidthStr::width(title);
    let fill = w
        .saturating_sub(UnicodeWidthStr::width(left))
        .saturating_sub(tlen)
        .saturating_sub(1) // space after the title
        .saturating_sub(UnicodeWidthStr::width(right));
    let line = format!("{left}{title} {}{right}", "─".repeat(fill));
    buf.set_string(x, y, &line, Style::default().fg(theme.border));
    buf.set_string(
        x + UnicodeWidthStr::width(left) as u16,
        y,
        title,
        Style::default().fg(accent).add_modifier(Modifier::BOLD),
    );
}

fn bottom_border(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    theme: &Theme,
    hint: &str,
) {
    let w = width as usize;
    let left = "╰── ";
    let right = " ──╯";
    let fill = w
        .saturating_sub(UnicodeWidthStr::width(left))
        .saturating_sub(UnicodeWidthStr::width(hint))
        .saturating_sub(UnicodeWidthStr::width(right));
    let line = format!("{left}{hint}{}{right}", "─".repeat(fill));
    buf.set_string(x, y, &line, Style::default().fg(theme.border));
    buf.set_string(
        x + UnicodeWidthStr::width(left) as u16,
        y,
        hint,
        Style::default().fg(theme.dim),
    );
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, NotifyConfig, Stage};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn all_on() -> Config {
        Config::default() // default is all banner=true, sound=true
    }

    fn all_off() -> Config {
        Config {
            notify: NotifyConfig {
                clarify: Stage {
                    banner: false,
                    sound: false,
                },
                approve: Stage {
                    banner: false,
                    sound: false,
                },
                done: Stage {
                    banner: false,
                    sound: false,
                },
            },
        }
    }

    fn render_to_string(cfg: &Config, w: u16, h: u16, scroll: usize, cursor: Cursor) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let theme = Theme::classic();
        term.draw(|f| {
            let area = f.area();
            render_settings(f.buffer_mut(), area, cfg, &theme, scroll, cursor);
        })
        .unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect()
    }

    // ── toggle() — pure logic ─────────────────────────────────────────────────

    // ① clarify.banner flipped; everything else untouched
    #[test]
    fn toggle_clarify_banner() {
        let cfg = all_on();
        let new_cfg = toggle(&cfg, Cursor { row: 0, col: 0 });
        assert!(!new_cfg.notify.clarify.banner, "clarify.banner should flip to false");
        assert!(new_cfg.notify.clarify.sound, "clarify.sound unchanged");
        assert!(new_cfg.notify.approve.banner, "approve.banner unchanged");
        assert!(new_cfg.notify.approve.sound, "approve.sound unchanged");
        assert!(new_cfg.notify.done.banner, "done.banner unchanged");
        assert!(new_cfg.notify.done.sound, "done.sound unchanged");
    }

    // ② clarify.sound flipped
    #[test]
    fn toggle_clarify_sound() {
        let cfg = all_on();
        let new_cfg = toggle(&cfg, Cursor { row: 0, col: 1 });
        assert!(new_cfg.notify.clarify.banner, "clarify.banner unchanged");
        assert!(!new_cfg.notify.clarify.sound, "clarify.sound should flip to false");
    }

    // ③ done.banner flipped from off → on
    #[test]
    fn toggle_done_banner_off_to_on() {
        let cfg = all_off();
        let new_cfg = toggle(&cfg, Cursor { row: 2, col: 0 });
        assert!(new_cfg.notify.done.banner, "done.banner should flip to true");
        assert!(!new_cfg.notify.done.sound, "done.sound unchanged");
        assert!(!new_cfg.notify.approve.banner, "approve.banner unchanged");
    }

    // ④ approve.sound flipped
    #[test]
    fn toggle_approve_sound() {
        let cfg = all_on();
        let new_cfg = toggle(&cfg, Cursor { row: 1, col: 1 });
        assert!(new_cfg.notify.approve.banner, "approve.banner unchanged");
        assert!(!new_cfg.notify.approve.sound, "approve.sound should flip to false");
        assert!(new_cfg.notify.clarify.sound, "clarify.sound unchanged");
        assert!(new_cfg.notify.done.sound, "done.sound unchanged");
    }

    // ⑤ toggle is a pure function — original cfg unchanged
    #[test]
    fn toggle_does_not_mutate_original() {
        let cfg = all_on();
        let _ = toggle(&cfg, Cursor { row: 0, col: 0 });
        assert!(cfg.notify.clarify.banner, "original cfg must not be mutated");
    }

    // ── move_cursor() — boundary clamping ────────────────────────────────────

    // row upper bound: at row 0, Up should stay at 0
    #[test]
    fn move_cursor_up_clamps_at_zero() {
        let c = Cursor { row: 0, col: 0 };
        assert_eq!(move_cursor(c, Dir::Up), Cursor { row: 0, col: 0 });
    }

    // row lower bound: at MAX_ROW, Down should stay at MAX_ROW
    #[test]
    fn move_cursor_down_clamps_at_max_row() {
        let c = Cursor {
            row: Cursor::MAX_ROW,
            col: 0,
        };
        let moved = move_cursor(c, Dir::Down);
        assert_eq!(moved.row, Cursor::MAX_ROW);
    }

    // col left bound: at col 0, Left stays at 0
    #[test]
    fn move_cursor_left_clamps_at_zero() {
        let c = Cursor { row: 1, col: 0 };
        assert_eq!(move_cursor(c, Dir::Left), Cursor { row: 1, col: 0 });
    }

    // col right bound: at MAX_COL, Right stays at MAX_COL
    #[test]
    fn move_cursor_right_clamps_at_max_col() {
        let c = Cursor {
            row: 1,
            col: Cursor::MAX_COL,
        };
        let moved = move_cursor(c, Dir::Right);
        assert_eq!(moved.col, Cursor::MAX_COL);
    }

    // normal Up/Down movement
    #[test]
    fn move_cursor_up_down_normal() {
        let c = Cursor { row: 1, col: 0 };
        assert_eq!(move_cursor(c, Dir::Up).row, 0);
        assert_eq!(move_cursor(c, Dir::Down).row, 2);
    }

    // normal Left/Right movement
    #[test]
    fn move_cursor_left_right_normal() {
        let c = Cursor { row: 0, col: 0 };
        let r = move_cursor(c, Dir::Right);
        assert_eq!(r.col, 1);
        let l = move_cursor(r, Dir::Left);
        assert_eq!(l.col, 0);
    }

    // ── render snapshot — content correctness ─────────────────────────────────

    #[test]
    fn render_shows_title_and_stages() {
        let cfg = all_on();
        let cursor = Cursor::default();
        let content = render_to_string(&cfg, 80, 24, 0, cursor);

        assert!(content.contains("roost · settings"), "title must appear");
        assert!(content.contains("clarify"), "clarify stage must appear");
        assert!(content.contains("approve"), "approve stage must appear");
        assert!(content.contains("done"), "done stage must appear");
        assert!(content.contains("banner"), "banner toggle must appear");
        assert!(content.contains("on"), "on state must appear when all on");
    }

    #[test]
    fn render_shows_off_when_disabled() {
        let cfg = all_off();
        let cursor = Cursor::default();
        let content = render_to_string(&cfg, 80, 24, 0, cursor);
        assert!(content.contains("off"), "off state must appear when all off");
    }

    #[test]
    fn render_shows_sound_label() {
        let cfg = all_on();
        let cursor = Cursor::default();
        let content = render_to_string(&cfg, 80, 24, 0, cursor);
        assert!(content.contains("sound"), "sound label must appear");
    }

    // ── scroll affordances ────────────────────────────────────────────────────

    #[test]
    fn short_terminal_shows_down_arrow() {
        let cfg = all_on();
        let total = line_count(&cfg);
        assert!(total > 4, "expected content taller than 4 lines, got {total}");

        // Very short terminal (height 4 → body 2): content overflows → ▼ shown.
        let content = render_to_string(&cfg, 80, 4, 0, Cursor::default());
        assert!(content.contains('▼'), "▼ must appear when content overflows below");
    }

    #[test]
    fn scrolling_to_bottom_shows_up_arrow() {
        let cfg = all_on();
        // Scroll past the end (will be clamped); only bottom visible → ▲ shown.
        let content = render_to_string(&cfg, 80, 4, 9999, Cursor::default());
        assert!(content.contains('▲'), "▲ must appear when scrolled past top content");
    }

    #[test]
    fn no_scroll_indicator_when_all_fits() {
        let cfg = all_on();
        // Very tall terminal — all content fits.
        let content = render_to_string(&cfg, 80, 50, 0, Cursor::default());
        assert!(!content.contains('▼'), "▼ must not appear when all content fits");
    }

    // ── cursor_line / scroll helper ───────────────────────────────────────────

    /// cursor_line maps each cursor row to the correct logical line index.
    #[test]
    fn cursor_line_offsets_are_correct() {
        assert_eq!(cursor_line(Cursor { row: 0, col: 0 }), 6, "clarify toggle at line 6");
        assert_eq!(cursor_line(Cursor { row: 1, col: 0 }), 7, "approve toggle at line 7");
        assert_eq!(cursor_line(Cursor { row: 2, col: 0 }), 8, "done toggle at line 8");
    }

    /// On a short viewport (height 6 → body_h 4), moving the cursor to the
    /// last row (done, line 8) from scroll=0 must advance scroll so that line
    /// 8 is visible — i.e. scroll must become > 0 and the ▲ indicator shows.
    #[test]
    fn scroll_advances_to_keep_cursor_visible_on_short_terminal() {
        // Simulate the auto-scroll logic from mod.rs for the last cursor row.
        let cursor = Cursor { row: 2, col: 0 }; // done row → line 8
        let body_h: usize = 4; // terminal height 6 → body 4
        let line = cursor_line(cursor); // 8
        let mut scroll: usize = 0;

        if line < scroll {
            scroll = line;
        } else if body_h > 0 && line >= scroll + body_h {
            scroll = line - body_h + 1;
        }

        // scroll must have advanced so line 8 is visible in [scroll, scroll+body_h)
        assert!(scroll > 0, "scroll should have advanced past 0, got {scroll}");
        assert!(
            line >= scroll && line < scroll + body_h,
            "cursor line {line} must be within viewport [{scroll}, {})",
            scroll + body_h
        );

        // Verify visually: rendering with the computed scroll shows ▲
        let cfg = all_on();
        let content = render_to_string(&cfg, 80, 6, scroll, cursor);
        assert!(content.contains('▲'), "▲ must appear when scrolled past top content");
    }

    // ── line_count sanity ─────────────────────────────────────────────────────

    #[test]
    fn line_count_is_positive_and_stable() {
        let cfg = all_on();
        let n = line_count(&cfg);
        // We have: blank + section_header + 3 desc lines + blank + 3 toggle rows
        //          + blank + hint line = 11 lines minimum
        assert!(n >= 11, "expected at least 11 logical lines, got {n}");
        // Config doesn't affect line count (no conditional sections)
        let cfg2 = all_off();
        assert_eq!(line_count(&cfg2), n, "line_count must not depend on toggle values");
    }
}
