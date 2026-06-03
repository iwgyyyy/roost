//! Terminal background luminance detection via OSC 11.
//!
//! OSC 11 queries the terminal for its default background colour.  Not all
//! terminals implement the protocol; we add a short timeout so callers that
//! run in terminals without support degrade cleanly to "assume dark background".
//!
//! Public surface:
//!   - [`parse_osc11_response`]   — pure fn: bytes → Option<(u16,u16,u16)>
//!   - [`relative_luminance`]     — pure fn: (r16, g16, b16) → f64 in [0,1]
//!   - [`is_light_from_osc11`]    — pure fn: raw OSC response bytes → bool
//!   - [`border_color_for_bg`]    — pure fn: is_light → ratatui Color
//!   - [`detect_light_background`] — I/O fn (only called in real TUI startup)

use ratatui::style::Color;

// ── Light-background border colour ───────────────────────────────────────────

/// Border colour to use on a light terminal background.
///
/// Chosen to be clearly visible on white/near-white backgrounds while being
/// noticeably lighter than `DarkGray` (which is too dark / harsh on light bg).
pub const COLOR_BORDER_LIGHT: Color = Color::Rgb(0xc4, 0xc4, 0xc4);

/// Border colour to use on a dark terminal background (status quo).
pub const COLOR_BORDER_DARK: Color = Color::DarkGray;

// ── Luminance threshold ───────────────────────────────────────────────────────

/// Relative luminance above which the background is considered "light".
///
/// Standard mid-point: 0.5 on the linear [0,1] scale.  Most "white" or
/// light-cream terminal backgrounds sit at 0.9+; "dark" themes sit below 0.15.
const LUMINANCE_LIGHT_THRESHOLD: f64 = 0.5;

// ── Pure parsing / logic functions ────────────────────────────────────────────

/// Parse the RGB triplet from an OSC 11 response string.
///
/// Expected formats (both ST variants):
///   `\x1b]11;rgb:RRRR/GGGG/BBBB\x07`
///   `\x1b]11;rgb:RRRR/GGGG/BBBB\x1b\\`
///
/// Each channel is 4 hex digits (0000–ffff).  Returns the three raw u16 values
/// on success, `None` on any parse failure (malformed, truncated, wrong format).
pub fn parse_osc11_response(raw: &[u8]) -> Option<(u16, u16, u16)> {
    // Work with a str slice for ease of parsing.
    let s = std::str::from_utf8(raw).ok()?;

    // Locate "rgb:" marker.
    let rgb_pos = s.find("rgb:")?;
    let after_rgb = &s[rgb_pos + 4..];

    // Strip trailing ST: either BEL (\x07) or ESC \ (\x1b\\).
    // We just find the next non-hex-slash character.
    // Format: RRRR/GGGG/BBBB — 4 hex digits, slash separator.
    let parts: Vec<&str> = after_rgb.splitn(4, '/').collect();
    if parts.len() < 3 {
        return None;
    }

    // Each part starts with 4 hex digits; ignore any trailing garbage (ST chars).
    let parse_channel = |s: &str| -> Option<u16> {
        let hex = s.get(..4)?; // take exactly 4 chars
        u16::from_str_radix(hex, 16).ok()
    };

    let r = parse_channel(parts[0])?;
    let g = parse_channel(parts[1])?;
    let b = parse_channel(parts[2])?;

    Some((r, g, b))
}

/// Compute relative luminance (sRGB) from 16-bit channel values.
///
/// Formula: linearise each channel (sRGB gamma), then apply the standard
/// luminance weights Y = 0.2126 R + 0.7152 G + 0.0722 B.
///
/// Input: 0–65535 per channel (16-bit as returned by OSC 11).
/// Output: [0.0, 1.0].
pub fn relative_luminance(r16: u16, g16: u16, b16: u16) -> f64 {
    let linearise = |c16: u16| -> f64 {
        let c = c16 as f64 / 65535.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055_f64).powf(2.4)
        }
    };

    let r = linearise(r16);
    let g = linearise(g16);
    let b = linearise(b16);

    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Return `true` if the raw OSC 11 response bytes indicate a light background.
///
/// On any parse failure returns `false` (treat as dark — safe default).
pub fn is_light_from_osc11(raw: &[u8]) -> bool {
    match parse_osc11_response(raw) {
        Some((r, g, b)) => relative_luminance(r, g, b) > LUMINANCE_LIGHT_THRESHOLD,
        None => false,
    }
}

