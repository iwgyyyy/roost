//! Animation state: spinner, breathing, heartbeat, relative-time formatting.
//!
//! All functions are pure (take `elapsed_secs: f64`), making them trivially testable
//! without faking a clock. The `Anim` struct holds the start `Instant` and hands
//! `elapsed()` to each pure helper on every tick.

use std::time::Instant;

// ── Spinner ───────────────────────────────────────────────────────────────────

/// Braille spinner frames (10 frames, ~90 ms/frame → full cycle ~0.9 s).
pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// ASCII fallback for terminals that don't render braille.
pub const SPINNER_FRAMES_ASCII: &[&str] = &["|", "/", "-", "\\"];

const SPINNER_FRAME_SECS: f64 = 0.090; // 90 ms per frame
const SPINNER_CYCLE: f64 = SPINNER_FRAME_SECS * SPINNER_FRAMES.len() as f64; // ~0.9 s

/// Return the braille spinner frame for the given elapsed seconds.
/// `elapsed` is seconds since the animation origin.
pub fn spinner_frame(elapsed: f64) -> &'static str {
    let idx = ((elapsed % SPINNER_CYCLE) / SPINNER_FRAME_SECS) as usize;
    let idx = idx.min(SPINNER_FRAMES.len() - 1);
    SPINNER_FRAMES[idx]
}

/// ASCII fallback spinner frame.
pub fn spinner_frame_ascii(elapsed: f64) -> &'static str {
    const CYCLE: f64 = 0.090 * SPINNER_FRAMES_ASCII.len() as f64;
    let idx = ((elapsed % CYCLE) / 0.090) as usize;
    let idx = idx.min(SPINNER_FRAMES_ASCII.len() - 1);
    SPINNER_FRAMES_ASCII[idx]
}

// ── Breathing (Approval) ──────────────────────────────────────────────────────

const BREATHE_PERIOD: f64 = 1.4; // 1.4 s ease-in-out cycle

/// Return a brightness value in [0.0, 1.0] for the Approval breathing animation.
/// Uses a triangle wave: linearly rises from 0→1 in the first half-period,
/// then linearly falls from 1→0 in the second half-period.
pub fn breathe(elapsed: f64) -> f32 {
    let phase = (elapsed % BREATHE_PERIOD) / BREATHE_PERIOD; // 0..1
    // Triangle wave: rising half [0, 0.5) → t = phase * 2;
    //                falling half [0.5, 1) → t = (1 - phase) * 2
    let t = if phase < 0.5 {
        phase * 2.0
    } else {
        (1.0 - phase) * 2.0
    };
    t as f32
}

// ── Heartbeat ────────────────────────────────────────────────────────────────

const HEARTBEAT_PERIOD: f64 = 1.6; // 1.6 s cycle
const HEARTBEAT_ON_FRACTION: f64 = 0.55; // "亮 ~55% 后闪灭"

/// Returns `true` when the header heartbeat dot should be lit.
pub fn heartbeat(elapsed: f64) -> bool {
    let phase = (elapsed % HEARTBEAT_PERIOD) / HEARTBEAT_PERIOD; // 0..1
    phase < HEARTBEAT_ON_FRACTION
}

// ── Relative time ─────────────────────────────────────────────────────────────

