//! Responsive column layout and unicode-aware text truncation.
//!
//! Pure functions — no I/O, no terminal state. Fully testable.
//!
//! ## Column budget (spec §8)
//!
//! | Width tier | selbar | icon | family | name | glyph | label | mid (flex)  | time |
//! |------------|--------|------|--------|------|-------|-------|-------------|------|
//! | ≥ 100      | 2      | 2    | 7      | 16   | 1     | 10    | flex (≥ 50) | 4    |
//! | 80–99      | 2      | 2    | 7      | 16   | 1     | —     | flex        | 4    |
//! | 60–79      | 2      | 2    | 7      | 12   | 1     | —     | flex        | 4    |
//! | 40–59      | 2      | —    | —      | 10   | 1     | —     | flex        | —    |
//! | < 40       | —      | —    | —      | flex | 1     | —     | —           | —    |

use unicode_width::UnicodeWidthStr;

// ── ColumnPlan ────────────────────────────────────────────────────────────────

/// Width allocations for one rendered row. `None` means the column is hidden.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnPlan {
    /// 2-column selection bar (▌ or space).
    pub selbar: Option<u16>,
    /// 2-column family icon (✻ / ⬡ / ◆).
    pub icon: Option<u16>,
    /// 9-column family label (fits "DeepSeek"; "Claude"/"Codex"). Hidden on terminals < 60 cols.
    /// Width includes 1 space gap after the label (6 chars + 1 gap = 7).
    pub family: Option<u16>,
    /// Name column (git repo basename).
    pub name: u16,
    /// 1-column state glyph (▲ / ? / spinner / ✓ / ○).
    pub glyph: u16,
    /// 10-column state label text (e.g. "approval"). Hidden on narrow terminals.
    pub label: Option<u16>,
    /// Elastic mid column (activity / task text). `None` only when width < 40.
    pub mid: Option<u16>,
    /// 4-column relative time. Hidden on narrow terminals.
    pub time: Option<u16>,
    /// Whether the terminal is too small to render anything meaningful.
    pub too_small: bool,
}

impl ColumnPlan {
    /// Total fixed overhead (all non-mid columns).
    pub fn fixed_width(&self) -> u16 {
        self.selbar.unwrap_or(0)
            + self.icon.unwrap_or(0)
            + self.family.unwrap_or(0)
            + self.name
            + self.glyph
            + self.label.unwrap_or(0)
            + self.time.unwrap_or(0)
        // 1-space gaps: between icon/family, family/name, name/glyph, glyph/label-or-mid, label/mid, mid/time
        // These are absorbed into the mid flex budget; no extra constant needed.
    }
}

// ── columns_for ───────────────────────────────────────────────────────────────

/// Compute the `ColumnPlan` for a given terminal `width`.
pub fn columns_for(width: u16) -> ColumnPlan {
    if width < 40 {
        // Degenerate: glyph + name only (no selbar, no icon, no family, no mid, no time)
        let name_w = width.saturating_sub(1 /* glyph */ + 1 /* gap */);
        return ColumnPlan {
            selbar: None,
            icon: None,
            family: None,
            name: name_w,
            glyph: 1,
            label: None,
            mid: None,
            time: None,
            too_small: width < 10,
        };
    }

    if width < 60 {
        // 40–59: selbar·glyph·name(≤10)·mid (no icon, no family)
        let fixed = 2 /* selbar */ + 1 /* glyph */ + 10 /* name */ + 1 /* gap */;
        let mid_w = width.saturating_sub(fixed);
        return ColumnPlan {
            selbar: Some(2),
            icon: None,
            family: None,
            name: 10,
            glyph: 1,
            label: None,
            mid: Some(mid_w.max(1)),
            time: None,
            too_small: false,
        };
    }

    if width < 80 {
        // 60–79: selbar·icon·family(9)·name(12)·glyph·mid·time
        let fixed = 2 /* selbar */ + 2 /* icon */ + 9 /* family */ + 12 /* name */ + 1 /* glyph */ + 8 /* time */ + 4; /* gaps */
        let mid_w = width.saturating_sub(fixed);
        return ColumnPlan {
            selbar: Some(2),
            icon: Some(2),
            family: Some(9),
            name: 12,
            glyph: 1,
            label: None,
            mid: Some(mid_w.max(1)),
            time: Some(8),
            too_small: false,
        };
    }

    if width < 100 {
        // 80–99: selbar·icon·family(9)·name(16)·glyph·mid·time (no label)
        let fixed = 2 /* selbar */ + 2 /* icon */ + 9 /* family */ + 16 /* name */ + 1 /* glyph */ + 8 /* time */ + 4; /* gaps */
        let mid_w = width.saturating_sub(fixed);
        return ColumnPlan {
            selbar: Some(2),
            icon: Some(2),
            family: Some(9),
            name: 16,
            glyph: 1,
            label: None,
            mid: Some(mid_w.max(1)),
            time: Some(8),
            too_small: false,
        };
    }

    // ≥ 100: full layout — selbar·icon·family(9)·name(16)·glyph·label·mid·time
    let fixed = 2 /* selbar */
        + 2 /* icon */
        + 9 /* family */
        + 16 /* name */
        + 1 /* glyph */
        + 10 /* label */
        + 8 /* time */
        + 5; /* gaps between columns */
    let mid_w = width.saturating_sub(fixed);
    ColumnPlan {
        selbar: Some(2),
        icon: Some(2),
        family: Some(9),
        name: 16,
        glyph: 1,
        label: Some(10),
        mid: Some(mid_w.max(50)),
        time: Some(8),
        too_small: false,
    }
}

