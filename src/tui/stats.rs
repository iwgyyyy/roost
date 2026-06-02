//! The `stats` page (toggled with `s`): historical work-time / project /
//! intervention summaries read from `~/.roost/history.db`.
//!
//! This module is presentation only — it pulls aggregates from `crate::history`
//! (the data layer) and renders them. The TUI holds a persistent read-only
//! connection and recomputes [`StatsData`] on the normal fetch cadence.

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

/// Render the full-screen stats page into `buf`.
pub fn render_stats(buf: &mut Buffer, area: Rect, data: &StatsData, theme: &Theme) {
    if area.height < 2 || area.width < 10 {
        return;
    }
    let x = area.x;
    let w = area.width as usize;
    let accent = theme::COLOR_ACCENT;

    // Top + bottom chrome.
    top_border(buf, x, area.y, area.width, theme, accent, "roost · stats");
    let footer_y = area.y + area.height - 1;
    bottom_border(buf, x, footer_y, area.width, theme, "esc back · q quit");

    let max_y = footer_y; // body rows must stay strictly above this
    let mut y = area.y + 2; // one blank line under the top border

    // ── TODAY ──────────────────────────────────────────────────────────────
    y = section(buf, x, y, max_y, "TODAY", accent);
    y = kv_row(
        buf,
        x,
        y,
        max_y,
        ("work", fmt_dur(data.today.busy_secs)),
        ("turns", data.today.turns.to_string()),
    );
    y = kv_row(
        buf,
        x,
        y,
        max_y,
        ("input", format!("{} ×", data.today.interventions)),
        ("avg wait", fmt_dur(data.today.avg_wait_secs.round() as i64)),
    );
    y = blank(y);

    // ── THIS WEEK ────────────────────────────────────────────────────────────
    y = section(buf, x, y, max_y, "THIS WEEK (7d)", accent);
    y = kv_row(
        buf,
        x,
        y,
        max_y,
        ("work", fmt_dur(data.week.busy_secs)),
        ("turns", data.week.turns.to_string()),
    );
    y = kv_row(
        buf,
        x,
        y,
        max_y,
        ("input", format!("{} ×", data.week.interventions)),
        ("avg wait", fmt_dur(data.week.avg_wait_secs.round() as i64)),
    );
    y = blank(y);

    // ── BY PROJECT ───────────────────────────────────────────────────────────
    if !data.projects.is_empty() {
        y = section(buf, x, y, max_y, "BY PROJECT (7d)", accent);
        let max = data.projects.iter().map(|p| p.busy_secs).max().unwrap_or(1);
        for p in &data.projects {
            if y >= max_y {
                break;
            }
            y = labeled_bar(buf, x, y, w, &p.name, p.busy_secs, max);
        }
        y = blank(y);
    }

    // ── DAILY TREND ──────────────────────────────────────────────────────────
    if !data.daily.is_empty() {
        y = section(buf, x, y, max_y, "DAILY (7d)", accent);
        let max = data.daily.iter().map(|d| d.busy_secs).max().unwrap_or(1);
        for d in &data.daily {
            if y >= max_y {
                break;
            }
            // Show MM-DD (drop the leading "YYYY-").
            let label = d.day.get(5..).unwrap_or(d.day.as_str());
            y = labeled_bar(buf, x, y, w, label, d.busy_secs, max);
        }
    }

    // If there is no data at all, say so plainly.
    if data.today.turns == 0
        && data.week.turns == 0
        && data.projects.is_empty()
        && data.daily.is_empty()
        && area.y + 2 < max_y
    {
        buf.set_string(
            x + 2,
            area.y + 2,
            "no history yet — stats appear once agents complete turns",
            Style::default().fg(Color::DarkGray),
        );
    }
}

// ── Rendering helpers ─────────────────────────────────────────────────────────

fn blank(y: u16) -> u16 {
    y + 1
}

/// A section header line in accent bold; returns the next y.
fn section(buf: &mut Buffer, x: u16, y: u16, max_y: u16, title: &str, accent: Color) -> u16 {
    if y < max_y {
        buf.set_string(
            x + 2,
            y,
            title,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        );
    }
    y + 1
}

/// Two `label value` pairs on one line at fixed columns; returns the next y.
fn kv_row(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    max_y: u16,
    left: (&str, String),
    right: (&str, String),
) -> u16 {
    if y < max_y {
        put_kv(buf, x + 4, y, left.0, &left.1);
        put_kv(buf, x + 24, y, right.0, &right.1);
    }
    y + 1
}

/// Render `label` (dim) followed by `value` (default fg) at column `cx`.
fn put_kv(buf: &mut Buffer, cx: u16, y: u16, label: &str, value: &str) {
    buf.set_string(cx, y, label, Style::default().fg(Color::DarkGray));
    let vx = cx + UnicodeWidthStr::width(label) as u16 + 2;
    buf.set_string(vx, y, value, Style::default().fg(Color::Reset));
}

/// `  <label>  <bar>  <dur>` row, scaling the bar to the available width.
fn labeled_bar(buf: &mut Buffer, x: u16, y: u16, w: usize, label: &str, value: i64, max: i64) -> u16 {
    let label_w = 16usize;
    let dur_w = 9usize;
    let bar_w = w
        .saturating_sub(4 + label_w + 1 + 1 + dur_w + 2)
        .clamp(0, 24);

    let name = pad_to_width(&truncate_to_width(label, label_w), label_w);
    buf.set_string(x + 4, y, &name, Style::default().fg(Color::Reset));

    let bar_x = x + 4 + label_w as u16 + 1;
    let b = bar(value, max, bar_w);
    if !b.is_empty() {
        buf.set_string(bar_x, y, &b, Style::default().fg(theme::COLOR_WORKING));
    }

    let dur_x = bar_x + bar_w as u16 + 1;
    buf.set_string(dur_x, y, fmt_dur(value), Style::default().fg(Color::DarkGray));
    y + 1
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
        // Tiny non-zero value still shows at least one cell.
        assert_eq!(bar(1, 1000, 10).chars().count(), 1);
    }

    #[test]
    fn render_empty_stats_does_not_panic_and_says_no_history() {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let theme = Theme::classic();
        let data = StatsData::default();
        term.draw(|f| {
            let area = f.area();
            render_stats(f.buffer_mut(), area, &data, &theme)
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        let content: String = buf.content().iter().map(|c| c.symbol().to_string()).collect();
        assert!(content.contains("roost · stats"), "title should render");
        assert!(content.contains("no history yet"), "empty hint should show");
    }

    #[test]
    fn render_populated_stats_shows_sections() {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let theme = Theme::classic();
        let data = StatsData {
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
            daily: vec![DailyWork {
                day: "2026-06-02".to_string(),
                busy_secs: 4 * 3600,
                turns: 10,
            }],
            projects: vec![ProjectWork {
                name: "roost".to_string(),
                busy_secs: 2 * 3600,
                turns: 8,
            }],
        };
        term.draw(|f| {
            let area = f.area();
            render_stats(f.buffer_mut(), area, &data, &theme)
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        let content: String = buf.content().iter().map(|c| c.symbol().to_string()).collect();
        assert!(content.contains("TODAY"));
        assert!(content.contains("BY PROJECT"));
        assert!(content.contains("roost"));
        assert!(content.contains("4h 12m"), "today work time should render");
    }
}
