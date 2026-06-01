//! Visual tokens: colours, icons, glyphs — spec §10.
//!
//! Hybrid colour strategy:
//!   - Semantic / state colours are fixed RGB (always readable, colour-blind safe).
//!   - Structural / chrome colours follow the terminal theme:
//!     * `Color::Reset`    — default terminal foreground (works on light & dark).
//!     * `Color::DarkGray` — ANSI dim grey for borders, timestamps, secondary text.
//!     * no background    — respects the terminal background.

use ratatui::style::Color;

// ── State colours ────────────────────────────────────────────────────────────

pub const COLOR_APPROVAL: Color = Color::Rgb(0xe3, 0xb3, 0x41);
pub const COLOR_APPROVAL_DIM: Color = Color::Rgb(0x8a, 0x6a, 0x1f);
pub const COLOR_QUESTION: Color = Color::Rgb(0x58, 0xc4, 0xd6);
pub const COLOR_WORKING: Color = Color::Rgb(0x6f, 0xd2, 0x83);
pub const COLOR_DONE: Color = Color::Rgb(0x7f, 0x9e, 0x84);
pub const COLOR_IDLE: Color = Color::Rgb(0x76, 0x83, 0x90);

// ── Family colours ───────────────────────────────────────────────────────────

pub const COLOR_CLAUDE: Color = Color::Rgb(0xd9, 0x77, 0x57);
pub const COLOR_CODEX: Color = Color::Rgb(0xc3, 0xcc, 0xd6);
pub const COLOR_UNKNOWN: Color = Color::Rgb(0x8b, 0x94, 0x9e);

// ── Accent (selection bar ▌) ─────────────────────────────────────────────────
pub const COLOR_ACCENT: Color = Color::Rgb(0xd9, 0x77, 0x57);

// ── Classic theme: chrome (adaptive — follow terminal theme) ──────────────────
//
// These are *not* fixed RGB values any more.  They resolve to terminal colours
// so roost looks correct on both dark and light backgrounds.
pub const COLOR_BG: Color = Color::Reset; // no background fill
pub const COLOR_BORDER: Color = Color::DarkGray; // box-drawing chars
pub const COLOR_FG: Color = Color::Reset; // default terminal foreground
pub const COLOR_FG_BRIGHT: Color = Color::Reset; // bold modifier applied at usage site
pub const COLOR_DIM: Color = Color::DarkGray; // secondary / muted text
pub const COLOR_MID: Color = Color::DarkGray; // tertiary — idle/done activity text

// ── Family icons ─────────────────────────────────────────────────────────────

/// Unicode glyph + foreground colour for a given agent family.
///
/// The glyph is delegated to `AgentFamily::icon()`; this function adds the
/// colour, which belongs in the theme layer.
pub fn family_icon(family: &crate::protocol::AgentFamily) -> (&'static str, Color) {
    use crate::protocol::AgentFamily;
    let color = match family {
        AgentFamily::Claude => COLOR_CLAUDE,
        AgentFamily::Codex => COLOR_CODEX,
        AgentFamily::Unknown => COLOR_UNKNOWN,
    };
    (family.icon(), color)
}

// ── State glyphs ─────────────────────────────────────────────────────────────

/// Static glyph for a state (Working uses a placeholder; live spinner comes from `anim`).
pub fn state_glyph(state: &str) -> (&'static str, Color) {
    match state {
        "approval" => ("▲", COLOR_APPROVAL),
        "question" => ("?", COLOR_QUESTION),
        "working" => ("⠋", COLOR_WORKING), // first spinner frame as placeholder
        "done" => ("✓", COLOR_DONE),
        _ => ("○", COLOR_IDLE), // idle or unknown
    }
}

/// Colour for a state string.
pub fn state_color(state: &str) -> Color {
    match state {
        "approval" => COLOR_APPROVAL,
        "question" => COLOR_QUESTION,
        "working" => COLOR_WORKING,
        "done" => COLOR_DONE,
        _ => COLOR_IDLE,
    }
}

/// Interpolate between dim and bright approval colour for the breathing animation.
/// `t` is in [0.0, 1.0]: 0 = dim, 1 = bright.
pub fn approval_breathe_color(t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    // Linear interpolation between DIM and APPROVAL colours
    let r = lerp(0x8a, 0xe3, t);
    let g = lerp(0x6a, 0xb3, t);
    let b = lerp(0x1f, 0x41, t);
    Color::Rgb(r, g, b)
}

fn lerp(lo: u8, hi: u8, t: f32) -> u8 {
    (lo as f32 + (hi as f32 - lo as f32) * t).round() as u8
}

// ── Theme struct ──────────────────────────────────────────────────────────────

