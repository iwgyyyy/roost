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

// ── card_viewport ─────────────────────────────────────────────────────────────

/// Result of [`card_viewport`]: which card range to render and the pixel-row
/// scroll offset into the virtual card stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardViewport {
    /// Index of the first card to render (inclusive).
    pub first_card: usize,
    /// Index of the last card to render (inclusive).
    pub last_card: usize,
    /// Row offset from the top of `first_card` at which the viewport starts.
    /// Always 0 (we never start mid-card — the invariant is that the selected
    /// card is fully visible, so we align the viewport to card boundaries).
    pub row_offset: usize,
    /// Whether a scrollbar indicator should be shown.
    pub scrollbar: bool,
}

/// Compute which cards are visible in the viewport given variable per-card
/// heights.
///
/// ## Parameters
///
/// * `card_heights` — height (in terminal rows) of **each** item in the flat
///   list.  This may include group-header "cards" (height 1) interleaved with
///   session cards (height 4) — callers build this slice however they like.
/// * `selected` — flat index of the currently selected card.
/// * `viewport_h` — number of terminal rows available for the card list body
///   (does **not** include the TUI header/footer rows).
///
/// ## Guarantees
///
/// * The selected card is always fully visible (never clipped at either edge).
/// * When all cards fit in the viewport, `first_card = 0`, `last_card =
///   card_heights.len() - 1`, and `scrollbar = false`.
/// * When the viewport is shorter than a single card we still show that card
///   (best-effort — the card will be clipped, but we still include it in the
///   range so the renderer can decide how to handle it).
/// * Empty input (`card_heights.is_empty()`) → `first_card = last_card = 0`,
///   `scrollbar = false`.
pub fn card_viewport(
    card_heights: &[usize],
    selected: usize,
    viewport_h: usize,
) -> CardViewport {
    if card_heights.is_empty() {
        return CardViewport {
            first_card: 0,
            last_card: 0,
            row_offset: 0,
            scrollbar: false,
        };
    }

    let n = card_heights.len();
    // Clamp selected to valid range.
    let selected = selected.min(n - 1);

    // Total height of all cards combined.
    let total_h: usize = card_heights.iter().sum();

    if total_h <= viewport_h {
        // Everything fits — no scrolling needed.
        return CardViewport {
            first_card: 0,
            last_card: n - 1,
            row_offset: 0,
            scrollbar: false,
        };
    }

    // Prefix-sum: row_start[i] = sum of heights of cards 0..i.
    let mut row_start = Vec::with_capacity(n);
    let mut acc = 0usize;
    for &h in card_heights {
        row_start.push(acc);
        acc += h;
    }

    let sel_top = row_start[selected];
    let sel_h = card_heights[selected];
    let sel_bottom = sel_top + sel_h; // exclusive

    // Strategy: try to centre the viewport on the selected card, snapping to
    // card boundaries (so we never show a partial card at the top).
    //
    // Desired viewport start: centre of selected card minus half viewport,
    // then snapped to the nearest card's top edge.
    let sel_mid = sel_top + sel_h / 2;
    let desired_start = sel_mid.saturating_sub(viewport_h / 2);

    // Snap desired_start down to the nearest card boundary (i.e. find the
    // largest row_start[i] that is ≤ desired_start).
    let snapped_start = {
        let mut best = 0usize;
        for &rs in &row_start {
            if rs <= desired_start {
                best = rs;
            } else {
                break;
            }
        }
        best
    };

    // Clamp so the viewport doesn't scroll past the last row.
    let max_start = total_h.saturating_sub(viewport_h);
    // Also snap max_start to a card boundary.
    let max_start_snapped = {
        let mut best = 0usize;
        for &rs in &row_start {
            if rs <= max_start {
                best = rs;
            } else {
                break;
            }
        }
        best
    };

    let mut vp_start = snapped_start.min(max_start_snapped);

    // Hard constraint: the selected card must be fully visible.
    // If the selected card's top is above vp_start, scroll up to its top.
    if sel_top < vp_start {
        vp_start = sel_top;
    }
    // If the selected card's bottom is below vp_start + viewport_h, scroll
    // down so the selected card's bottom aligns with the viewport bottom.
    // (Only when the card fits; if it's taller than the viewport we prioritise
    // showing the top of the card.)
    let vp_end = vp_start + viewport_h;
    if sel_bottom > vp_end && sel_h <= viewport_h {
        // Snap the new vp_start to the card boundary just above sel_bottom - viewport_h.
        let raw_start = sel_bottom.saturating_sub(viewport_h);
        let mut best = 0usize;
        for &rs in &row_start {
            if rs <= raw_start {
                best = rs;
            } else {
                break;
            }
        }
        vp_start = best;
    }

    // Determine first and last visible card indices based on vp_start / vp_end.
    let vp_end = vp_start + viewport_h;

    let first_card = {
        // First card whose bottom edge is > vp_start (i.e. at least partially visible).
        let mut idx = 0;
        for (i, &h) in card_heights.iter().enumerate() {
            let bottom = row_start[i] + h;
            if bottom > vp_start {
                idx = i;
                break;
            }
            idx = i;
        }
        idx
    };

    let last_card = {
        // Last card whose top edge is < vp_end (at least partially visible).
        let mut idx = first_card;
        for (i, _) in card_heights.iter().enumerate().skip(first_card) {
            if row_start[i] < vp_end {
                idx = i;
            } else {
                break;
            }
        }
        idx
    };

    // row_offset: how many rows of first_card are hidden above the viewport top.
    // With card-boundary snapping this should always be 0, but we compute it
    // defensively so callers can detect any edge-case clipping.
    let row_offset = vp_start.saturating_sub(row_start[first_card]);

    CardViewport {
        first_card,
        last_card,
        row_offset,
        scrollbar: true,
    }
}

// ── v2 card layout ────────────────────────────────────────────────────────────

