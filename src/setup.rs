use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

/// Marker prefix used to identify roost-managed hook entries.
const ROOST_MARKER: &str = "roost hook ";

/// The 7 Claude hook events roost registers.
const CLAUDE_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "Stop",
    "SessionEnd",
];

/// Merge roost hooks into existing Claude settings JSON.
/// Returns the merged JSON value.
pub fn merge_claude_hooks(existing: Option<Value>, roost_bin: &str) -> Value {
    let mut root = match existing {
        Some(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };

    // Get or create the "hooks" object
    let hooks_obj = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));

    let hooks_map = if let Value::Object(map) = hooks_obj {
        map
    } else {
        // Reset if wrong type
        *hooks_obj = Value::Object(serde_json::Map::new());
        if let Value::Object(map) = hooks_obj {
            map
        } else {
            unreachable!()
        }
    };

    for event in CLAUDE_EVENTS {
        let command = format!("{} hook claude {}", roost_bin, event);

        // Get or create the array for this event
        let event_arr = hooks_map
            .entry(*event)
            .or_insert_with(|| Value::Array(vec![]));

        if let Value::Array(arr) = event_arr {
            // Check if roost entry already exists (idempotent)
            let already_exists = arr.iter().any(|entry| {
                is_roost_entry(entry)
                    && entry
                        .get("hooks")
                        .and_then(|h| h.as_array())
                        .map(|hooks| {
                            hooks.iter().any(|h| {
                                h.get("command")
                                    .and_then(|c| c.as_str())
                                    .map(|c| c == command)
                                    .unwrap_or(false)
                            })
                        })
                        .unwrap_or(false)
            });

            if !already_exists {
                arr.push(make_roost_hook_entry(&command));
            }
        }
    }

    Value::Object(root)
}

/// Create a roost hook entry object for a given command.
fn make_roost_hook_entry(command: &str) -> Value {
    serde_json::json!({
        "matcher": "",
        "hooks": [
            {
                "type": "command",
                "command": command
            }
        ]
    })
}

fn is_roost_entry(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|hooks| {
            hooks.iter().any(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .map(|c| c.contains(ROOST_MARKER))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Remove roost-managed hooks from Claude settings JSON.
pub fn remove_claude_hooks(existing: Value) -> Value {
    let mut root = match existing {
        Value::Object(map) => map,
        other => return other,
    };

    if let Some(Value::Object(hooks_map)) = root.get_mut("hooks") {
        for event in CLAUDE_EVENTS {
            if let Some(Value::Array(arr)) = hooks_map.get_mut(*event) {
                arr.retain(|entry| !is_roost_entry(entry));
            }
        }
    }

    Value::Object(root)
}

/// Install Claude Code hooks into ~/.claude/settings.json.
pub fn install_claude() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("HOME not set")?;
    let roost_bin = current_exe_path();
    install_claude_into(std::path::Path::new(&home), &roost_bin)
}

/// Install Claude Code hooks using an explicit home directory and binary path.
/// Useful for testing without mutating the real HOME or relying on the test binary name.
pub fn install_claude_into(home_dir: &std::path::Path, roost_bin: &str) -> Result<PathBuf> {
    let path = home_dir.join(".claude").join("settings.json");

    let existing = read_settings(&path)?;
    let merged = merge_claude_hooks(existing, roost_bin);

    write_settings_atomic(&path, &merged)?;
    Ok(path)
}

/// Uninstall roost hooks from ~/.claude/settings.json.
pub fn uninstall_claude() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("HOME not set")?;
    uninstall_claude_from(std::path::Path::new(&home))
}

/// Uninstall roost hooks using an explicit home directory.
/// Useful for testing without mutating the real HOME.
pub fn uninstall_claude_from(home_dir: &std::path::Path) -> Result<PathBuf> {
    let path = home_dir.join(".claude").join("settings.json");

    let existing = read_settings(&path)?;
    let cleaned = match existing {
        Some(v) => remove_claude_hooks(v),
        None => return Ok(path),
    };

    write_settings_atomic(&path, &cleaned)?;
    Ok(path)
}

fn current_exe_path() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "roost".to_string())
}