/// Choose the appropriate dim border colour based on whether the background is light.
///
/// - `is_light = true`  → [`COLOR_BORDER_LIGHT`] (a lighter grey, readable on white bg)
/// - `is_light = false` → [`COLOR_BORDER_DARK`]  (`DarkGray`, status quo for dark bg)
pub fn border_color_for_bg(is_light: bool) -> Color {
    if is_light {
        COLOR_BORDER_LIGHT
    } else {
        COLOR_BORDER_DARK
    }
}

// ── I/O: real OSC 11 query (only used in TUI startup) ─────────────────────────

/// Query the terminal for its background colour using OSC 11.
///
/// Writes the query sequence to stdout, then reads stdin in raw mode waiting
/// for the response.  A 100 ms timeout guards against terminals that do not
/// implement the protocol.
///
/// **Must be called after `enable_raw_mode()` and before the alt-screen
/// clears the display**, so that any stray bytes do not corrupt the TUI.
///
/// Returns `true` if the background appears light, `false` otherwise
/// (including on timeout, I/O error, or parse failure).
pub fn detect_light_background() -> bool {
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};

    // OSC 11 query: ESC ] 11 ; ? BEL
    let query = b"\x1b]11;?\x07";

    // Write query to stdout.
    if std::io::stdout().write_all(query).is_err() {
        return false;
    }
    if std::io::stdout().flush().is_err() {
        return false;
    }

    // Read response from stdin with a 100 ms timeout.
    // We read byte-by-byte until we see the ST terminator or time out.
    let deadline = Instant::now() + Duration::from_millis(100);
    let mut buf = Vec::with_capacity(64);

    let mut stdin = std::io::stdin();
    let stdin_fd = {
        use std::os::unix::io::AsRawFd;
        stdin.as_raw_fd()
    };

    // Use `select`-style polling so we can respect the deadline.
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {

        // Poll stdin for readability with the remaining timeout.
        let ready = unsafe {
            let mut fds: libc::fd_set = std::mem::zeroed();
            libc::FD_SET(stdin_fd, &mut fds);
            let mut tv = libc::timeval {
                tv_sec: remaining.as_secs() as libc::time_t,
                tv_usec: remaining.subsec_micros() as libc::suseconds_t,
            };
            libc::select(
                stdin_fd + 1,
                &mut fds,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut tv,
            )
        };

        if ready <= 0 {
            break; // timeout or error
        }

        // Read one byte.
        let mut byte = [0u8; 1];
        match stdin.read(&mut byte) {
            Ok(1) => {
                buf.push(byte[0]);
                // Check for terminator: BEL (\x07) or ESC-backslash (\x1b\\).
                if byte[0] == 0x07 {
                    break; // BEL terminator
                }
                if buf.len() >= 2 && buf[buf.len() - 2] == 0x1b && buf[buf.len() - 1] == b'\\' {
                    break; // ESC \ terminator
                }
                // Safety cap: if response is unexpectedly long, bail out.
                if buf.len() > 128 {
                    break;
                }
            }
            _ => break,
        }
    }

    is_light_from_osc11(&buf)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    // ── parse_osc11_response ─────────────────────────────────────────────

    #[test]
    fn parse_white_background() {
        // Pure white: ffff/ffff/ffff — BEL terminated
        let raw = b"\x1b]11;rgb:ffff/ffff/ffff\x07";
        let result = parse_osc11_response(raw);
        assert_eq!(result, Some((0xffff, 0xffff, 0xffff)));
    }

    #[test]
    fn parse_black_background() {
        // Pure black: 0000/0000/0000
        let raw = b"\x1b]11;rgb:0000/0000/0000\x07";
        let result = parse_osc11_response(raw);
        assert_eq!(result, Some((0x0000, 0x0000, 0x0000)));
    }

    #[test]
    fn parse_dark_gray_background() {
        // #1c1c1c → 1c1c/1c1c/1c1c (typical dark terminal)
        let raw = b"\x1b]11;rgb:1c1c/1c1c/1c1c\x07";
        let result = parse_osc11_response(raw);
        assert_eq!(result, Some((0x1c1c, 0x1c1c, 0x1c1c)));
    }

    #[test]
    fn parse_esc_backslash_terminator() {
        // ESC \ (ST) terminator variant
        let raw = b"\x1b]11;rgb:eeee/eeee/eeee\x1b\\";
        let result = parse_osc11_response(raw);
        assert_eq!(result, Some((0xeeee, 0xeeee, 0xeeee)));
    }

    #[test]
    fn parse_malformed_returns_none() {
        assert_eq!(parse_osc11_response(b"garbage"), None);
        assert_eq!(parse_osc11_response(b"\x1b]11;rgb:zzzz/ffff/ffff\x07"), None);
        assert_eq!(parse_osc11_response(b"\x1b]11;rgb:ffff/ffff\x07"), None); // only 2 channels
        assert_eq!(parse_osc11_response(b""), None);
        assert_eq!(parse_osc11_response(b"\x1b]11;rgb:\x07"), None); // empty channel
    }

    // ── relative_luminance ────────────────────────────────────────────────

    #[test]
    fn luminance_of_white_is_one() {
        let l = relative_luminance(0xffff, 0xffff, 0xffff);
        // Should be very close to 1.0
        assert!((l - 1.0).abs() < 1e-6, "expected ~1.0 got {l}");
    }

    #[test]
    fn luminance_of_black_is_zero() {
        let l = relative_luminance(0x0000, 0x0000, 0x0000);
        assert!(l < 1e-9, "expected ~0.0 got {l}");
    }

    #[test]
    fn luminance_of_dark_gray_1c1c_is_low() {
        // #1c1c1c: each channel = 0x1c1c / 0xffff ≈ 0.109
        // sRGB linearised ≈ 0.109/12.92 ≈ 0.0084 → Y ≈ 0.0084
        let l = relative_luminance(0x1c1c, 0x1c1c, 0x1c1c);
        assert!(l < 0.5, "dark gray should have luminance < 0.5, got {l}");
    }

    #[test]
    fn luminance_of_mid_gray_is_near_half() {
        // Perceptual mid-gray #777777 → linear ≈ 0.2158 → Y ≈ 0.216
        // (still below 0.5 because sRGB gamma makes mid-gray map to ~0.22)
        let l = relative_luminance(0x7777, 0x7777, 0x7777);
        assert!(l > 0.0 && l < 1.0, "mid gray luminance should be in (0,1), got {l}");
    }

    // ── is_light_from_osc11 ───────────────────────────────────────────────

    #[test]
    fn white_bg_is_light() {
        let raw = b"\x1b]11;rgb:ffff/ffff/ffff\x07";
        assert!(is_light_from_osc11(raw), "white bg should be detected as light");
    }

    #[test]
    fn black_bg_is_dark() {
        let raw = b"\x1b]11;rgb:0000/0000/0000\x07";
        assert!(!is_light_from_osc11(raw), "black bg should be detected as dark");
    }

    #[test]
    fn dark_1c1c_bg_is_dark() {
        let raw = b"\x1b]11;rgb:1c1c/1c1c/1c1c\x07";
        assert!(!is_light_from_osc11(raw), "#1c1c1c bg should be detected as dark");
    }

    #[test]
    fn malformed_response_falls_back_to_dark() {
        assert!(!is_light_from_osc11(b"garbage"), "malformed OSC should fall back to dark");
        assert!(!is_light_from_osc11(b""), "empty response should fall back to dark");
    }

    // ── border_color_for_bg ───────────────────────────────────────────────

    #[test]
    fn light_bg_gives_lighter_border() {
        let c = border_color_for_bg(true);
        assert_eq!(c, COLOR_BORDER_LIGHT, "light bg should give lighter border colour");
        // Verify it is an RGB value (not DarkGray)
        assert!(
            matches!(c, Color::Rgb(_, _, _)),
            "light border should be an RGB colour"
        );
    }

    #[test]
    fn dark_bg_gives_dark_gray_border() {
        let c = border_color_for_bg(false);
        assert_eq!(c, COLOR_BORDER_DARK, "dark bg should give DarkGray border");
        assert_eq!(c, Color::DarkGray);
    }

    #[test]
    fn light_border_is_visually_lighter_than_dark_gray() {
        // Extract the RGB values from the light border and confirm each channel
        // is >= 0xb0 (perceptibly light on white background).
        let Color::Rgb(r, g, b) = COLOR_BORDER_LIGHT else {
            panic!("COLOR_BORDER_LIGHT should be Rgb");
        };
        assert!(r >= 0xb0, "light border R channel should be ≥ 0xb0, got {r:#x}");
        assert!(g >= 0xb0, "light border G channel should be ≥ 0xb0, got {g:#x}");
        assert!(b >= 0xb0, "light border B channel should be ≥ 0xb0, got {b:#x}");
    }
}
