//! Notification configuration: reads/writes `~/.roost/settings.json`.
//!
//! On any error (missing file, corrupt JSON, missing fields) `load()` falls
//! back to the all-enabled default — notifications are fail-open by design.

use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::history::roost_home;
use crate::session::AgentState;

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
}

/// Top-level config persisted to `settings.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub notify: NotifyConfig,
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
}
