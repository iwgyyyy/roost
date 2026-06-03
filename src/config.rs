//! Notification configuration: reads/writes `~/.roost/settings.json`.
//!
//! On any error (missing file, corrupt JSON, missing fields) `load()` falls
//! back to the all-enabled default — notifications are fail-open by design.

use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use ratatui::style::Color;
use serde::{Deserialize, Serialize};

use crate::history::roost_home;
use crate::session::AgentState;

// ── Accent default ────────────────────────────────────────────────────────────

/// Default accent colour as a hex string — the canonical orange.
const DEFAULT_ACCENT_HEX: &str = "#d97757";

/// Return the default accent hex string. Exposed for use in struct literals elsewhere.
pub fn default_accent_hex() -> &'static str {
    DEFAULT_ACCENT_HEX
}

fn default_accent() -> String {
    DEFAULT_ACCENT_HEX.to_owned()
}

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// Per-stage notification channel flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage {
    #[serde(default = "default_true")]
    pub banner: bool,
    #[serde(default = "default_true")]
    pub sound: bool,
}

impl Default for Stage {
    fn default() -> Self {
        Stage {
            banner: true,
            sound: true,
        }
    }
}

/// Notification settings, one `Stage` per observable agent state.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NotifyConfig {
    #[serde(default)]
    pub clarify: Stage,
    #[serde(default)]
    pub approve: Stage,
    #[serde(default)]
    pub done: Stage,
    /// Whether to fire a notification when a remote tunnel goes `Down`.
    ///
    /// Defaults to `false` (opt-in). Unlike the per-stage toggles which are
    /// fail-open (default true), remote offline alerts are off by default to
    /// avoid noise for users who have not set up fleet monitoring.
    #[serde(default)]
    pub remote_offline: bool,
}

/// Top-level config persisted to `settings.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub notify: NotifyConfig,
    /// Theme accent colour as a hex string (e.g. `"#d97757"`).
    ///
    /// Used for the `roost` title and the selected-card border highlight.
    /// Edit `~/.roost/settings.json` directly to change it.
    #[serde(default = "default_accent")]
    pub accent: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            notify: NotifyConfig::default(),
            accent: default_accent(),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Serde default helper — returns `true`.
fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// I/O
// ---------------------------------------------------------------------------

/// Path to the roost settings file (`~/.roost/settings.json`).
pub fn settings_path() -> PathBuf {
    roost_home().join("settings.json")
}

/// Load config from disk.  Any I/O or parse error returns [`Config::default()`].
///
/// Never panics, never returns `Err`.
pub fn load() -> Config {
    load_from(&settings_path())
}

/// Persist `cfg` to [`settings_path()`] as pretty-printed JSON.
///
/// Creates the parent directory if it does not exist.
pub fn save(cfg: &Config) -> Result<()> {
    save_to(&settings_path(), cfg)
}

/// Internal: write `cfg` to an explicit `path`.  Used by tests to avoid
/// touching the real `~/.roost/settings.json`.
fn save_to(path: &std::path::Path, cfg: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cfg)?;
    // Write to a sibling temp file then rename so the update is atomic —
    // a crash mid-write cannot leave a truncated settings.json.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Internal: read config from an explicit `path`.
fn load_from(path: &std::path::Path) -> Config {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(_) => return Config::default(),
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Hex colour parsing
// ---------------------------------------------------------------------------

/// Parse a hex colour string into [`Color::Rgb`].
///
/// Accepts `"#rrggbb"` or `"rrggbb"` (case-insensitive).
/// On any parse failure (wrong length, non-hex digits, empty string) returns
/// the default accent orange — fail-open by design.
pub fn hex_to_color(s: &str) -> Color {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return Color::Rgb(0xd9, 0x77, 0x57);
    }
    let r = u8::from_str_radix(&s[0..2], 16);
    let g = u8::from_str_radix(&s[2..4], 16);
    let b = u8::from_str_radix(&s[4..6], 16);
    match (r, g, b) {
        (Ok(r), Ok(g), Ok(b)) => Color::Rgb(r, g, b),
        _ => Color::Rgb(0xd9, 0x77, 0x57),
    }
}

// ---------------------------------------------------------------------------
// AgentState → Stage mapping
// ---------------------------------------------------------------------------