/// A bundle of colours for a theme. Currently only `classic` is active.
#[derive(Debug, Clone)]
pub struct Theme {
    pub bg: Color,
    pub border: Color,
    pub fg: Color,
    pub fg_bright: Color,
    pub dim: Color,
    pub mid: Color,
}

impl Theme {
    /// Adaptive "classic" theme.
    ///
    /// Structural fields use terminal-relative colours (`Reset` / `DarkGray`).
    /// Semantic state/accent RGB constants are defined separately and never
    /// routed through the Theme struct.
    pub fn classic() -> Self {
        Theme {
            bg: Color::Reset,
            border: Color::DarkGray,
            fg: Color::Reset,
            fg_bright: Color::Reset, // callers add Modifier::BOLD where needed
            dim: Color::DarkGray,
            mid: Color::DarkGray,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AgentFamily;

    #[test]
    fn family_icon_claude() {
        let (glyph, color) = family_icon(&AgentFamily::Claude);
        assert_eq!(glyph, "✻");
        assert_eq!(color, COLOR_CLAUDE);
        // Verify colour value
        assert_eq!(color, Color::Rgb(0xd9, 0x77, 0x57));
    }

    #[test]
    fn family_icon_codex() {
        let (glyph, color) = family_icon(&AgentFamily::Codex);
        assert_eq!(glyph, "⬡");
        assert_eq!(color, COLOR_CODEX);
        assert_eq!(color, Color::Rgb(0xc3, 0xcc, 0xd6));
    }

    #[test]
    fn family_icon_unknown() {
        let (glyph, color) = family_icon(&AgentFamily::Unknown);
        assert_eq!(glyph, "◆");
        assert_eq!(color, COLOR_UNKNOWN);
    }

    #[test]
    fn state_glyph_approval() {
        let (g, c) = state_glyph("approval");
        assert_eq!(g, "▲");
        assert_eq!(c, COLOR_APPROVAL);
        assert_eq!(c, Color::Rgb(0xe3, 0xb3, 0x41));
    }

    #[test]
    fn state_glyph_question() {
        let (g, c) = state_glyph("question");
        assert_eq!(g, "?");
        assert_eq!(c, COLOR_QUESTION);
        assert_eq!(c, Color::Rgb(0x58, 0xc4, 0xd6));
    }

    #[test]
    fn state_glyph_working() {
        let (g, c) = state_glyph("working");
        // The placeholder glyph; live spinner is from anim
        assert!(!g.is_empty());
        assert_eq!(c, COLOR_WORKING);
        assert_eq!(c, Color::Rgb(0x6f, 0xd2, 0x83));
    }

    #[test]
    fn state_glyph_done() {
        let (g, c) = state_glyph("done");
        assert_eq!(g, "✓");
        assert_eq!(c, COLOR_DONE);
    }

    #[test]
    fn state_glyph_idle() {
        let (g, c) = state_glyph("idle");
        assert_eq!(g, "○");
        assert_eq!(c, COLOR_IDLE);
    }

    #[test]
    fn state_color_matches_glyph_color() {
        for state in ["approval", "question", "working", "done", "idle"] {
            let (_, glyph_c) = state_glyph(state);
            let c = state_color(state);
            assert_eq!(c, glyph_c, "state_color mismatch for {state}");
        }
    }

    #[test]
    fn approval_breathe_color_at_zero_is_dim() {
        let c = approval_breathe_color(0.0);
        assert_eq!(c, Color::Rgb(0x8a, 0x6a, 0x1f));
    }

    #[test]
    fn approval_breathe_color_at_one_is_bright() {
        let c = approval_breathe_color(1.0);
        assert_eq!(c, COLOR_APPROVAL);
    }

    #[test]
    fn approval_breathe_color_clamps() {
        let c_neg = approval_breathe_color(-0.5);
        let c_pos = approval_breathe_color(1.5);
        assert_eq!(c_neg, approval_breathe_color(0.0));
        assert_eq!(c_pos, approval_breathe_color(1.0));
    }

    #[test]
    fn theme_classic_structural_colours_are_adaptive() {
        // Structural fields must be terminal-adaptive (no fixed RGB).
        let t = Theme::classic();
        assert_eq!(
            t.bg,
            Color::Reset,
            "bg should be Reset (no background fill)"
        );
        assert_eq!(t.border, Color::DarkGray, "border should be DarkGray");
        assert_eq!(t.fg, Color::Reset, "fg should be Reset (terminal default)");
        assert_eq!(t.dim, Color::DarkGray, "dim should be DarkGray");
        assert_eq!(t.mid, Color::DarkGray, "mid should be DarkGray");
    }
}
