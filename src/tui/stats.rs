//! The `stats` page (toggled with `s`): historical work-time / project /
//! intervention summaries read from `~/.roost/history.db`.
//!
//! This module is presentation only — it pulls aggregates from `crate::history`
//! (the data layer) and renders them. The TUI holds a persistent read-only
//! connection and recomputes [`StatsData`] on the normal fetch cadence.
//!
//! The page is fully scrollable: content is built into a flat list of logical
//! lines, then a window of `height - 2` lines is drawn at the current scroll
//! offset, with `▲`/`▼` affordances so nothing is ever silently cut off.

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};
use rusqlite::Connection;
use unicode_width::UnicodeWidthStr;

use crate::history::{self, DailyWork, ProjectWork, WindowStats};
use crate::tui::layout::{pad_to_width, truncate_to_width};
use crate::tui::theme::{self, Theme};

/// Snapshot of all stats shown on the page. Cheap to recompute each fetch tick.
#[derive(Debug, Clone, Default)]
pub struct StatsData {
    pub today: WindowStats,
    pub week: WindowStats,
    /// Per-day work for the last 7 days, newest first.
    pub daily: Vec<DailyWork>,
    /// Top projects by busy time over the last 7 days.
    pub projects: Vec<ProjectWork>,
}

/// Compute the full stats snapshot from a read-only connection. Each query is
/// resilient: a failure yields an empty section rather than hiding the page.
pub fn compute(conn: &Connection) -> StatsData {
    let now = now_unix();
    StatsData {
        today: history::today_stats(conn, now).unwrap_or_default(),
        week: history::window_stats(conn, now, 7).unwrap_or_default(),
        daily: history::daily_work(conn, now, 7).unwrap_or_default(),
        projects: history::project_breakdown(conn, now, 7, 6).unwrap_or_default(),
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Format a duration in seconds as a compact `4h 12m` / `12m` / `45s` string.
pub fn fmt_dur(secs: i64) -> String {
    let s = secs.max(0);
    if s >= 3600 {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

/// A proportional bar of `█` for `value` relative to `max`, up to `width` cells.
fn bar(value: i64, max: i64, width: usize) -> String {
    if max <= 0 || value <= 0 || width == 0 {
        return String::new();
    }
    let n = ((value as f64 / max as f64) * width as f64).round() as usize;
    "█".repeat(n.clamp(1, width))
}

// ── Logical lines ───────────────────────────────────────────────────────────
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

/// Number of logical lines the page has for `data` (width-independent — used to
/// clamp the scroll offset). Built from the same source as `render_stats`.
pub fn line_count(data: &StatsData) -> usize {
    build_lines(data, 0).len()
}

fn build_lines(data: &StatsData, w: usize) -> Vec<Line> {
    let accent = theme::COLOR_ACCENT;
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Vec::new()); // breathing room under the top border

    lines.push(section_line("TODAY", accent));
    lines.push(kv_line(
        ("work", fmt_dur(data.today.busy_secs)),
        ("turns", data.today.turns.to_string()),
    ));
    lines.push(kv_line(
        ("input", format!("{} ×", data.today.interventions)),
        ("avg wait", fmt_dur(data.today.avg_wait_secs.round() as i64)),
    ));
    lines.push(Vec::new());

    lines.push(section_line("THIS WEEK (7d)", accent));
    lines.push(kv_line(
        ("work", fmt_dur(data.week.busy_secs)),
        ("turns", data.week.turns.to_string()),
    ));
    lines.push(kv_line(
        ("input", format!("{} ×", data.week.interventions)),
        ("avg wait", fmt_dur(data.week.avg_wait_secs.round() as i64)),
    ));
    lines.push(Vec::new());

    if !data.projects.is_empty() {
        lines.push(section_line("BY PROJECT (7d)", accent));
        let max = data.projects.iter().map(|p| p.busy_secs).max().unwrap_or(1);
        for p in &data.projects {
            lines.push(bar_line(&p.name, p.busy_secs, max, w));
        }
        lines.push(Vec::new());
    }

    if !data.daily.is_empty() {
        lines.push(section_line("DAILY (7d)", accent));
        let max = data.daily.iter().map(|d| d.busy_secs).max().unwrap_or(1);
        for d in &data.daily {
            // Show MM-DD (drop the leading "YYYY-").
            let label = d.day.get(5..).unwrap_or(d.day.as_str());
            lines.push(bar_line(label, d.busy_secs, max, w));
        }
        lines.push(Vec::new());
    }

    if data.today.turns == 0
        && data.week.turns == 0
        && data.projects.is_empty()
        && data.daily.is_empty()
    {
        lines.push(vec![Seg {
            col: 2,
            text: "no history yet — stats appear once agents complete turns".to_string(),
            style: Style::default().fg(Color::DarkGray),
        }]);
    }

    lines
}

fn section_line(title: &str, accent: Color) -> Line {
    vec![Seg {
        col: 2,
        text: title.to_string(),
        style: Style::default().fg(accent).add_modifier(Modifier::BOLD),
    }]
}

fn kv_line(left: (&str, String), right: (&str, String)) -> Line {
    let mut segs = Vec::new();
    push_kv(&mut segs, 4, left.0, &left.1);
    push_kv(&mut segs, 24, right.0, &right.1);
    segs
}

fn push_kv(segs: &mut Line, col: u16, label: &str, value: &str) {
    segs.push(Seg {
        col,
        text: label.to_string(),
        style: Style::default().fg(Color::DarkGray),
    });
    let vcol = col + UnicodeWidthStr::width(label) as u16 + 2;
    segs.push(Seg {
        col: vcol,
        text: value.to_string(),
        style: Style::default().fg(Color::Reset),
    });
}

fn bar_line(label: &str, value: i64, max: i64, w: usize) -> Line {
    let label_w = 16usize;
    let dur_w = 9usize;
    let bar_w = w
        .saturating_sub(4 + label_w + 1 + 1 + dur_w + 2)
        .clamp(0, 24);

    let name = pad_to_width(&truncate_to_width(label, label_w), label_w);
    let mut segs = vec![Seg {
        col: 4,
        text: name,
        style: Style::default().fg(Color::Reset),
    }];

    let bar_col = 4 + label_w as u16 + 1;
    let b = bar(value, max, bar_w);
    if !b.is_empty() {
        segs.push(Seg {
            col: bar_col,
            text: b,
            style: Style::default().fg(theme::COLOR_WORKING),
        });
    }

    let dur_col = bar_col + bar_w as u16 + 1;
    segs.push(Seg {
        col: dur_col,
        text: fmt_dur(value),
        style: Style::default().fg(Color::DarkGray),
    });
    segs
}

// ── Render ───────────────────────────────────────────────────────────────────

/// Render the full-screen, scrollable stats page into `buf`. `scroll` is the
/// number of logical lines hidden above the viewport (clamped here defensively).
pub fn render_stats(buf: &mut Buffer, area: Rect, data: &StatsData, theme: &Theme, scroll: usize) {
    if area.height < 2 || area.width < 10 {
        return;
    }
    let x = area.x;
    let accent = theme::COLOR_ACCENT;

    let lines = build_lines(data, area.width as usize);
    let body_h = area.height.saturating_sub(2) as usize; // between top & bottom borders
    let max_scroll = lines.len().saturating_sub(body_h);
    let scroll = scroll.min(max_scroll);

    top_border(buf, x, area.y, area.width, theme, accent, "roost · stats");
    let footer_y = area.y + area.height - 1;
    bottom_border(buf, x, footer_y, area.width, theme, "↑/↓ scroll · esc back · q quit");

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

fn top_border(buf: &mut Buffer, x: u16, y: u16, width: u16, theme: &Theme, accent: Color, title: &str) {
    let w = width as usize;
    let left = "╭─ ";
    let right = " ─╮";
    let tlen = UnicodeWidthStr::width(title);
    let fill = w
        .saturating_sub(UnicodeWidthStr::width(left))
        .saturating_sub(tlen)
        .saturating_sub(1) // the space after the title
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

fn bottom_border(buf: &mut Buffer, x: u16, y: u16, width: u16, theme: &Theme, hint: &str) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render_to_string(data: &StatsData, w: u16, h: u16, scroll: usize) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let theme = Theme::classic();
        term.draw(|f| {
            let area = f.area();
            render_stats(f.buffer_mut(), area, data, &theme, scroll);
        })
        .unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol().to_string())
            .collect()
    }

    fn long_data() -> StatsData {
        StatsData {
            today: WindowStats {
                interventions: 3,
                avg_wait_secs: 90.0,
                turns: 10,
                busy_secs: 4 * 3600 + 12 * 60,
            },
            week: WindowStats {
                interventions: 20,
                avg_wait_secs: 75.0,
                turns: 100,
                busy_secs: 18 * 3600,
            },
            daily: (1..=7)
                .map(|d| DailyWork {
                    day: format!("2026-06-0{d}"),
                    busy_secs: (d as i64) * 600,
                    turns: 1,
                })
                .collect(),
            projects: (0..6)
                .map(|i| ProjectWork {
                    name: format!("proj-{i}"),
                    busy_secs: (i as i64 + 1) * 600,
                    turns: 1,
                })
                .collect(),
        }
    }

    #[test]
    fn fmt_dur_cases() {
        assert_eq!(fmt_dur(0), "0s");
        assert_eq!(fmt_dur(45), "45s");
        assert_eq!(fmt_dur(60), "1m");
        assert_eq!(fmt_dur(125), "2m");
        assert_eq!(fmt_dur(3600), "1h 0m");
        assert_eq!(fmt_dur(3600 + 12 * 60), "1h 12m");
        assert_eq!(fmt_dur(-5), "0s");
    }

    #[test]
    fn bar_is_proportional_and_clamped() {
        assert_eq!(bar(0, 100, 10), "");
        assert_eq!(bar(100, 100, 10).chars().count(), 10);
        assert_eq!(bar(50, 100, 10).chars().count(), 5);
        assert_eq!(bar(1, 1000, 10).chars().count(), 1);
    }

    #[test]
    fn render_empty_stats_says_no_history() {
        let content = render_to_string(&StatsData::default(), 80, 24, 0);
        assert!(content.contains("roost · stats"), "title should render");
        assert!(content.contains("no history yet"), "empty hint should show");
    }

    #[test]
    fn render_populated_stats_shows_sections() {
        let content = render_to_string(&long_data(), 80, 30, 0);
        assert!(content.contains("TODAY"));
        assert!(content.contains("BY PROJECT"));
        assert!(content.contains("4h 12m"), "today work time should render");
    }

    #[test]
    fn long_content_scrolls_and_shows_indicators() {
        let data = long_data();
        let total = line_count(&data);
        assert!(total > 8, "expected long content, got {total}");

        // Short terminal (height 10 → body 8): top visible, bottom hidden, ▼ shown.
        let top = render_to_string(&data, 80, 10, 0);
        assert!(top.contains("TODAY"), "top section visible at scroll 0");
        assert!(top.contains('▼'), "more-below indicator at scroll 0");
        assert!(!top.contains("06-07"), "last day hidden at scroll 0");

        // Scroll past the end (clamped): reveals the bottom, hides the top, ▲ shown.
        let bottom = render_to_string(&data, 80, 10, 1000);
        assert!(bottom.contains("06-07"), "scrolling reveals the last day");
        assert!(bottom.contains('▲'), "more-above indicator after scrolling");
        assert!(!bottom.contains("TODAY"), "top section scrolled away");
    }
}