/// Format a duration in seconds as a compact, full-resolution string.
///
/// Tiers (largest unit first; trailing zero components are trimmed):
/// - `< 1m`  → `38s`
/// - `< 1h`  → `3m39s`
/// - `< 1d`  → `1h34m34s`
/// - `>= 1d` → `3d3h23m` (seconds dropped at the day tier; days never roll up
///   into weeks/months, so three months reads as `90d`).
pub fn relative_time(secs: u64) -> String {
    const MIN: u64 = 60;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;

    // Component breakdown for the chosen tier, largest unit first.
    let parts: Vec<(u64, char)> = if secs < MIN {
        vec![(secs, 's')]
    } else if secs < HOUR {
        vec![(secs / MIN, 'm'), (secs % MIN, 's')]
    } else if secs < DAY {
        vec![
            (secs / HOUR, 'h'),
            (secs % HOUR / MIN, 'm'),
            (secs % MIN, 's'),
        ]
    } else {
        // Day tier: drop seconds; keep counting in days (no weeks/months).
        vec![
            (secs / DAY, 'd'),
            (secs % DAY / HOUR, 'h'),
            (secs % HOUR / MIN, 'm'),
        ]
    };

    // Trim trailing zero components, but always keep the largest unit.
    let mut end = parts.len();
    while end > 1 && parts[end - 1].0 == 0 {
        end -= 1;
    }
    parts[..end]
        .iter()
        .map(|(v, u)| format!("{v}{u}"))
        .collect()
}

// ── Anim aggregator ───────────────────────────────────────────────────────────

/// Holds the origin `Instant` and exposes per-tick accessors.
#[derive(Debug)]
pub struct Anim {
    start: Instant,
}

impl Anim {
    pub fn new() -> Self {
        Anim {
            start: Instant::now(),
        }
    }

    fn elapsed(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Current braille spinner frame.
    pub fn spinner(&self) -> &'static str {
        spinner_frame(self.elapsed())
    }

    /// Brightness [0,1] for Approval breathing.
    pub fn breathe(&self) -> f32 {
        breathe(self.elapsed())
    }

    /// Whether the header heartbeat dot is lit.
    pub fn heartbeat(&self) -> bool {
        heartbeat(self.elapsed())
    }
}

impl Default for Anim {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── relative_time ──────────────────────────────────────────────────────

    #[test]
    fn relative_time_zero() {
        assert_eq!(relative_time(0), "0s");
    }

    #[test]
    fn relative_time_seconds() {
        assert_eq!(relative_time(1), "1s");
        assert_eq!(relative_time(59), "59s");
    }

    #[test]
    fn relative_time_minutes_show_seconds() {
        assert_eq!(relative_time(60), "1m"); // 1m0s → trailing 0s trimmed
        assert_eq!(relative_time(90), "1m30s");
        assert_eq!(relative_time(219), "3m39s");
        assert_eq!(relative_time(3599), "59m59s");
    }

    #[test]
    fn relative_time_hours_show_h_m_s() {
        assert_eq!(relative_time(3600), "1h"); // 1h0m0s → trimmed
        assert_eq!(relative_time(7200), "2h");
        assert_eq!(relative_time(3601), "1h0m1s"); // middle zero kept
        assert_eq!(relative_time(5674), "1h34m34s");
    }

    #[test]
    fn relative_time_days_drop_seconds() {
        assert_eq!(relative_time(86_400), "1d"); // exactly 1 day
        assert_eq!(relative_time(271_380), "3d3h23m");
        // Three months keeps counting in days, no seconds at the day tier.
        assert_eq!(relative_time(90 * 86_400), "90d");
        assert_eq!(relative_time(90 * 86_400 + 3 * 3600 + 23 * 60), "90d3h23m");
    }

    // ── spinner_frame ──────────────────────────────────────────────────────

    #[test]
    fn spinner_frame_at_zero_is_first() {
        assert_eq!(spinner_frame(0.0), SPINNER_FRAMES[0]);
    }

    #[test]
    fn spinner_frame_advances_every_90ms() {
        for (i, expected) in SPINNER_FRAMES.iter().enumerate() {
            let t = i as f64 * 0.090 + 0.001; // slightly past boundary
            assert_eq!(spinner_frame(t), *expected, "frame {i} at t={t:.3}");
        }
    }

    #[test]
    fn spinner_frame_wraps_at_cycle_end() {
        let one_cycle = SPINNER_FRAME_SECS * SPINNER_FRAMES.len() as f64;
        assert_eq!(spinner_frame(one_cycle), spinner_frame(0.0));
    }