fn read_settings(path: &Path) -> Result<Option<Value>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path)?;
    if content.trim().is_empty() {
        return Ok(None);
    }
    let v: Value = serde_json::from_str(&content).context("invalid JSON in settings.json")?;
    Ok(Some(v))
}

fn write_settings_atomic(path: &Path, value: &Value) -> Result<()> {
    // Create parent dirs
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Write to temp file then rename (atomic)
    let tmp_path = path.with_extension("json.tmp");
    let content = serde_json::to_string_pretty(value)?;
    std::fs::write(&tmp_path, content)?;
    std::fs::rename(&tmp_path, path)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Codex setup
// ---------------------------------------------------------------------------

/// The Codex hook events roost registers (real Codex event names).
/// Must stay in sync with `hook::parse_event_kind` — the round-trip test
/// `setup_event_names_parse_correctly` (in tests) enforces this invariant.
const CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "Stop",
    "PostToolUse",
    "PermissionRequest",
];

/// Stale root-level keys written by an older roost bug that used a flat schema.
/// These are removed during both install (cleanup) and uninstall.
const CODEX_STALE_ROOT_KEYS: &[&str] = &[
    "SessionStart",
    "Stop",
    "UserPromptSubmit",
    "agent-turn-complete",
    "approval-requested",
];

/// Return the timeout (seconds) for a Codex hook event.
fn codex_event_timeout(event: &str) -> u64 {
    if event == "PermissionRequest" {
        7200
    } else {
        5
    }
}

/// Build a roost hook group entry for Codex hooks.json.
/// Structure: `{ "hooks": [{"type":"command","command":"…","timeout":N}] }`
/// PostToolUse additionally carries `"matcher": ""`.
fn make_codex_roost_group(command: &str, event: &str) -> Value {
    let timeout = codex_event_timeout(event);
    let hook_obj = serde_json::json!({
        "type": "command",
        "command": command,
        "timeout": timeout,
    });
    if event == "PostToolUse" {
        serde_json::json!({
            "hooks": [hook_obj],
            "matcher": "",
        })
    } else {
        serde_json::json!({
            "hooks": [hook_obj],
        })
    }
}