/// Truncate a `cwd` path so its display width fits within `max_cols`,
/// **preserving the last path segment** (the basename).
///
/// The algorithm:
/// 1. If the full path fits, return it unchanged.
/// 2. Otherwise, split off the basename (after the last `/`).
/// 3. If the basename alone (with `…/` prefix) fits, show `…/<basename>`.
/// 4. If even the basename is too long, truncate it from the right with `…`.
///
/// The result is always ≤ `max_cols` display columns.
pub fn truncate_cwd(cwd: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    let full_w = UnicodeWidthStr::width(cwd);
    if full_w <= max_cols {
        return cwd.to_string();
    }
    // Split off the basename.
    let (_, basename) = cwd.rsplit_once('/').unwrap_or(("", cwd));
    // Attempt to show "…/<basename>".
    let candidate = format!("…/{}", basename);
    let cand_w = UnicodeWidthStr::width(candidate.as_str());
    if cand_w <= max_cols {
        return candidate;
    }
    // Basename itself is too wide; truncate the basename from the right.
    // We have "…" (1 col) + "/" (1 col) = 2 col overhead.
    let avail_for_name = max_cols.saturating_sub(2);
    let truncated_name = truncate_to_width(basename, avail_for_name);
    format!("…/{}", truncated_name)
}

// ── CardLayoutV2 ──────────────────────────────────────────────────────────────

/// v2 card layout: 4-row card matching the design spec §1.
///
/// Layout:
/// ```text
/// ┌─ <icon> <name> ─×D─ <state_glyph> <state_word> · <time_str> ─┐
/// │ <step_prefix> <step_text>                                       │
/// │ <cwd_display>                    <source_str><via_str>          │
/// └─────────────────────────────────────────────────────────────────┘
/// ```
///
/// All string fields are already truncated to fit their column budgets.
/// The renderer (Task 6) can emit these directly without further measurement.
///
/// Geometry notes:
/// - `height` is always 4.
/// - `inner_width = width - 2` (one column per border character).
/// - `fill_d` is the number of `─` (or `┄` when offline) characters to place
///   between the name and the state section in the top border.  It is
///   pre-computed so the renderer does not have to measure anything.
///   Formula (spec §1 D): `D = inner_width - (name_w + state_word_w + time_w + 13)`.
///   A minimum of 1 is guaranteed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardLayoutV2 {
    // ── Top border ────────────────────────────────────────────────────────
    /// Family icon glyph (`✻` / `⬡` / `◆`). Always 1 display column.
    pub icon: String,
    /// Session name (cwd basename). Truncated to fit name budget.
    pub name: String,
    /// Number of fill-dash columns between name and state section (D ≥ 1).
    pub fill_d: usize,
    /// State glyph placeholder (e.g. `▲` / `?` / `⠋` / `✓` / `○` / `⊘`).
    /// The live spinner frame is overlaid at render time; this is the static
    /// placeholder so the renderer can measure its column width.
    pub state_glyph: String,
    /// State word shown in the top border (e.g. `Approval`, `Working`, `offline`).
    pub state_word: String,
    /// Time string, always exactly 3 display columns wide (e.g. ` 8s`, `12s`).
    /// For offline (stale) sessions this represents "last seen".
    pub time_str: String,

    // ── Body row 1: step line ─────────────────────────────────────────────
    /// Step prefix: `▸` for online sessions, `⊘` for offline.
    pub step_prefix: String,
    /// Step text (activity / done summary / last known step). Truncated to fit.
    pub step_text: String,

    // ── Body row 2: cwd + source line ─────────────────────────────────────
    /// Full working-directory path, right-truncated with `…` to preserve the
    /// last path segment. Fits within `cwd_cols` columns.
    pub cwd_display: String,
    /// Source label: `@local` (dim) or `@<device>` (colour).
    pub source_str: String,
    /// App label: `· via <app>` or empty string when host_app is unknown.
    pub via_str: String,

    // ── Geometry ─────────────────────────────────────────────────────────
    /// Total card height (always 4).
    pub height: u16,
    /// Total card width (equals the terminal width passed in).
    pub width: u16,
    /// Inner content width (`width − 2`).
    pub inner_width: u16,
    /// Whether this card represents an offline (stale) session.
    pub is_offline: bool,
}