    #[test]
    fn spinner_frame_returns_valid_braille() {
        for i in 0..SPINNER_FRAMES.len() {
            let t = i as f64 * SPINNER_FRAME_SECS + 0.001;
            let f = spinner_frame(t);
            // All braille frames are single Unicode scalar values in ⠀–⣿
            let ch = f.chars().next().unwrap();
            assert!(
                ('\u{2800}'..='\u{28FF}').contains(&ch),
                "expected braille char, got {ch:?}"
            );
        }
    }

    // ── breathe ───────────────────────────────────────────────────────────

    #[test]
    fn breathe_at_zero_is_near_zero() {
        // At t=0, cos(0)=1 → (1-1)/2 = 0
        let b = breathe(0.0);
        assert!(b.abs() < 0.01, "expected ~0, got {b}");
    }

    #[test]
    fn breathe_at_half_period_is_near_one() {
        // At t=half period, cos(π)=−1 → (1−(−1))/2 = 1
        let b = breathe(BREATHE_PERIOD / 2.0);
        assert!((b - 1.0).abs() < 0.01, "expected ~1, got {b}");
    }

    #[test]
    fn breathe_is_in_range() {
        for i in 0..100 {
            let t = i as f64 * BREATHE_PERIOD / 100.0;
            let b = breathe(t);
            assert!((0.0..=1.0).contains(&b), "breathe({t}) = {b} out of range");
        }
    }

    /// Triangle wave: rising half is perfectly linear (0→1 from t=0 to t=T/2).
    #[test]
    fn breathe_triangle_wave_rising_half_is_linear() {
        // At 25% of period, triangle wave should be exactly 0.5.
        let t25 = BREATHE_PERIOD * 0.25;
        let b = breathe(t25);
        assert!(
            (b - 0.5).abs() < 0.01,
            "at 25% period, triangle wave should be ~0.5, got {b}"
        );
    }

    /// Triangle wave: falling half is perfectly linear (1→0 from t=T/2 to t=T).
    #[test]
    fn breathe_triangle_wave_falling_half_is_linear() {
        // At 75% of period, triangle wave should be exactly 0.5.
        let t75 = BREATHE_PERIOD * 0.75;
        let b = breathe(t75);
        assert!(
            (b - 0.5).abs() < 0.01,
            "at 75% period, triangle wave should be ~0.5, got {b}"
        );
    }

    /// Triangle wave: midpoint of rising half (12.5%) should be 0.25.
    #[test]
    fn breathe_triangle_wave_quarter_point_is_0_25() {
        let t125 = BREATHE_PERIOD * 0.125;
        let b = breathe(t125);
        assert!(
            (b - 0.25).abs() < 0.01,
            "at 12.5% period, triangle wave should be ~0.25, got {b}"
        );
    }

    /// Triangle wave: wraps cleanly — at period start and period end both = 0.
    #[test]
    fn breathe_triangle_wave_wraps_to_zero() {
        let b_start = breathe(0.0);
        let b_end = breathe(BREATHE_PERIOD);
        assert!(b_start.abs() < 0.01, "triangle start should be ~0, got {b_start}");
        assert!(b_end.abs() < 0.01, "triangle end should be ~0, got {b_end}");
    }

    // ── heartbeat ────────────────────────────────────────────────────────

    #[test]
    fn heartbeat_on_at_start() {
        assert!(heartbeat(0.0));
    }

    #[test]
    fn heartbeat_off_after_55_pct() {
        // at 56% of period it should be off
        let t = HEARTBEAT_PERIOD * 0.56;
        assert!(!heartbeat(t));
    }

    #[test]
    fn heartbeat_wraps() {
        // After one full period it should behave like t=0
        assert_eq!(heartbeat(HEARTBEAT_PERIOD), heartbeat(0.0));
    }

    // ── Anim struct ───────────────────────────────────────────────────────

    #[test]
    fn anim_new_does_not_panic() {
        let a = Anim::new();
        let _ = a.spinner();
        let _ = a.breathe();
        let _ = a.heartbeat();
    }
}