/// Returns true if `group` is a roost-managed Codex hook group
/// (contains an inner hook with a command starting with ROOST_MARKER).
fn is_codex_roost_group(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|hooks| {
            hooks.iter().any(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .map(|c| c.contains(ROOST_MARKER))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Returns true if `group` is a roost-managed Codex hook group for a specific command.
fn is_codex_roost_group_for(group: &Value, command: &str) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|hooks| {
            hooks.iter().any(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .map(|c| c == command)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Merge roost hooks into existing Codex hooks.json.
///
/// Uses the correct Codex schema:
/// ```json
/// { "hooks": {
///   "SessionStart":      [ { "hooks": [{"type":"command","command":"…","timeout":5}] } ],
///   "PostToolUse":       [ { "hooks": [{"type":"command","command":"…","timeout":5}], "matcher":"" } ],
///   "PermissionRequest": [ { "hooks": [{"type":"command","command":"…","timeout":7200}] } ],
///   ...
/// } }
/// ```
///
/// - Preserves existing non-roost groups in each event array (e.g. third-party hooks).
/// - Idempotent: running install twice does not produce duplicate roost groups.
/// - Cleans up stale flat root-level keys written by the old buggy schema.
pub fn merge_codex_hooks_json(existing: Option<Value>, roost_bin: &str) -> Value {
    let mut root = match existing {
        Some(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };

    // ── 1. Remove stale root-level keys from old buggy schema ────────────────
    for key in CODEX_STALE_ROOT_KEYS {
        // Only remove if it is NOT inside the "hooks" object (i.e. it's at root level).
        root.remove(*key);
    }

    // ── 2. Get or create the top-level "hooks" object ────────────────────────
    let hooks_obj = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));

    let hooks_map = if let Value::Object(map) = hooks_obj {
        map
    } else {
        *hooks_obj = Value::Object(serde_json::Map::new());
        if let Value::Object(map) = hooks_obj {
            map
        } else {
            unreachable!()
        }
    };

    // ── 3. For each event, append roost group if not already present ──────────
    for event in CODEX_EVENTS {
        let command = format!("{} hook codex {}", roost_bin, event);

        let event_arr = hooks_map
            .entry(*event)
            .or_insert_with(|| Value::Array(vec![]));

        if let Value::Array(arr) = event_arr {
            let already_exists = arr
                .iter()
                .any(|group| is_codex_roost_group_for(group, &command));

            if !already_exists {
                arr.push(make_codex_roost_group(&command, event));
            }
        }
    }

    Value::Object(root)
}

/// Remove roost-managed groups from Codex hooks.json.
/// Also removes stale flat root-level keys from the old buggy schema.
/// Preserves non-roost groups (e.g. third-party or user's own hooks).
pub fn remove_codex_hooks_json(existing: Value, _roost_bin: &str) -> Value {
    let mut root = match existing {
        Value::Object(map) => map,
        other => return other,
    };

    // Remove stale root-level keys
    for key in CODEX_STALE_ROOT_KEYS {
        root.remove(*key);
    }

    // Remove roost groups from inside the "hooks" object
    if let Some(Value::Object(hooks_map)) = root.get_mut("hooks") {
        for event in CODEX_EVENTS {
            if let Some(Value::Array(arr)) = hooks_map.get_mut(*event) {
                arr.retain(|group| !is_codex_roost_group(group));
            }
        }
    }

    Value::Object(root)
}

/// Enable Codex hooks feature in config.toml string.
/// Sets `[features]\nhooks = true`, preserving all other config.
/// Uses `toml_edit` so existing formatting/comments are retained.
pub fn enable_codex_hooks_feature(config_toml: &str) -> Result<String> {
    use toml_edit::{DocumentMut, Item, Table, value};

    let mut doc: DocumentMut = config_toml
        .parse()
        .context("failed to parse Codex config.toml")?;

    // Get or create [features] table.
    if !doc.contains_table("features") {
        doc["features"] = Item::Table(Table::new());
    }
    doc["features"]["hooks"] = value(true);

    Ok(doc.to_string())
}

/// Disable Codex hooks feature in config.toml string (set hooks = false).
pub fn disable_codex_hooks_feature(config_toml: &str) -> Result<String> {
    use toml_edit::{DocumentMut, value};

    let mut doc: DocumentMut = config_toml
        .parse()
        .context("failed to parse Codex config.toml")?;

    if doc.contains_table("features") {
        doc["features"]["hooks"] = value(false);
    }

    Ok(doc.to_string())
}

/// Resolve the Codex config directory.
/// Priority: `CODEX_HOME` env var → `~/.codex`.
/// Returns `Ok(None)` if the directory does not exist (Codex not installed).
fn resolve_codex_dir() -> Result<Option<PathBuf>> {
    if let Ok(ch) = std::env::var("CODEX_HOME") {
        let p = PathBuf::from(ch);
        return Ok(if p.exists() { Some(p) } else { None });
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("HOME not set")?;
    let p = PathBuf::from(home).join(".codex");
    Ok(if p.exists() { Some(p) } else { None })
}

/// Install Codex hooks using the real HOME / CODEX_HOME.
/// Resolves the Codex directory (respecting CODEX_HOME), then delegates to
/// `install_codex_into`.
/// Returns Ok(None) if the Codex directory is not found (not installed).
pub fn install_codex() -> Result<Option<PathBuf>> {
    let roost_bin = current_exe_path();
    let codex_dir = resolve_codex_dir()?;
    let base_dir = match codex_dir {
        // resolve_codex_dir already resolved the full .codex path; derive the
        // "home" by taking its parent so install_codex_into can join(".codex").
        Some(d) => d
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| d.clone()),
        None => return Ok(None),
    };
    install_codex_into(&base_dir, &roost_bin)
}

/// Install Codex hooks into an explicit base (home) directory.
/// Mirrors `install_claude_into`: accepts the home directory and resolves
/// `.codex` internally; never reads `CODEX_HOME`.
/// Returns Ok(None) if `base_dir/.codex` does not exist (Codex not installed).
pub fn install_codex_into(base_dir: &Path, roost_bin: &str) -> Result<Option<PathBuf>> {
    let codex_dir = base_dir.join(".codex");
    if !codex_dir.exists() {
        return Ok(None);
    }

    // 1. Write hooks.json
    let hooks_path = codex_dir.join("hooks.json");
    let existing_hooks = if hooks_path.exists() {
        let content = std::fs::read_to_string(&hooks_path)?;
        if content.trim().is_empty() {
            None
        } else {
            serde_json::from_str(&content).ok()
        }
    } else {
        None
    };
    let merged = merge_codex_hooks_json(existing_hooks, roost_bin);
    write_json_atomic(&hooks_path, &merged)?;

    // 2. Update config.toml
    let config_path = codex_dir.join("config.toml");
    let existing_toml = if config_path.exists() {
        std::fs::read_to_string(&config_path)?
    } else {
        String::new()
    };
    let updated_toml = enable_codex_hooks_feature(&existing_toml)?;
    write_file_atomic(&config_path, &updated_toml)?;

    Ok(Some(codex_dir))
}

/// Uninstall Codex hooks using the real HOME / CODEX_HOME.
pub fn uninstall_codex() -> Result<Option<PathBuf>> {
    let roost_bin = current_exe_path();
    let codex_dir = resolve_codex_dir()?;
    let base_dir = match codex_dir {
        Some(d) => d
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| d.clone()),
        None => return Ok(None),
    };
    uninstall_codex_from(&base_dir, &roost_bin)
}

/// Uninstall Codex hooks from an explicit base (home) directory.
/// Mirrors `uninstall_codex_from`: accepts the home directory and resolves
/// `.codex` internally; never reads `CODEX_HOME`.
pub fn uninstall_codex_from(base_dir: &Path, roost_bin: &str) -> Result<Option<PathBuf>> {
    let codex_dir = base_dir.join(".codex");
    if !codex_dir.exists() {
        return Ok(None);
    }

    // 1. Clean hooks.json
    let hooks_path = codex_dir.join("hooks.json");
    if hooks_path.exists() {
        let content = std::fs::read_to_string(&hooks_path)?;
        if !content.trim().is_empty()
            && let Ok(v) = serde_json::from_str::<Value>(&content)
        {
            let cleaned = remove_codex_hooks_json(v, roost_bin);
            write_json_atomic(&hooks_path, &cleaned)?;
        }
    }

    // 2. Revert config.toml
    let config_path = codex_dir.join("config.toml");
    if config_path.exists() {
        let existing_toml = std::fs::read_to_string(&config_path)?;
        let updated = disable_codex_hooks_feature(&existing_toml)?;
        write_file_atomic(&config_path, &updated)?;
    }

    Ok(Some(codex_dir))
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let content = serde_json::to_string_pretty(value)?;
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn write_file_atomic(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---------------------------------------------------------------------------
    // Codex pure-function tests (correct nested schema)
    // ---------------------------------------------------------------------------

    /// Helper: get event array from inside the "hooks" object.
    fn codex_event_arr<'a>(result: &'a Value, event: &str) -> &'a Vec<Value> {
        result
            .get("hooks")
            .and_then(|h| h.get(event))
            .and_then(|v| v.as_array())
            .unwrap_or_else(|| panic!("codex event {event} not found inside hooks object"))
    }

    /// Helper: extract the inner command from a Codex hook group.
    fn codex_group_command(group: &Value) -> Option<&str> {
        group
            .get("hooks")
            .and_then(|h| h.as_array())
            .and_then(|h| h.first())
            .and_then(|h| h.get("command"))
            .and_then(|c| c.as_str())
    }

    #[test]
    fn merge_codex_hooks_from_none_creates_all_events() {
        let result = merge_codex_hooks_json(None, "/usr/local/bin/roost");
        // Events must be nested inside a top-level "hooks" object
        let hooks = result
            .get("hooks")
            .expect("top-level 'hooks' object required");
        for event in CODEX_EVENTS {
            let arr = hooks.get(*event).and_then(|v| v.as_array());
            assert!(
                arr.is_some(),
                "codex event {event} missing inside hooks object"
            );
            assert!(
                !arr.unwrap().is_empty(),
                "codex event {event} array is empty"
            );
        }
    }

    #[test]
    fn merge_codex_hooks_uses_nested_group_schema() {
        // Verify the shape: hooks.SessionStart[0].hooks[0].command
        let result = merge_codex_hooks_json(None, "/usr/bin/roost");
        let arr = codex_event_arr(&result, "SessionStart");
        let group = &arr[0];
        // Group must have a "hooks" array (not a flat "command" key)
        assert!(
            group.get("hooks").is_some(),
            "group must have 'hooks' array"
        );
        assert!(
            group.get("command").is_none(),
            "group must NOT have flat 'command' key"
        );
        let cmd = codex_group_command(group).expect("inner command must be present");
        assert_eq!(cmd, "/usr/bin/roost hook codex SessionStart");
        // Inner hook must have type and timeout
        let inner = &group["hooks"][0];
        assert_eq!(inner.get("type").and_then(|v| v.as_str()), Some("command"));
        assert_eq!(inner.get("timeout").and_then(|v| v.as_u64()), Some(5));
    }

    #[test]
    fn merge_codex_hooks_permission_request_has_long_timeout() {
        let result = merge_codex_hooks_json(None, "/usr/bin/roost");
        let arr = codex_event_arr(&result, "PermissionRequest");
        let inner = &arr[0]["hooks"][0];
        assert_eq!(inner.get("timeout").and_then(|v| v.as_u64()), Some(7200));
    }

    #[test]
    fn merge_codex_hooks_post_tool_use_has_matcher() {
        let result = merge_codex_hooks_json(None, "/usr/bin/roost");
        let arr = codex_event_arr(&result, "PostToolUse");
        let group = &arr[0];
        // PostToolUse group must carry "matcher": ""
        assert_eq!(
            group.get("matcher").and_then(|v| v.as_str()),
            Some(""),
            "PostToolUse group must have matcher:\"\""
        );
    }

    #[test]
    fn merge_codex_hooks_idempotent() {
        let r1 = merge_codex_hooks_json(None, "/usr/bin/roost");
        let r2 = merge_codex_hooks_json(Some(r1.clone()), "/usr/bin/roost");
        for event in CODEX_EVENTS {
            let c1 = r1
                .get("hooks")
                .and_then(|h| h.get(*event))
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            let c2 = r2
                .get("hooks")
                .and_then(|h| h.get(*event))
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            assert_eq!(c1, c2, "codex event {event} duplicate after second merge");
        }
    }

    #[test]
    fn merge_codex_hooks_preserves_third_party_style_groups() {
        // Simulate an existing third-party style entry (correct nested schema)
        let existing = json!({
            "hooks": {
                "SessionStart": [
                    { "hooks": [{"type":"command","command":"third-party hook SessionStart","timeout":5}] }
                ]
            }
        });
        let result = merge_codex_hooks_json(Some(existing), "/usr/bin/roost");
        let arr = codex_event_arr(&result, "SessionStart");
        // Must have two groups: third-party + roost
        assert_eq!(
            arr.len(),
            2,
            "must preserve existing group AND add roost group"
        );
        let has_third_party = arr
            .iter()
            .any(|g| codex_group_command(g) == Some("third-party hook SessionStart"));
        assert!(has_third_party, "third-party group must be preserved");
    }

    #[test]
    fn merge_codex_hooks_cleans_stale_root_keys() {
        // Old buggy schema: stale keys at root level
        let existing = json!({
            "SessionStart": [{"command": "old-roost hook codex SessionStart"}],
            "Stop": [{"command": "old-roost hook codex Stop"}],
            "approval-requested": [{"command": "old-roost hook codex approval-requested"}],
            "agent-turn-complete": [{"command": "old-roost hook codex agent-turn-complete"}],
        });
        let result = merge_codex_hooks_json(Some(existing), "/usr/bin/roost");
        // Stale root keys must be removed
        for key in CODEX_STALE_ROOT_KEYS {
            assert!(
                result.get(key).is_none(),
                "stale root key '{key}' must be removed"
            );
        }
        // Events must now live inside "hooks" object
        let hooks = result.get("hooks").expect("hooks object required");
        for event in CODEX_EVENTS {
            assert!(
                hooks.get(*event).is_some(),
                "event {event} must be inside hooks object"
            );
        }
    }

    #[test]
    fn remove_codex_hooks_removes_roost_groups() {
        let installed = merge_codex_hooks_json(None, "/usr/bin/roost");
        let cleaned = remove_codex_hooks_json(installed, "/usr/bin/roost");
        if let Some(hooks) = cleaned.get("hooks").and_then(|h| h.as_object()) {
            for event in CODEX_EVENTS {
                if let Some(Value::Array(arr)) = hooks.get(*event) {
                    for group in arr {
                        assert!(
                            !is_codex_roost_group(group),
                            "codex event {event} still has roost group after remove"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn remove_codex_hooks_preserves_third_party_groups() {
        let existing = json!({
            "hooks": {
                "SessionStart": [
                    { "hooks": [{"type":"command","command":"third-party hook SessionStart","timeout":5}] }
                ]
            }
        });
        let installed = merge_codex_hooks_json(Some(existing), "/usr/bin/roost");
        let cleaned = remove_codex_hooks_json(installed, "/usr/bin/roost");
        let arr = codex_event_arr(&cleaned, "SessionStart");
        assert_eq!(arr.len(), 1, "only the third-party group should remain");
        assert_eq!(
            codex_group_command(&arr[0]),
            Some("third-party hook SessionStart")
        );
    }

    #[test]
    fn remove_codex_hooks_cleans_stale_root_keys() {
        let dirty = json!({
            "SessionStart": [{"command": "old"}],
            "approval-requested": [{"command": "old"}],
            "hooks": {}
        });
        let cleaned = remove_codex_hooks_json(dirty, "/usr/bin/roost");
        for key in CODEX_STALE_ROOT_KEYS {
            assert!(
                cleaned.get(key).is_none(),
                "stale root key '{key}' must be removed"
            );
        }
    }

    #[test]
    fn enable_codex_hooks_feature_sets_flag() {
        let toml = "[model]\nname = \"o4-mini\"\n";
        let result = enable_codex_hooks_feature(toml).unwrap();
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        assert_eq!(
            doc["features"]["hooks"].as_bool(),
            Some(true),
            "hooks feature flag should be true"
        );
        // Original content preserved
        assert_eq!(doc["model"]["name"].as_str(), Some("o4-mini"));
    }

    #[test]
    fn enable_codex_hooks_feature_idempotent() {
        let toml = "[features]\nhooks = true\n";
        let result = enable_codex_hooks_feature(toml).unwrap();
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        assert_eq!(doc["features"]["hooks"].as_bool(), Some(true));
    }

    #[test]
    fn enable_codex_hooks_feature_empty_toml() {
        let result = enable_codex_hooks_feature("").unwrap();
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        assert_eq!(doc["features"]["hooks"].as_bool(), Some(true));
    }

    #[test]
    fn disable_codex_hooks_feature_sets_false() {
        let toml = "[features]\nhooks = true\n";
        let result = disable_codex_hooks_feature(toml).unwrap();
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        assert_eq!(doc["features"]["hooks"].as_bool(), Some(false));
    }

    // ---------------------------------------------------------------------------
    // Round-trip: every event name written by setup must parse in hook::parse_event_kind
    // This prevents setup↔hook drift (e.g. setup writes "ApprovalRequested" but
    // hook parser only accepts "approval-requested").
    // ---------------------------------------------------------------------------

    #[test]
    fn setup_event_names_parse_correctly() {
        use crate::hook::parse_event_kind;

        // All Claude events written by roost setup must parse correctly.
        for event in CLAUDE_EVENTS {
            assert!(
                parse_event_kind(event).is_ok(),
                "Claude event '{event}' written by setup cannot be parsed by hook::parse_event_kind"
            );
        }

        // All Codex events written by roost setup must parse correctly.
        for event in CODEX_EVENTS {
            assert!(
                parse_event_kind(event).is_ok(),
                "Codex event '{event}' written by setup cannot be parsed by hook::parse_event_kind"
            );
        }
    }

    #[test]
    fn merge_from_none_creates_all_events() {
        let result = merge_claude_hooks(None, "/usr/local/bin/roost");

        let hooks = result.get("hooks").expect("hooks key");
        for event in CLAUDE_EVENTS {
            let arr = hooks.get(event).and_then(|v| v.as_array());
            assert!(arr.is_some(), "event {event} missing");
            assert!(!arr.unwrap().is_empty(), "event {event} empty");
        }
    }

    #[test]
    fn merge_from_none_correct_command_format() {
        let result = merge_claude_hooks(None, "/usr/local/bin/roost");
        let hooks = result.get("hooks").unwrap();
        let arr = hooks
            .get("SessionStart")
            .and_then(|v| v.as_array())
            .unwrap();

        let entry = &arr[0];
        let cmd = entry
            .get("hooks")
            .and_then(|h| h.as_array())
            .and_then(|h| h.first())
            .and_then(|h| h.get("command"))
            .and_then(|c| c.as_str())
            .unwrap();

        assert_eq!(cmd, "/usr/local/bin/roost hook claude SessionStart");
    }

    #[test]
    fn merge_preserves_user_hooks() {
        let existing = json!({
            "hooks": {
                "SessionStart": [
                    {
                        "matcher": "",
                        "hooks": [{"type": "command", "command": "my-custom-hook"}]
                    }
                ]
            }
        });

        let result = merge_claude_hooks(Some(existing), "/usr/bin/roost");
        let arr = result
            .get("hooks")
            .and_then(|h| h.get("SessionStart"))
            .and_then(|v| v.as_array())
            .unwrap();

        // Should have both user hook AND roost hook
        assert_eq!(arr.len(), 2);

        let has_user = arr.iter().any(|e| {
            e.get("hooks")
                .and_then(|h| h.as_array())
                .and_then(|h| h.first())
                .and_then(|h| h.get("command"))
                .and_then(|c| c.as_str())
                == Some("my-custom-hook")
        });
        assert!(has_user, "user hook should be preserved");
    }

    #[test]
    fn merge_is_idempotent() {
        let result1 = merge_claude_hooks(None, "/usr/bin/roost");
        let result2 = merge_claude_hooks(Some(result1.clone()), "/usr/bin/roost");

        for event in CLAUDE_EVENTS {
            let arr1 = result1
                .get("hooks")
                .and_then(|h| h.get(*event))
                .and_then(|v| v.as_array())
                .unwrap();
            let arr2 = result2
                .get("hooks")
                .and_then(|h| h.get(*event))
                .and_then(|v| v.as_array())
                .unwrap();

            assert_eq!(
                arr1.len(),
                arr2.len(),
                "event {event}: applying merge twice should not create duplicates"
            );
        }
    }

    #[test]
    fn remove_claude_hooks_removes_roost_entries() {
        let installed = merge_claude_hooks(None, "/usr/bin/roost");
        let cleaned = remove_claude_hooks(installed);

        let hooks = cleaned.get("hooks").unwrap();
        for event in CLAUDE_EVENTS {
            if let Some(arr) = hooks.get(event).and_then(|v| v.as_array()) {
                assert!(
                    arr.iter().all(|e| !is_roost_entry(e)),
                    "event {event} still has roost entry after uninstall"
                );
            }
        }
    }

    #[test]
    fn remove_claude_hooks_preserves_user_entries() {
        let existing = json!({
            "hooks": {
                "SessionStart": [
                    {
                        "matcher": "",
                        "hooks": [{"type": "command", "command": "my-custom-hook"}]
                    }
                ]
            }
        });

        let installed = merge_claude_hooks(Some(existing), "/usr/bin/roost");
        let cleaned = remove_claude_hooks(installed);

        let arr = cleaned
            .get("hooks")
            .and_then(|h| h.get("SessionStart"))
            .and_then(|v| v.as_array())
            .unwrap();

        // user hook should remain
        assert_eq!(arr.len(), 1);
        let cmd = arr[0]
            .get("hooks")
            .and_then(|h| h.as_array())
            .and_then(|h| h.first())
            .and_then(|h| h.get("command"))
            .and_then(|c| c.as_str())
            .unwrap();
        assert_eq!(cmd, "my-custom-hook");
    }
}