impl Config {
    /// Return the [`Stage`] that governs notifications for the given state.
    ///
    /// - `Question`  → `notify.clarify`
    /// - `Approval`  → `notify.approve`
    /// - `Done`      → `notify.done`
    /// - Everything else → `None`
    pub fn stage_for(&self, state: AgentState) -> Option<Stage> {
        match state {
            AgentState::Question => Some(self.notify.clarify),
            AgentState::Approval => Some(self.notify.approve),
            AgentState::Done => Some(self.notify.done),
            _ => None,
        }
    }

    /// Resolve the configured accent hex string into a [`Color::Rgb`].
    ///
    /// Fail-open: invalid or missing hex → default orange `#d97757`.
    pub fn accent_color(&self) -> Color {
        hex_to_color(&self.accent)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ① Config::default() → all stages have banner=true, sound=true
    #[test]
    fn default_is_all_enabled() {
        let cfg = Config::default();
        for stage in [cfg.notify.clarify, cfg.notify.approve, cfg.notify.done] {
            assert!(stage.banner, "expected banner=true in default");
            assert!(stage.sound, "expected sound=true in default");
        }
    }

    // ② Empty JSON `{}` parses to default
    #[test]
    fn empty_json_equals_default() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg, Config::default());
    }

    // ③ Partial JSON: only done.sound=false; rest stays true
    #[test]
    fn partial_json_preserves_defaults() {
        let cfg: Config =
            serde_json::from_str(r#"{"notify":{"done":{"sound":false}}}"#).unwrap();
        assert!(cfg.notify.done.banner);
        assert!(!cfg.notify.done.sound);
        assert!(cfg.notify.clarify.banner);
        assert!(cfg.notify.clarify.sound);
        assert!(cfg.notify.approve.banner);
        assert!(cfg.notify.approve.sound);
    }

    // ④ save() → load() round-trip equality (uses explicit path so the test
    //    doesn't touch the real ~/.roost/settings.json).
    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        let mut cfg = Config::default();
        cfg.notify.done.sound = false;
        cfg.notify.approve.banner = false;

        save_to(&path, &cfg).unwrap();
        let loaded = load_from(&path);

        assert_eq!(cfg, loaded);
    }

    // ⑤ Corrupt JSON file → load_from() returns default (no panic)
    #[test]
    fn corrupt_json_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, b"not valid json {{{{").unwrap();

        let loaded = load_from(&path);
        assert_eq!(loaded, Config::default());
    }

    // ⑥ stage_for mappings
    #[test]
    fn stage_for_mappings() {
        let cfg = Config::default();

        // Question → clarify
        assert_eq!(cfg.stage_for(AgentState::Question), Some(cfg.notify.clarify));
        // Approval → approve
        assert_eq!(cfg.stage_for(AgentState::Approval), Some(cfg.notify.approve));
        // Done → done
        assert_eq!(cfg.stage_for(AgentState::Done), Some(cfg.notify.done));
        // Non-target states → None
        assert_eq!(cfg.stage_for(AgentState::Working), None);
        assert_eq!(cfg.stage_for(AgentState::Idle), None);
    }

    // ── remote_offline default and round-trip ─────────────────────────────────

    /// remote_offline defaults to false (opt-in, unlike stages which are fail-open)
    #[test]
    fn remote_offline_defaults_to_false() {
        let cfg = Config::default();
        assert!(
            !cfg.notify.remote_offline,
            "remote_offline must default to false"
        );
    }

    /// Empty JSON `{}` → remote_offline=false (serde default)
    #[test]
    fn remote_offline_empty_json_is_false() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(
            !cfg.notify.remote_offline,
            "remote_offline must be false when absent from JSON"
        );
    }

    /// JSON with remote_offline=true round-trips correctly
    #[test]
    fn remote_offline_roundtrip_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        let mut cfg = Config::default();
        cfg.notify.remote_offline = true;

        save_to(&path, &cfg).unwrap();
        let loaded = load_from(&path);

        assert!(
            loaded.notify.remote_offline,
            "remote_offline=true must survive save/load"
        );
        assert_eq!(cfg, loaded);
    }

    /// JSON with remote_offline=false round-trips correctly
    #[test]
    fn remote_offline_roundtrip_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        let mut cfg = Config::default();
        cfg.notify.remote_offline = false;

        save_to(&path, &cfg).unwrap();
        let loaded = load_from(&path);
        assert!(!loaded.notify.remote_offline);
    }

    /// Corrupt JSON → load_from() returns default (remote_offline=false)
    #[test]
    fn remote_offline_corrupt_json_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, b"{{not json}}").unwrap();

        let loaded = load_from(&path);
        assert!(
            !loaded.notify.remote_offline,
            "corrupt JSON must fall back to default (false)"
        );
    }

    /// Partial JSON: only remote_offline set; stage flags stay default
    #[test]
    fn remote_offline_partial_json() {
        let cfg: Config =
            serde_json::from_str(r#"{"notify":{"remote_offline":true}}"#).unwrap();
        assert!(cfg.notify.remote_offline, "remote_offline must be parsed");
        // Stage flags must still be default-true
        assert!(cfg.notify.clarify.banner);
        assert!(cfg.notify.clarify.sound);
        assert!(cfg.notify.approve.banner);
        assert!(cfg.notify.done.sound);
    }

    // ── accent / theme colour tests ───────────────────────────────────────────

    /// Config::default() accent field is the canonical orange hex string.
    #[test]
    fn accent_default_is_orange_hex() {
        let cfg = Config::default();
        assert_eq!(cfg.accent, "#d97757");
    }

    /// Empty JSON `{}` → accent defaults to "#d97757".
    #[test]
    fn accent_empty_json_defaults_to_orange() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.accent, "#d97757");
    }

    /// Explicit accent in JSON is preserved.
    #[test]
    fn accent_json_custom_value_is_parsed() {
        let cfg: Config = serde_json::from_str(r##"{"accent":"#ff0000"}"##).unwrap();
        assert_eq!(cfg.accent, "#ff0000");
    }

    /// accent_color() on default config returns the orange Rgb value.
    #[test]
    fn accent_color_default_is_orange_rgb() {
        use ratatui::style::Color;
        let cfg = Config::default();
        assert_eq!(cfg.accent_color(), Color::Rgb(0xd9, 0x77, 0x57));
    }

    /// accent_color() with a valid custom hex returns the correct Rgb.
    #[test]
    fn accent_color_custom_hex_parses_correctly() {
        use ratatui::style::Color;
        let cfg = Config {
            accent: "#1a2b3c".to_string(),
            ..Default::default()
        };
        assert_eq!(cfg.accent_color(), Color::Rgb(0x1a, 0x2b, 0x3c));
    }

    /// hex_to_color: works without leading '#'.
    #[test]
    fn hex_to_color_without_hash() {
        use ratatui::style::Color;
        let color = hex_to_color("d97757");
        assert_eq!(color, Color::Rgb(0xd9, 0x77, 0x57));
    }

    /// hex_to_color: invalid string falls back to default orange.
    #[test]
    fn hex_to_color_invalid_falls_back_to_orange() {
        use ratatui::style::Color;
        assert_eq!(hex_to_color("nope"), Color::Rgb(0xd9, 0x77, 0x57));
        assert_eq!(hex_to_color(""), Color::Rgb(0xd9, 0x77, 0x57));
        assert_eq!(hex_to_color("#zzzzzz"), Color::Rgb(0xd9, 0x77, 0x57));
        assert_eq!(hex_to_color("#12345"), Color::Rgb(0xd9, 0x77, 0x57)); // too short
    }

    /// accent_color() with an invalid hex string falls back to default orange.
    #[test]
    fn accent_color_invalid_hex_falls_back_to_orange() {
        use ratatui::style::Color;
        let cfg = Config {
            accent: "not-a-color".to_string(),
            ..Default::default()
        };
        assert_eq!(cfg.accent_color(), Color::Rgb(0xd9, 0x77, 0x57));
    }

    /// Round-trip: save with custom accent, load back, accent_color matches.
    #[test]
    fn accent_roundtrip_via_file() {
        use ratatui::style::Color;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        let cfg = Config {
            accent: "#aabbcc".to_string(),
            ..Default::default()
        };

        save_to(&path, &cfg).unwrap();
        let loaded = load_from(&path);

        assert_eq!(loaded.accent, "#aabbcc");
        assert_eq!(loaded.accent_color(), Color::Rgb(0xaa, 0xbb, 0xcc));
    }

    /// Serialized JSON contains the "accent" key.
    #[test]
    fn accent_serializes_to_json_key() {
        let cfg = Config::default();
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"accent\""), "JSON must contain 'accent' key");
        assert!(json.contains("d97757"), "JSON must contain the default orange value");
    }
}