/// Compute the v2 `CardLayoutV2` for `session` at the given terminal `width`.
///
/// Pure function — no I/O, no buffer writes, no runtime animation state.
/// The spinner placeholder in `state_glyph` must be replaced by the live
/// spinner frame at render time (Task 6/9).
pub fn card_layout_v2(width: u16, session: &crate::protocol::SessionView) -> CardLayoutV2 {
    let width = width.max(6);
    let inner_width = width.saturating_sub(2) as usize;
    // body content width = inner_width - 2 (one leading space + one trailing space)
    let _body_w = inner_width.saturating_sub(2);

    let is_offline = session.stale;

    // ── Icon ─────────────────────────────────────────────────────────────
    let icon = session.family.icon().to_string();
    // icon is 1 display col (all spec glyphs are BMP, width 1).

    // ── State word + glyph ────────────────────────────────────────────────
    // State word maps state string → display word (Title-case).
    let state_word_raw = if is_offline {
        "offline"
    } else {
        match session.state.as_str() {
            "approval" => "Approval",
            "question" => "Question",
            "working"  => "Working",
            "done"     => "Done",
            _          => "Idle",
        }
    };

    let (glyph_placeholder, _) = crate::tui::theme::state_glyph(if is_offline {
        "idle" // offline shows ⊘ via step_prefix; state_glyph is overridden below
    } else {
        &session.state
    });
    let state_glyph = if is_offline {
        "⊘".to_string()
    } else {
        glyph_placeholder.to_string()
    };

    // ── Time string ───────────────────────────────────────────────────────
    // Source:
    //   Offline  → last_activity_secs (last-seen elapsed)
    //   Online   → busy_secs for all states (working: growing; done/idle: frozen
    //              at turn end; approval/question: still active)
    let time_secs = if is_offline {
        session.last_activity_secs
    } else {
        session.busy_secs
    };
    // Format with natural-width relative_time.  No fixed padding — fill_d (D)
    // absorbs the width change as the counter ticks through values like
    // "2s" (2 cols), "36s" (3 cols), "1m30s" (5 cols), "59m59s" (6 cols).
    // Shorter time → longer D → time still hugs the right border.
    let time_str = crate::tui::anim::relative_time(time_secs);

    // ── D computation ─────────────────────────────────────────────────────
    // Top border layout (inner = width - 2, excludes ┌ and ┐):
    //   ┌ <icon> <name> ─×D─ <state_glyph> <state_word> · <time> ─┐
    //
    // Inner content (all between ┌ and ┐):
    //   sp(1) + icon(1) + sp(1) + name + sp(1)
    //   + fill_d + sp(1) + glyph(1) + sp(1) + word + " · "(3) + time + sp(1)
    //
    // Fixed overhead (excluding name, word, time, D) = 1+1+1+1+1+1+1+3+1 = 11
    //   (icon=1, glyph=1 included in that count)
    //
    // D = inner - icon(1) - 1(sp) - name - 1(sp) - 1(sp) - glyph(1) - 1(sp)
    //       - word - 3(" · ") - time - 1(sp, before ┐)
    //   = inner - 1(sp_before_icon) - icon - 1 - name - 1 - 1 - glyph - 1 - word - 3 - time - 1
    //
    // Glyph is 1 col.
    let state_word_w = UnicodeWidthStr::width(state_word_raw);
    // time_w = actual display width of time_str (natural, no padding).
    let time_w = UnicodeWidthStr::width(time_str.as_str());
    // icon is always 1 col.
    let icon_w = 1usize;

    // Glyph is 1 col.
    let glyph_w = UnicodeWidthStr::width(state_glyph.as_str());

    // Name budget: leave at least 1 fill dash (D ≥ 1).
    // Fixed overhead = 1(sp_before_icon) + icon(1) + sp(1) + sp_after_name(1)
    //                + D(≥1) + sp(1) + glyph(1) + sp(1) + " · "(3) + sp_before_corner(1)
    //                = 11 (with D counted as 1)
    // So: name_budget = inner - word_w - time_w - 11 - D(≥1=1) = inner - word_w - time_w - 12
    // Using 14 for conservatism (matches prior style of slightly conservative budget).
    let name_budget = inner_width
        .saturating_sub(state_word_w + time_w + 14 + 1)
        .max(4); // minimum 4 cols for the name
    // Card top border shows the agent family name (e.g. "Claude", "Codex", "CodeWhale",
    // "Cursor") instead of the session's cwd basename.  The full cwd is still shown in
    // body row 2.  `session.name` (cwd basename) is preserved on SessionView for other
    // callers (peek title, etc.) — we only swap the card-label here.
    let card_name_raw = session.family.card_label();
    let name = truncate_to_width(card_name_raw, name_budget);
    let name_w = UnicodeWidthStr::width(name.as_str());

    // Now compute D (clamped to ≥ 1).
    // Formula: D = inner - 1(sp_before_icon) - icon - sp - name - sp - sp - glyph - sp - word - 3 - time - sp
    let d_raw = inner_width
        .saturating_sub(1 + icon_w + 1 + name_w + 1 + 1 + glyph_w + 1 + state_word_w + 3 + time_w + 1);
    let fill_d = d_raw.max(1);

    // ── Step line (body row 1) ────────────────────────────────────────────
    // Slot mapping (spec §2):
    //   Working/Approval/Question → activity
    //   Idle/Done                → done_summary = last_prompt (or empty)
    //   Offline (stale)          → last_prompt / activity (last known step)
    let step_prefix = if is_offline {
        "⊘".to_string()
    } else {
        "▸".to_string()
    };

    let step_raw = if is_offline {
        // Offline: last known step — prefer last_prompt, fall back to activity.
        session.last_prompt.clone()
            .or_else(|| session.activity.clone())
            .unwrap_or_default()
    } else {
        match session.state.as_str() {
            "working" | "approval" | "question" => {
                session.activity.clone().unwrap_or_default()
            }
            _ => {
                // Idle / Done: show done summary (last_prompt). A fresh session
                // that has never received a prompt falls back to a placeholder
                // so the step line is never blank.
                session
                    .last_prompt
                    .clone()
                    .unwrap_or_else(|| "New session".to_string())
            }
        }
    };

    // Body content budget: inner_width - 2 (leading space + step_prefix).
    // step_prefix is 1 col; one space before it; one space after.
    // Available for step_text = inner_width - 1(sp) - 1(prefix) - 1(sp) = inner_width - 3
    let step_budget = inner_width.saturating_sub(3);
    let step_text = truncate_to_width(&step_raw, step_budget);

    // ── Source string ─────────────────────────────────────────────────────
    let source_str = if session.origin == "local" {
        "@local".to_string()
    } else {
        format!("@{}", session.origin)
    };

    // ── Via string ────────────────────────────────────────────────────────
    let via_str = match session.host_app.as_deref() {
        Some(app) if !app.is_empty() => format!(" · via {}", app),
        _ => String::new(),
    };

    // ── CWD display (body row 2 left) ─────────────────────────────────────
    // Budget: body_w - source_str_w - via_str_w - 2 (separation spaces)
    // The row 2 layout: " <cwd>  <source><via> "
    // Right-align source+via; left-align cwd.
    // Minimum separation between cwd and source: 1 space.
    let source_w = UnicodeWidthStr::width(source_str.as_str());
    let via_w = UnicodeWidthStr::width(via_str.as_str());
    let right_part_w = source_w + via_w;
    // cwd budget: inner_width - 1(leading sp) - right_part_w - 1(sep) - 1(trailing sp)
    // = inner_width - right_part_w - 3
    let cwd_budget = inner_width.saturating_sub(right_part_w + 3).max(4);
    let cwd_display = truncate_cwd(&session.cwd, cwd_budget);

    CardLayoutV2 {
        icon,
        name,
        fill_d,
        state_glyph,
        state_word: state_word_raw.to_string(),
        time_str,
        step_prefix,
        step_text,
        cwd_display,
        source_str,
        via_str,
        height: 4,
        width,
        inner_width: inner_width as u16,
        is_offline,
    }
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

    // ── card_viewport ─────────────────────────────────────────────────────────

    /// Helper: build a heights slice of `n` uniform cards, each `h` rows tall.
    fn uniform_heights(n: usize, h: usize) -> Vec<usize> {
        vec![h; n]
    }

    // --- Empty input ---

    #[test]
    fn card_viewport_empty_input_returns_zero_range() {
        let cv = card_viewport(&[], 0, 20);
        assert_eq!(cv.first_card, 0);
        assert_eq!(cv.last_card, 0);
        assert!(!cv.scrollbar);
    }

    // --- Everything fits (no scrolling) ---

    #[test]
    fn card_viewport_all_fit_no_scroll() {
        // 3 cards × 4 rows = 12 rows, viewport = 20 → no scroll.
        let heights = uniform_heights(3, 4);
        let cv = card_viewport(&heights, 1, 20);
        assert_eq!(cv.first_card, 0);
        assert_eq!(cv.last_card, 2);
        assert_eq!(cv.row_offset, 0);
        assert!(!cv.scrollbar);
    }

    #[test]
    fn card_viewport_exact_fit_no_scroll() {
        // 3 cards × 4 rows = 12, viewport = 12 → no scroll.
        let heights = uniform_heights(3, 4);
        let cv = card_viewport(&heights, 0, 12);
        assert!(!cv.scrollbar, "exact fit should not show scrollbar");
        assert_eq!(cv.first_card, 0);
        assert_eq!(cv.last_card, 2);
    }

    // --- Single card ---

    #[test]
    fn card_viewport_single_card_no_scroll() {
        let cv = card_viewport(&[4], 0, 20);
        assert_eq!(cv.first_card, 0);
        assert_eq!(cv.last_card, 0);
        assert!(!cv.scrollbar);
    }

    #[test]
    fn card_viewport_single_card_exact_fit() {
        let cv = card_viewport(&[4], 0, 4);
        assert_eq!(cv.first_card, 0);
        assert_eq!(cv.last_card, 0);
        assert!(!cv.scrollbar);
    }

    // --- Selection at first card ---

    #[test]
    fn card_viewport_selected_first_card_offset_zero() {
        // 10 cards × 4 rows = 40 rows, viewport = 12.
        let heights = uniform_heights(10, 4);
        let cv = card_viewport(&heights, 0, 12);
        assert_eq!(cv.first_card, 0, "first visible should be card 0");
        assert_eq!(cv.row_offset, 0, "no partial offset at top");
        assert!(cv.scrollbar);
        // Card 0 must be fully within the rendered range.
        assert!(cv.last_card >= cv.first_card);
    }

    // --- Selection at last card ---

    #[test]
    fn card_viewport_selected_last_card_fully_visible() {
        // 10 cards × 4 rows = 40 rows, viewport = 12.
        let heights = uniform_heights(10, 4);
        let n = heights.len();
        let cv = card_viewport(&heights, n - 1, 12);
        assert_eq!(cv.last_card, n - 1, "last visible must be the last card");
        assert!(cv.scrollbar);
        // row_offset must be 0 since we align to card boundaries.
        assert_eq!(cv.row_offset, 0);
    }

    // --- Selected card must always be fully in range ---

    #[test]
    fn card_viewport_selected_always_in_range() {
        let heights = uniform_heights(20, 4);
        let viewport_h = 12; // 3 cards
        for sel in 0..heights.len() {
            let cv = card_viewport(&heights, sel, viewport_h);
            assert!(
                cv.first_card <= sel && sel <= cv.last_card,
                "selected={sel} not in [{}, {}]",
                cv.first_card,
                cv.last_card
            );
        }
    }

    // --- No partial card visible (row_offset == 0) ---

    #[test]
    fn card_viewport_row_offset_always_zero_for_uniform_cards() {
        // With uniform card heights, the viewport always aligns to card boundaries.
        let heights = uniform_heights(10, 4);
        let viewport_h = 8; // exactly 2 cards
        for sel in 0..heights.len() {
            let cv = card_viewport(&heights, sel, viewport_h);
            assert_eq!(
                cv.row_offset, 0,
                "row_offset should be 0 for uniform cards; sel={sel}"
            );
        }
    }

    // --- Mixed heights (group headers interleaved) ---

    #[test]
    fn card_viewport_mixed_heights_selected_in_range() {
        // Pattern: header(1) + 3 cards(4) + header(1) + 3 cards(4) = 1+12+1+12 = 26
        let heights = vec![1, 4, 4, 4, 1, 4, 4, 4];
        let viewport_h = 9;
        for sel in 0..heights.len() {
            let cv = card_viewport(&heights, sel, viewport_h);
            assert!(
                cv.first_card <= sel && sel <= cv.last_card,
                "selected={sel} not in [{}, {}]",
                cv.first_card,
                cv.last_card
            );
        }
    }

    #[test]
    fn card_viewport_group_header_at_top() {
        // Header (h=1) then cards (h=4). When selecting card 1 (first session
        // card) the group header should scroll in too.
        let heights = vec![1, 4, 4, 4, 4, 4]; // header + 5 session cards = 21 rows
        let viewport_h = 9; // fits header(1) + 2 session cards(8)
        let cv = card_viewport(&heights, 1, viewport_h);
        // Card 0 (header) and card 1 (selected) must both be visible.
        assert!(
            cv.first_card <= 1 && cv.last_card >= 1,
            "selected card 1 must be visible; got [{}, {}]",
            cv.first_card,
            cv.last_card
        );
    }

    // --- Viewport shorter than one card (extreme case) ---

    #[test]
    fn card_viewport_shorter_than_one_card() {
        // viewport_h = 2, card height = 4 — card is taller than viewport.
        let heights = uniform_heights(5, 4);
        let cv = card_viewport(&heights, 2, 2);
        // Selected card must be in the rendered range even if it's clipped.
        assert!(
            cv.first_card <= 2 && cv.last_card >= 2,
            "selected card 2 must be in range [{}, {}] even with tiny viewport",
            cv.first_card,
            cv.last_card
        );
    }

    // --- Scrollbar flag ---

    #[test]
    fn card_viewport_scrollbar_when_content_overflows() {
        let heights = uniform_heights(10, 4); // 40 rows
        let cv = card_viewport(&heights, 0, 20); // viewport = 20 — content overflows
        assert!(cv.scrollbar, "scrollbar should be shown when content overflows");
    }

    #[test]
    fn card_viewport_no_scrollbar_when_content_fits() {
        let heights = uniform_heights(2, 4); // 8 rows
        let cv = card_viewport(&heights, 0, 20); // viewport = 20 — everything fits
        assert!(!cv.scrollbar);
    }

    // --- Rendered range covers enough rows to fill the viewport ---

    #[test]
    fn card_viewport_range_covers_viewport_when_enough_content() {
        let heights = uniform_heights(10, 4); // 40 rows
        let viewport_h = 12usize;
        let cv = card_viewport(&heights, 5, viewport_h);
        let rendered_rows: usize = heights[cv.first_card..=cv.last_card].iter().sum::<usize>()
            - cv.row_offset;
        assert!(
            rendered_rows >= viewport_h,
            "rendered rows {rendered_rows} should be >= viewport_h {viewport_h}"
        );
    }

    // ── truncate_cwd ──────────────────────────────────────────────────────────

    #[test]
    fn truncate_cwd_short_path_unchanged() {
        let path = "~/dev/myproject";
        let result = truncate_cwd(path, 40);
        assert_eq!(result, path);
    }

    #[test]
    fn truncate_cwd_long_path_preserves_basename() {
        let path = "~/dev/acme/payments-api";
        let result = truncate_cwd(path, 14);
        // "…/payments-api" = 1 + 1 + 12 = 14 cols
        assert!(result.contains("payments-api"), "basename must be preserved: {result:?}");
        assert!(
            UnicodeWidthStr::width(result.as_str()) <= 14,
            "result too wide: {result:?}"
        );
    }

    #[test]
    fn truncate_cwd_very_long_basename_truncated() {
        // Basename itself is longer than budget.
        let path = "/dev/a-very-long-basename-project";
        let result = truncate_cwd(path, 10);
        let w = UnicodeWidthStr::width(result.as_str());
        assert!(w <= 10, "result width {w} > 10: {result:?}");
        assert!(result.contains('…'));
    }

    #[test]
    fn truncate_cwd_zero_budget_is_empty() {
        let result = truncate_cwd("/dev/project", 0);
        assert!(result.is_empty());
    }

    // ── card_layout_v2 ───────────────────────────────────────────────────────

    fn make_v2_session(
        state: &str,
        activity: Option<&str>,
        last_activity_secs: u64,
        busy_secs: u64,
        origin: &str,
        stale: bool,
    ) -> crate::protocol::SessionView {
        crate::protocol::SessionView {
            id: "v2-test".to_string(),
            family: crate::protocol::AgentFamily::Claude,
            name: "payments-api".to_string(),
            cwd: "/home/dev/acme/payments-api".to_string(),
            state: state.to_string(),
            activity: activity.map(str::to_string),
            last_activity_secs,
            busy_secs,
            last_prompt: None,
            last_prompt_age_secs: None,
            host_app: Some("Cursor".to_string()),
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

    // ── height is always 4 ────────────────────────────────────────────────

    #[test]
    fn v2_height_always_4() {
        let s = make_v2_session("working", Some("Editing main.rs"), 10, 30, "local", false);
        for width in [30u16, 40, 60, 80, 100, 120] {
            let cl = card_layout_v2(width, &s);
            assert_eq!(cl.height, 4, "height should be 4 at width={width}");
        }
    }

    // ── width and inner_width geometry ────────────────────────────────────

    #[test]
    fn v2_inner_width_is_width_minus_2() {
        let s = make_v2_session("working", None, 5, 30, "local", false);
        for width in [40u16, 80, 100, 120] {
            let cl = card_layout_v2(width, &s);
            assert_eq!(cl.inner_width, width - 2, "inner_width at width={width}");
        }
    }

    // ── fill_d is at least 1 ─────────────────────────────────────────────

    #[test]
    fn v2_fill_d_at_least_1() {
        let s = make_v2_session("approval", Some("run: git push --force origin main"), 8, 300, "local", false);
        for width in [40u16, 60, 80, 100] {
            let cl = card_layout_v2(width, &s);
            assert!(cl.fill_d >= 1, "fill_d={} at width={width}", cl.fill_d);
        }
    }

    // ── time string: natural width (D absorbs changes) ───────────────────

    /// time_str must have natural width (no fixed padding).
    /// fill_d is at least 1 for all time values.
    #[test]
    fn v2_time_str_natural_fill_d_at_least_1() {
        for secs in [0u64, 2, 8, 12, 59, 60, 90, 3600, 7200] {
            let s = make_v2_session("working", None, secs, secs, "local", false);
            let cl = card_layout_v2(80, &s);
            assert!(cl.fill_d >= 1, "fill_d must be ≥1 for secs={secs}, got {}", cl.fill_d);
        }
    }

    /// 2s (2 cols) and 59s (3 cols) have different time_str widths;
    /// their fill_d differ by 1 (D absorbs the 1-col change).
    #[test]
    fn v2_fill_d_differs_by_1_between_2s_and_59s() {
        let s2 = make_v2_session("working", Some("Editing main.rs"), 2, 2, "local", false);
        let s59 = make_v2_session("working", Some("Editing main.rs"), 59, 59, "local", false);
        let cl2 = card_layout_v2(80, &s2);
        let cl59 = card_layout_v2(80, &s59);
        // "2s"=2 cols, "59s"=3 cols → fill_d(2s) = fill_d(59s) + 1
        assert_eq!(
            cl2.fill_d, cl59.fill_d + 1,
            "fill_d(2s)={} should be fill_d(59s)={} + 1 (D absorbs 1-col time change)",
            cl2.fill_d, cl59.fill_d
        );
    }

    /// 1m30s (5 cols) and 59m59s (6 cols) have different time widths;
    /// fill_d(1m30s) = fill_d(59m59s) + 1.
    #[test]
    fn v2_fill_d_differs_by_1_between_1m30s_and_59m59s() {
        let s_short = make_v2_session("working", Some("Editing"), 0, 90, "local", false);   // "1m30s" = 5 cols
        let s_long = make_v2_session("working", Some("Editing"), 0, 3599, "local", false);  // "59m59s" = 6 cols
        let cl_short = card_layout_v2(80, &s_short);
        let cl_long = card_layout_v2(80, &s_long);
        assert_eq!(
            cl_short.fill_d, cl_long.fill_d + 1,
            "fill_d(1m30s)={} should be fill_d(59m59s)={} + 1",
            cl_short.fill_d, cl_long.fill_d
        );
    }

    // ── time natural width (改动1): 去掉固定填充 ─────────────────────────

    /// time_str should have natural width — NOT padded to a fixed 6 cols.
    /// Short times like "36s" (3 cols) must stay 3 cols; fill_d absorbs the rest.
    #[test]
    fn v2_time_str_is_natural_width_not_padded() {
        // "36s" = 3 cols raw; must NOT be padded to ≥6 cols after the change.
        let s36 = make_v2_session("working", None, 36, 36, "local", false);
        let cl36 = card_layout_v2(80, &s36);
        let w36 = UnicodeWidthStr::width(cl36.time_str.as_str());
        assert_eq!(w36, 3, "time_str for 36s should be 3 cols (natural, no padding), got {w36}: {:?}", cl36.time_str);

        // "1m30s" = 5 cols raw; must stay 5 cols.
        let s1m30 = make_v2_session("working", None, 90, 90, "local", false);
        let cl1m30 = card_layout_v2(80, &s1m30);
        let w1m30 = UnicodeWidthStr::width(cl1m30.time_str.as_str());
        assert_eq!(w1m30, 5, "time_str for 1m30s should be 5 cols (natural), got {w1m30}: {:?}", cl1m30.time_str);
    }

    /// fill_d must be larger for shorter time strings (D absorbs the freed space).
    /// 36s (3 cols) vs 1m30s (5 cols): fill_d(36s) should be fill_d(1m30s) + 2.
    #[test]
    fn v2_fill_d_absorbs_time_width_change() {
        let s36 = make_v2_session("working", None, 36, 36, "local", false);
        let s1m30 = make_v2_session("working", None, 90, 90, "local", false);
        let cl36 = card_layout_v2(80, &s36);
        let cl1m30 = card_layout_v2(80, &s1m30);
        // 36s=3 cols, 1m30s=5 cols → 2 col difference → fill_d for 36s must be 2 more
        assert_eq!(
            cl36.fill_d, cl1m30.fill_d + 2,
            "fill_d(36s)={} should be fill_d(1m30s)={} + 2 (D absorbs time width change)",
            cl36.fill_d, cl1m30.fill_d
        );
    }

    /// Right border alignment invariant: icon+sp+sp+name+sp+D+sp+glyph+sp+word+3+time+sp
    /// must always equal inner_width regardless of time length.
    #[test]
    fn v2_top_border_total_always_equals_inner_width() {
        for busy_secs in [2u64, 36, 90, 600, 3600] {
            let s = make_v2_session("working", None, busy_secs, busy_secs, "local", false);
            let cl = card_layout_v2(80, &s);
            let icon_w = UnicodeWidthStr::width(cl.icon.as_str());
            let name_w = UnicodeWidthStr::width(cl.name.as_str());
            let glyph_w = UnicodeWidthStr::width(cl.state_glyph.as_str());
            let word_w = UnicodeWidthStr::width(cl.state_word.as_str());
            let time_w = UnicodeWidthStr::width(cl.time_str.as_str());
            // Layout: sp(1) + icon + sp(1) + name + sp(1) + fill_d + sp(1) + glyph + sp(1) + word + " · "(3) + time + sp(1)
            let total = 1 + icon_w + 1 + name_w + 1 + cl.fill_d + 1 + glyph_w + 1 + word_w + 3 + time_w + 1;
            assert_eq!(
                total, cl.inner_width as usize,
                "border total {total} != inner_width {} at busy_secs={busy_secs}",
                cl.inner_width
            );
        }
    }

    // ── icon spacing (改动2): icon 前有一个空格 ──────────────────────────

    /// After change 2, the icon must be at inner position 1 (x0+2),
    /// and there must be a space at inner position 0 (x0+1) before it.
    /// We verify via fill_d: adding 1 sp before icon increases overhead by 1,
    /// so fill_d(icon_sp) = fill_d_old - 1 compared to current (pre-change) layout.
    /// For now we just verify the total inner width identity holds with the new layout.
    #[test]
    fn v2_top_border_total_with_icon_space_equals_inner_width() {
        // Same as v2_top_border_total_always_equals_inner_width but with sp before icon
        // Layout: sp(1,before icon) + icon + sp(1) + name + sp(1) + fill_d + sp(1) + glyph + sp(1) + word + " · "(3) + time + sp(1)
        let s = make_v2_session("working", None, 36, 36, "local", false);
        let cl = card_layout_v2(80, &s);
        let icon_w = UnicodeWidthStr::width(cl.icon.as_str());
        let name_w = UnicodeWidthStr::width(cl.name.as_str());
        let glyph_w = UnicodeWidthStr::width(cl.state_glyph.as_str());
        let word_w = UnicodeWidthStr::width(cl.state_word.as_str());
        let time_w = UnicodeWidthStr::width(cl.time_str.as_str());
        // New layout includes extra leading space before icon
        let total = 1 + icon_w + 1 + name_w + 1 + cl.fill_d + 1 + glyph_w + 1 + word_w + 3 + time_w + 1;
        assert_eq!(
            total, cl.inner_width as usize,
            "with icon-space: border total {total} != inner_width {}", cl.inner_width
        );
    }

    // ── step slot mapping ─────────────────────────────────────────────────

    #[test]
    fn v2_step_working_uses_activity() {
        let s = make_v2_session("working", Some("Editing src/main.rs"), 5, 30, "local", false);
        let cl = card_layout_v2(80, &s);
        assert!(
            cl.step_text.contains("Editing"),
            "working state should use activity for step_text, got: {:?}",
            cl.step_text
        );
        assert_eq!(cl.step_prefix, "▸");
    }

    #[test]
    fn v2_step_approval_uses_activity() {
        let s = make_v2_session("approval", Some("run: git push --force"), 8, 300, "local", false);
        let cl = card_layout_v2(80, &s);
        assert!(
            cl.step_text.contains("git push"),
            "approval should use activity for step_text, got: {:?}",
            cl.step_text
        );
        assert_eq!(cl.step_prefix, "▸");
    }

    #[test]
    fn v2_step_idle_uses_last_prompt() {
        let mut s = make_v2_session("idle", None, 120, 0, "local", false);
        s.last_prompt = Some("Fix the auth bug".to_string());
        let cl = card_layout_v2(80, &s);
        assert!(
            cl.step_text.contains("Fix the auth bug"),
            "idle should show last_prompt in step_text, got: {:?}",
            cl.step_text
        );
        assert_eq!(cl.step_prefix, "▸");
    }

    #[test]
    fn v2_step_idle_without_prompt_falls_back_to_new_session() {
        // Fresh session_start: idle, no last_prompt and no activity yet.
        let s = make_v2_session("idle", None, 0, 0, "local", false);
        let cl = card_layout_v2(80, &s);
        assert!(
            cl.step_text.contains("New session"),
            "idle with no prompt should fall back to 'New session', got: {:?}",
            cl.step_text
        );
        assert_eq!(cl.step_prefix, "▸");
    }

    #[test]
    fn v2_step_done_uses_last_prompt() {
        let mut s = make_v2_session("done", None, 60, 120, "local", false);
        s.last_prompt = Some("Refactor auth module".to_string());
        let cl = card_layout_v2(80, &s);
        assert!(
            cl.step_text.contains("Refactor"),
            "done should show last_prompt in step_text, got: {:?}",
            cl.step_text
        );
        assert_eq!(cl.step_prefix, "▸");
    }

    #[test]
    fn v2_step_offline_uses_last_prompt_with_offline_prefix() {
        let mut s = make_v2_session("working", Some("Training epoch 4/10"), 120, 0, "gpu-rig", true);
        s.last_prompt = Some("Train model".to_string());
        let cl = card_layout_v2(80, &s);
        // Offline prefers last_prompt over activity.
        assert!(
            cl.step_text.contains("Train model"),
            "offline should prefer last_prompt for step_text, got: {:?}",
            cl.step_text
        );
        assert_eq!(cl.step_prefix, "⊘", "offline step prefix must be ⊘");
    }

    #[test]
    fn v2_step_offline_falls_back_to_activity() {
        let s = make_v2_session("working", Some("Training epoch 4/10"), 120, 0, "gpu-rig", true);
        // last_prompt is None (default from make_v2_session)
        let cl = card_layout_v2(80, &s);
        assert!(
            cl.step_text.contains("Training"),
            "offline without last_prompt should fall back to activity, got: {:?}",
            cl.step_text
        );
        assert_eq!(cl.step_prefix, "⊘");
    }

    // ── state_glyph ───────────────────────────────────────────────────────

    #[test]
    fn v2_state_glyph_approval() {
        let s = make_v2_session("approval", None, 0, 0, "local", false);
        let cl = card_layout_v2(80, &s);
        assert_eq!(cl.state_glyph, "▲");
        assert_eq!(cl.state_word, "Approval");
    }

    #[test]
    fn v2_state_glyph_question() {
        let s = make_v2_session("question", None, 0, 0, "local", false);
        let cl = card_layout_v2(80, &s);
        assert_eq!(cl.state_glyph, "?");
        assert_eq!(cl.state_word, "Question");
    }

    #[test]
    fn v2_state_glyph_working() {
        let s = make_v2_session("working", None, 0, 0, "local", false);
        let cl = card_layout_v2(80, &s);
        // Spinner placeholder — must not be empty.
        assert!(!cl.state_glyph.is_empty(), "working state_glyph must not be empty");
        assert_eq!(cl.state_word, "Working");
    }

    #[test]
    fn v2_state_glyph_done() {
        let s = make_v2_session("done", None, 0, 0, "local", false);
        let cl = card_layout_v2(80, &s);
        assert_eq!(cl.state_glyph, "✓");
        assert_eq!(cl.state_word, "Done");
    }

    #[test]
    fn v2_state_glyph_offline() {
        let s = make_v2_session("working", None, 120, 0, "gpu-rig", true);
        let cl = card_layout_v2(80, &s);
        assert_eq!(cl.state_glyph, "⊘");
        assert_eq!(cl.state_word, "offline");
        assert!(cl.is_offline);
    }

    // ── time source: online→busy_secs (frozen when done/idle), offline→last_activity_secs ──

    #[test]
    fn v2_time_stale_uses_last_activity_secs() {
        // Offline: last_activity_secs=120 (2m), busy_secs=30.
        // time_str must reflect last_activity_secs (last-seen), not busy_secs.
        let s = make_v2_session("working", None, 120, 30, "gpu-rig", true);
        let cl = card_layout_v2(80, &s);
        // relative_time(120) = "2m", right-padded to 6 → "2m    "
        let expected = crate::tui::anim::relative_time(120);
        assert!(
            cl.time_str.trim() == expected,
            "stale should show last_activity_secs={expected:?}, got time_str={:?}",
            cl.time_str
        );
    }

    #[test]
    fn v2_time_online_working_uses_busy_secs() {
        // Online working: time = busy_secs (active counter, growing while working).
        let s = make_v2_session("working", None, 5, 90, "local", false);
        let cl = card_layout_v2(80, &s);
        // relative_time(90) = "1m30s", right-padded to 6 → "1m30s "
        let expected = crate::tui::anim::relative_time(90);
        assert!(
            cl.time_str.trim() == expected,
            "online working should show busy_secs={expected:?}, got time_str={:?}",
            cl.time_str
        );
    }

    #[test]
    fn v2_time_idle_uses_busy_secs_frozen() {
        // Online idle: time = busy_secs (frozen — stopped accumulating when idle).
        // busy_secs=45 (frozen at last active time), last_activity_secs=300 (much larger).
        let s = make_v2_session("idle", None, 300, 45, "local", false);
        let cl = card_layout_v2(80, &s);
        // relative_time(45) = "45s", left-padded to 6 → "    45s"
        let expected = crate::tui::anim::relative_time(45);
        assert!(
            cl.time_str.trim() == expected,
            "idle should show busy_secs (frozen)={expected:?}, got time_str={:?}",
            cl.time_str
        );
        // Also confirm it does NOT show last_activity_secs (300s → "5m").
        let not_expected = crate::tui::anim::relative_time(300);
        assert!(
            cl.time_str.trim() != not_expected,
            "idle must NOT show last_activity_secs={not_expected:?}, but got {:?}",
            cl.time_str
        );
    }

    #[test]
    fn v2_time_done_uses_busy_secs_frozen() {
        // Online done: time = busy_secs (frozen at turn end).
        // busy_secs=120, last_activity_secs=5 (irrelevant).
        let s = make_v2_session("done", None, 5, 120, "local", false);
        let cl = card_layout_v2(80, &s);
        // relative_time(120) = "2m", right-padded to 6 → "2m    "
        let expected = crate::tui::anim::relative_time(120);
        assert!(
            cl.time_str.trim() == expected,
            "done should show busy_secs (frozen)={expected:?}, got time_str={:?}",
            cl.time_str
        );
    }

    #[test]
    fn v2_time_approval_uses_busy_secs() {
        // Online approval: time = busy_secs (still active).
        let s = make_v2_session("approval", None, 10, 3599, "local", false);
        let cl = card_layout_v2(80, &s);
        // relative_time(3599) = "59m59s"
        let expected = crate::tui::anim::relative_time(3599);
        assert!(
            cl.time_str.trim() == expected,
            "approval should show busy_secs={expected:?}, got time_str={:?}",
            cl.time_str
        );
    }

    // ── source_str: @local vs @device ────────────────────────────────────

    #[test]
    fn v2_source_local() {
        let s = make_v2_session("working", None, 5, 5, "local", false);
        let cl = card_layout_v2(80, &s);
        assert_eq!(cl.source_str, "@local");
    }

    #[test]
    fn v2_source_remote_device() {
        let s = make_v2_session("working", None, 5, 5, "desk-mini", false);
        let cl = card_layout_v2(80, &s);
        assert_eq!(cl.source_str, "@desk-mini");
    }

    // ── via_str ───────────────────────────────────────────────────────────

    #[test]
    fn v2_via_str_with_host_app() {
        let s = make_v2_session("working", None, 5, 5, "local", false);
        // host_app = Some("Cursor") from make_v2_session
        let cl = card_layout_v2(80, &s);
        assert_eq!(cl.via_str, " · via Cursor");
    }

    #[test]
    fn v2_via_str_without_host_app() {
        let mut s = make_v2_session("working", None, 5, 5, "local", false);
        s.host_app = None;
        let cl = card_layout_v2(80, &s);
        assert!(cl.via_str.is_empty(), "no host_app → via_str must be empty");
    }

    // ── cwd_display ───────────────────────────────────────────────────────

    #[test]
    fn v2_cwd_short_path_unchanged() {
        let mut s = make_v2_session("working", None, 5, 5, "local", false);
        s.cwd = "~/dev/myproject".to_string();
        let cl = card_layout_v2(100, &s);
        assert_eq!(cl.cwd_display, "~/dev/myproject");
    }

    #[test]
    fn v2_cwd_long_path_preserves_basename() {
        // Use a deliberately long cwd.
        let mut s = make_v2_session("working", None, 5, 5, "local", false);
        s.cwd = "/home/dev/very/deep/nested/project/payments-api".to_string();
        let cl = card_layout_v2(80, &s);
        assert!(
            cl.cwd_display.contains("payments-api"),
            "cwd basename must be preserved: {:?}",
            cl.cwd_display
        );
    }

    // ── CJK width correctness ─────────────────────────────────────────────

    #[test]
    fn v2_cjk_name_fits() {
        let mut s = make_v2_session("working", None, 5, 30, "local", false);
        s.name = "前端认证重构项目名称太长了".to_string();
        let cl = card_layout_v2(80, &s);
        let name_w = UnicodeWidthStr::width(cl.name.as_str());
        // inner_width - state_word - time(6) - 13 - 1 (D≥1) = budget
        let inner = (80u16 - 2) as usize;
        let sw = UnicodeWidthStr::width("Working");
        let budget = inner.saturating_sub(sw + 6 + 13 + 1).max(4);
        assert!(
            name_w <= budget,
            "CJK name width {name_w} > budget {budget}: {:?}",
            cl.name
        );
    }

    #[test]
    fn v2_cjk_step_text_fits() {
        let cjk = "正在编辑认证模块的全部代码逻辑重构功能实现路径解析器文件系统监视器";
        let s = make_v2_session("working", Some(cjk), 5, 30, "local", false);
        for width in [40u16, 60, 80, 100] {
            let cl = card_layout_v2(width, &s);
            let step_w = UnicodeWidthStr::width(cl.step_text.as_str());
            let inner = (width - 2) as usize;
            let budget = inner.saturating_sub(3);
            assert!(
                step_w <= budget,
                "CJK step_text width {step_w} > budget {budget} at width={width}"
            );
        }
    }

    // ── wide (100) and narrow (80) tiers preserve 4-row structure ─────────

    #[test]
    fn v2_wide_100_col_structure() {
        // Design spec: 100-col → cardW=96, inner=94.
        // We pass cardW (96) to card_layout_v2 since the outer 2-col indent
        // is handled by the list renderer.  Here test with width=96.
        let s = make_v2_session("working", Some("Editing src/components/Editor.tsx"), 3, 30, "local", false);
        let cl = card_layout_v2(96, &s);
        assert_eq!(cl.height, 4);
        assert_eq!(cl.inner_width, 94);
        assert!(cl.fill_d >= 1);
        // time_str is natural width (no fixed padding); at minimum "0s" = 2 cols.
        assert!(UnicodeWidthStr::width(cl.time_str.as_str()) >= 1);
    }

    #[test]
    fn v2_narrow_80_col_structure() {
        // Design spec: 80-col → cardW=76, inner=74.
        let s = make_v2_session("question", Some("choose: Postgres or SQLite?"), 60, 0, "desk-mini", false);
        let cl = card_layout_v2(76, &s);
        assert_eq!(cl.height, 4);
        assert_eq!(cl.inner_width, 74);
        assert!(cl.fill_d >= 1);
        // time_str is natural width (no fixed padding).
        assert!(UnicodeWidthStr::width(cl.time_str.as_str()) >= 1);
        assert_eq!(cl.source_str, "@desk-mini");
        assert!(cl.via_str.contains("Cursor"));
    }
}