// ── Text truncation / padding ─────────────────────────────────────────────────

/// Truncate `s` so its *display width* (per `unicode-width`) is at most `max_cols`.
/// If truncation is needed, replaces the last visible cell with `…`.
pub fn truncate_to_width(s: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    let display_w = UnicodeWidthStr::width(s);
    if display_w <= max_cols {
        return s.to_string();
    }
    // Need to truncate. Reserve 1 column for `…`.
    let target = max_cols.saturating_sub(1);
    let mut result = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
        if used + cw > target {
            break;
        }
        result.push(ch);
        used += cw;
    }
    result.push('…');
    result
}

/// Pad `s` with trailing spaces so its display width equals exactly `width`.
/// If `s` is already wider than `width`, returns `s` unchanged.
pub fn pad_to_width(s: &str, width: usize) -> String {
    let w = UnicodeWidthStr::width(s);
    if w >= width {
        return s.to_string();
    }
    let mut out = s.to_string();
    for _ in 0..(width - w) {
        out.push(' ');
    }
    out
}

// ── height_profile ─────────────────────────────────────────────────────────────

/// Describes how the TUI should adapt to the terminal height.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeightProfile {
    /// Normal rendering (≥ 16 rows available for the list body).
    Normal,
    /// Compact: hide some decorations but show all groups.
    Compact,
    /// Critical: only NEEDS INPUT rows; other groups show "… N more".
    Critical,
    /// Too small for anything meaningful.
    TooSmall,
}

/// Determine the height profile for a given terminal height.
pub fn height_profile(height: u16) -> HeightProfile {
    match height {
        0..=3 => HeightProfile::TooSmall,
        4..=5 => HeightProfile::Critical,
        6..=15 => HeightProfile::Compact,
        _ => HeightProfile::Normal,
    }
}

// ── visible_window ────────────────────────────────────────────────────────────

/// Compute the scroll offset and whether to show a scrollbar indicator.
///
/// `total_rows`: number of data rows.
/// `selected`: current flat selection index.
/// `viewport_h`: available rows in the viewport (not counting header/footer).
pub fn visible_window(total_rows: usize, selected: usize, viewport_h: usize) -> (usize, bool) {
    if total_rows <= viewport_h {
        return (0, false);
    }
    // Keep the selected row within [offset, offset + viewport_h)
    // Centre strategy: try to keep selected in the middle.
    let half = viewport_h / 2;
    let offset = if selected <= half {
        0
    } else if selected + viewport_h > total_rows + half {
        total_rows.saturating_sub(viewport_h)
    } else {
        selected.saturating_sub(half)
    };
    (offset, true)
}

// ── PeekPlacement ─────────────────────────────────────────────────────────────

/// How the peek panel should be placed given the viewport height.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeekPlacement {
    /// Inline split: main list occupies the top half, peek the bottom half.
    Inline,
    /// Full-screen overlay: peek covers the entire terminal.
    FullScreen,
}

/// Determine how to place the peek panel given the terminal height.
///
/// Rule (spec §8 high-adaptivity):
/// - `viewport_h >= 16` → `Inline` (peek occupies the lower half of the screen)
/// - `viewport_h < 16`  → `FullScreen` (peek covers the whole screen)
pub fn peek_placement(viewport_h: u16) -> PeekPlacement {
    if viewport_h >= 16 {
        PeekPlacement::Inline
    } else {
        PeekPlacement::FullScreen
    }
}

/// Split `area` into `(list_area, peek_area)` for inline mode.
///
/// The list takes the top half; the peek takes the bottom half (rounding up).
pub fn split_for_inline_peek(
    area: ratatui::layout::Rect,
) -> (ratatui::layout::Rect, ratatui::layout::Rect) {
    let list_h = area.height / 2;
    let peek_h = area.height - list_h;
    let list_area = ratatui::layout::Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: list_h,
    };
    let peek_area = ratatui::layout::Rect {
        x: area.x,
        y: area.y + list_h,
        width: area.width,
        height: peek_h,
    };
    (list_area, peek_area)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── columns_for ────────────────────────────────────────────────────────

    #[test]
    fn columns_for_full_width() {
        let plan = columns_for(120);
        assert!(!plan.too_small);
        assert_eq!(plan.selbar, Some(2));
        assert_eq!(plan.icon, Some(2));
        assert_eq!(plan.family, Some(9));
        assert_eq!(plan.name, 16);
        assert_eq!(plan.glyph, 1);
        assert_eq!(plan.label, Some(10));
        assert!(plan.time.is_some());
        // mid = 120 - (2+2+9+16+1+10+8+5) = 67 — must be ≥ 50 with family column
        assert!(plan.mid.unwrap() >= 50, "mid={}", plan.mid.unwrap());
    }

    #[test]
    fn columns_for_100_is_full() {
        let plan = columns_for(100);
        assert_eq!(plan.label, Some(10));
        assert_eq!(plan.family, Some(9));
        // mid = 100 - 53 = 47, clamped to the ≥ 50 floor
        assert!(plan.mid.unwrap() >= 50);
    }

    #[test]
    fn columns_for_80_no_label() {
        let plan = columns_for(80);
        assert_eq!(plan.label, None);
        assert_eq!(plan.icon, Some(2));
        assert_eq!(plan.family, Some(9));
        assert_eq!(plan.time, Some(8));
        assert!(plan.mid.is_some());
        // mid = 80 - (2+2+9+16+1+8+4) = 38
        assert_eq!(plan.mid.unwrap(), 38, "mid={}", plan.mid.unwrap());
    }

    #[test]
    fn columns_for_99_no_label() {
        let plan = columns_for(99);
        assert_eq!(plan.label, None);
        assert_eq!(plan.icon, Some(2));
        assert_eq!(plan.family, Some(9));
        // mid = 99 - (2+2+9+16+1+8+4) = 57
        assert_eq!(plan.mid.unwrap(), 57, "mid={}", plan.mid.unwrap());
    }

    #[test]
    fn columns_for_60_no_label() {
        let plan = columns_for(60);
        assert_eq!(plan.label, None);
        assert_eq!(plan.family, Some(9));
        assert_eq!(plan.name, 12);
        assert!(plan.mid.is_some());
        assert_eq!(plan.time, Some(8));
        // mid = 60 - (2+2+9+12+1+8+4) = 22
        assert_eq!(plan.mid.unwrap(), 22, "mid={}", plan.mid.unwrap());
    }

    #[test]
    fn columns_for_40_no_icon_no_time() {
        let plan = columns_for(40);
        assert_eq!(plan.icon, None);
        assert_eq!(plan.family, None);
        assert_eq!(plan.time, None);
        assert_eq!(plan.name, 10);
        assert!(plan.mid.is_some());
        // mid = 40 - (2+1+10+1) = 26
        assert_eq!(plan.mid.unwrap(), 26, "mid={}", plan.mid.unwrap());
    }

    #[test]
    fn columns_for_39_degenerate() {
        let plan = columns_for(39);
        assert_eq!(plan.icon, None);
        assert_eq!(plan.family, None);
        assert_eq!(plan.label, None);
        assert_eq!(plan.mid, None);
        assert_eq!(plan.time, None);
    }

    // ── family column presence by width tier ──────────────────────────────

    #[test]
    fn family_column_present_at_60_and_above() {
        for width in [60, 80, 99, 100, 120, 200] {
            let plan = columns_for(width);
            assert_eq!(
                plan.family,
                Some(9),
                "family should be Some(9) at width={width}"
            );
        }
    }

    #[test]
    fn family_column_absent_below_60() {
        for width in [40, 50, 59] {
            let plan = columns_for(width);
            assert_eq!(plan.family, None, "family should be None at width={width}");
        }
    }

    #[test]
    fn columns_for_5_too_small() {
        let plan = columns_for(5);
        assert!(plan.too_small);
    }

    // ── truncate_to_width ──────────────────────────────────────────────────

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_width_unchanged() {
        let s = "hello"; // 5 chars, 5 cols
        assert_eq!(truncate_to_width(s, 5), s);
    }

    #[test]
    fn truncate_ascii_adds_ellipsis() {
        let result = truncate_to_width("hello world", 8);
        assert!(result.ends_with('…'));
        assert!(
            UnicodeWidthStr::width(result.as_str()) <= 8,
            "result width {} > 8",
            UnicodeWidthStr::width(result.as_str())
        );
    }

    #[test]
    fn truncate_cjk_does_not_overflow() {
        // Each CJK char is 2 columns wide
        let cjk = "思考中…测试项目名称"; // 9 chars × 2 = 18 cols
        let result = truncate_to_width(cjk, 10);
        let w = UnicodeWidthStr::width(result.as_str());
        assert!(w <= 10, "truncated CJK width {w} > 10, got: {result:?}");
        assert!(result.ends_with('…'));
    }

    #[test]
    fn truncate_mixed_cjk_ascii() {
        let mixed = "project思考中心abc"; // mixed
        for max in [5, 8, 12, 16, 20] {
            let result = truncate_to_width(mixed, max);
            let w = UnicodeWidthStr::width(result.as_str());
            assert!(
                w <= max,
                "width {w} > {max} for input {mixed:?}, got {result:?}"
            );
        }
    }

    #[test]
    fn truncate_zero_width_returns_empty() {
        assert_eq!(truncate_to_width("hello", 0), "");
    }

    // ── pad_to_width ───────────────────────────────────────────────────────

    #[test]
    fn pad_short_string() {
        let result = pad_to_width("hi", 5);
        assert_eq!(UnicodeWidthStr::width(result.as_str()), 5);
        assert_eq!(result, "hi   ");
    }

    #[test]
    fn pad_exact_unchanged() {
        let result = pad_to_width("hello", 5);
        assert_eq!(result, "hello");
    }

    #[test]
    fn pad_wide_unchanged() {
        let result = pad_to_width("hello world", 5);
        assert_eq!(result, "hello world");
    }

    // ── height_profile ─────────────────────────────────────────────────────

    #[test]
    fn height_profile_normal() {
        assert_eq!(height_profile(24), HeightProfile::Normal);
        assert_eq!(height_profile(16), HeightProfile::Normal);
    }

    #[test]
    fn height_profile_compact() {
        assert_eq!(height_profile(15), HeightProfile::Compact);
        assert_eq!(height_profile(6), HeightProfile::Compact);
    }

    #[test]
    fn height_profile_critical() {
        assert_eq!(height_profile(5), HeightProfile::Critical);
        assert_eq!(height_profile(4), HeightProfile::Critical);
    }

    #[test]
    fn height_profile_too_small() {
        assert_eq!(height_profile(0), HeightProfile::TooSmall);
        assert_eq!(height_profile(3), HeightProfile::TooSmall);
    }

    // ── visible_window ──────────────────────────────────────────────────────

    #[test]
    fn visible_window_fits_no_scroll() {
        let (offset, scrollbar) = visible_window(5, 2, 10);
        assert_eq!(offset, 0);
        assert!(!scrollbar);
    }

    #[test]
    fn visible_window_scrolls_when_overflows() {
        let (_, scrollbar) = visible_window(20, 5, 10);
        assert!(scrollbar);
    }

    #[test]
    fn visible_window_selected_is_always_visible() {
        let total = 20;
        let viewport = 5;
        for selected in 0..total {
            let (offset, _) = visible_window(total, selected, viewport);
            assert!(
                selected >= offset && selected < offset + viewport,
                "selected={selected} not in [{offset}, {})",
                offset + viewport
            );
        }
    }

    #[test]
    fn visible_window_selected_zero_offset_zero() {
        let (offset, _) = visible_window(20, 0, 5);
        assert_eq!(offset, 0);
    }

    // ── peek_placement ─────────────────────────────────────────────────────

    #[test]
    fn peek_placement_inline_at_16() {
        assert_eq!(peek_placement(16), PeekPlacement::Inline);
    }

    #[test]
    fn peek_placement_inline_at_24() {
        assert_eq!(peek_placement(24), PeekPlacement::Inline);
    }

    #[test]
    fn peek_placement_inline_at_large() {
        assert_eq!(peek_placement(100), PeekPlacement::Inline);
    }

    #[test]
    fn peek_placement_fullscreen_at_15() {
        assert_eq!(peek_placement(15), PeekPlacement::FullScreen);
    }

    #[test]
    fn peek_placement_fullscreen_at_8() {
        assert_eq!(peek_placement(8), PeekPlacement::FullScreen);
    }

    #[test]
    fn peek_placement_fullscreen_at_zero() {
        assert_eq!(peek_placement(0), PeekPlacement::FullScreen);
    }

    #[test]
    fn split_for_inline_peek_splits_evenly() {
        use ratatui::layout::Rect;
        let area = Rect::new(0, 0, 100, 24);
        let (list, peek) = split_for_inline_peek(area);
        assert_eq!(list.height, 12);
        assert_eq!(peek.height, 12);
        assert_eq!(list.y, 0);
        assert_eq!(peek.y, 12);
    }

    #[test]
    fn split_for_inline_peek_odd_height() {
        use ratatui::layout::Rect;
        let area = Rect::new(0, 0, 100, 17);
        let (list, peek) = split_for_inline_peek(area);
        // list gets the floor, peek gets the ceiling
        assert_eq!(list.height, 8);
        assert_eq!(peek.height, 9);
        assert_eq!(list.height + peek.height, 17);
    }
}
