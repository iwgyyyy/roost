use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

/// Marker prefix used to identify roost-managed hook entries.
const ROOST_MARKER: &str = "roost hook ";

// ---------------------------------------------------------------------------
// Flag helpers
// ---------------------------------------------------------------------------

/// Build the optional flag fragment that is baked into hook commands.
///
/// The fragment is placed **after** `hook` in the generated command so that
/// the `ROOST_MARKER` (`"roost hook "`) remains intact and idempotency checks
/// continue to work. Returns a string like `" --sock /path --origin label"`
/// (leading space included so it can be directly concatenated), or an empty
/// string when both flags are absent.
///
/// Resulting command shape:
///   `<roost_bin> hook [--sock <path>] [--origin <label>] <family> <event>`
fn build_extra_flags(sock: Option<&str>, origin: Option<&str>) -> String {
    let mut s = String::new();
    if let Some(p) = sock {
        s.push_str(" --sock ");
        s.push_str(p);
    }
    if let Some(o) = origin {
        s.push_str(" --origin ");
        s.push_str(o);
    }
    s
}

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

/// Merge roost hooks into existing Claude settings JSON, with optional extra flags.
///
/// `sock` and `origin` are baked into every generated hook command; both are
/// `None` for a local-machine install (zero diff vs. the original behaviour).
pub fn merge_claude_hooks_with_flags(
    existing: Option<Value>,
    roost_bin: &str,
    sock: Option<&str>,
    origin: Option<&str>,
) -> Value {
    let extra = build_extra_flags(sock, origin);

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
        // Flags go between `hook` and the family+event so the ROOST_MARKER
        // ("roost hook ") prefix is preserved for idempotency detection.
        let command = format!("{} hook{} claude {}", roost_bin, extra, event);

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

/// Merge roost hooks into existing Claude settings JSON.
/// Returns the merged JSON value.
pub fn merge_claude_hooks(existing: Option<Value>, roost_bin: &str) -> Value {
    merge_claude_hooks_with_flags(existing, roost_bin, None, None)
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
/// `sock` and `origin` are baked into commands when given.
pub fn install_claude_with_opts(sock: Option<&str>, origin: Option<&str>) -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("HOME not set")?;
    let roost_bin = current_exe_path();
    install_claude_into_with_opts(std::path::Path::new(&home), &roost_bin, sock, origin)
}

/// Install Claude Code hooks into ~/.claude/settings.json.
pub fn install_claude() -> Result<PathBuf> {
    install_claude_with_opts(None, None)
}

/// Install Claude Code hooks using an explicit home directory and binary path.
/// Useful for testing without mutating the real HOME or relying on the test binary name.
pub fn install_claude_into(home_dir: &std::path::Path, roost_bin: &str) -> Result<PathBuf> {
    install_claude_into_with_opts(home_dir, roost_bin, None, None)
}

/// Install Claude Code hooks using an explicit home directory, binary path, and optional flags.
pub fn install_claude_into_with_opts(
    home_dir: &std::path::Path,
    roost_bin: &str,
    sock: Option<&str>,
    origin: Option<&str>,
) -> Result<PathBuf> {
    let path = home_dir.join(".claude").join("settings.json");

    let existing = read_settings(&path)?;
    let merged = merge_claude_hooks_with_flags(existing, roost_bin, sock, origin);

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

/// True if roost hooks are already present in the Claude settings or, failing
/// that, the Codex hooks file. Used to decide whether to offer setup on first
/// run. Missing files / parse errors are treated as "not installed".
pub fn hooks_installed() -> bool {
    let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) else {
        return false;
    };
    let home = Path::new(&home);

    let claude = home.join(".claude").join("settings.json");
    if let Ok(Some(v)) = read_settings(&claude)
        && config_has_roost_hook(&v, is_roost_entry)
    {
        return true;
    }

    let codex = home.join(".codex").join("hooks.json");
    if let Ok(Some(v)) = read_settings(&codex)
        && config_has_roost_hook(&v, is_codex_roost_group)
    {
        return true;
    }

    for sub in [".codewhale", ".deepseek"] {
        let cw = home.join(sub).join("config.toml");
        if let Ok(content) = std::fs::read_to_string(&cw)
            && deepseek_config_has_roost_hook(&content)
        {
            return true;
        }
    }

    let cursor = home.join(".cursor").join("hooks.json");
    if let Ok(Some(v)) = read_settings(&cursor)
        && config_has_roost_hook(&v, is_cursor_roost_entry)
    {
        return true;
    }

    false
}

/// Shared shape check for both agents: a top-level `hooks` object whose event
/// arrays contain at least one entry/group matching `is_roost` (a roost hook).
fn config_has_roost_hook(v: &Value, is_roost: fn(&Value) -> bool) -> bool {
    v.get("hooks")
        .and_then(|h| h.as_object())
        .map(|events| {
            events.values().any(|arr| {
                arr.as_array()
                    .map(|items| items.iter().any(is_roost))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
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
    "PreToolUse",
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
/// - `PostToolUse` carries `"matcher": ""` (all tools).
/// - `PreToolUse` carries `"matcher": "request_user_input"` so it fires ONLY for
///   the clarify question tool, never for ordinary tool calls (zero overhead on
///   Bash/edits). See `session::CLARIFY_TOOL`.
fn make_codex_roost_group(command: &str, event: &str) -> Value {
    let timeout = codex_event_timeout(event);
    let hook_obj = serde_json::json!({
        "type": "command",
        "command": command,
        "timeout": timeout,
    });
    match event {
        "PostToolUse" => serde_json::json!({ "hooks": [hook_obj], "matcher": "" }),
        "PreToolUse" => serde_json::json!({
            "hooks": [hook_obj],
            "matcher": crate::session::CLARIFY_TOOL,
        }),
        _ => serde_json::json!({ "hooks": [hook_obj] }),
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

/// Merge roost hooks into existing Codex hooks.json, with optional extra flags.
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
/// - `sock` and `origin` are baked into every generated command when given.
pub fn merge_codex_hooks_json_with_flags(
    existing: Option<Value>,
    roost_bin: &str,
    sock: Option<&str>,
    origin: Option<&str>,
) -> Value {
    let extra = build_extra_flags(sock, origin);

    let mut root = match existing {
        Some(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };

    // ── 1. Remove stale root-level keys from old buggy schema ────────────────
    for key in CODEX_STALE_ROOT_KEYS {
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
        let command = format!("{} hook{} codex {}", roost_bin, extra, event);

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

/// Merge roost hooks into existing Codex hooks.json.
pub fn merge_codex_hooks_json(existing: Option<Value>, roost_bin: &str) -> Value {
    merge_codex_hooks_json_with_flags(existing, roost_bin, None, None)
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
/// `sock` and `origin` are baked into commands when given.
pub fn install_codex_with_opts(sock: Option<&str>, origin: Option<&str>) -> Result<Option<PathBuf>> {
    let roost_bin = current_exe_path();
    let codex_dir = resolve_codex_dir()?;
    let base_dir = match codex_dir {
        Some(d) => d
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| d.clone()),
        None => return Ok(None),
    };
    install_codex_into_with_opts(&base_dir, &roost_bin, sock, origin)
}

/// Install Codex hooks using the real HOME / CODEX_HOME.
/// Resolves the Codex directory (respecting CODEX_HOME), then delegates to
/// `install_codex_into`.
/// Returns Ok(None) if the Codex directory is not found (not installed).
pub fn install_codex() -> Result<Option<PathBuf>> {
    install_codex_with_opts(None, None)
}

/// Install Codex hooks into an explicit base (home) directory.
/// Mirrors `install_claude_into`: accepts the home directory and resolves
/// `.codex` internally; never reads `CODEX_HOME`.
/// Returns Ok(None) if `base_dir/.codex` does not exist (Codex not installed).
pub fn install_codex_into(base_dir: &Path, roost_bin: &str) -> Result<Option<PathBuf>> {
    install_codex_into_with_opts(base_dir, roost_bin, None, None)
}

/// Install Codex hooks into an explicit base directory with optional extra flags.
pub fn install_codex_into_with_opts(
    base_dir: &Path,
    roost_bin: &str,
    sock: Option<&str>,
    origin: Option<&str>,
) -> Result<Option<PathBuf>> {
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
    let merged = merge_codex_hooks_json_with_flags(existing_hooks, roost_bin, sock, origin);
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

// ---------------------------------------------------------------------------
// DeepSeek / CodeWhale setup
// ---------------------------------------------------------------------------

/// CodeWhale hook events roost registers, paired with the roost EventKind name
/// embedded in the hook command. CodeWhale fires snake_case lifecycle events;
/// the command argument uses roost's canonical EventKind names so `hook.rs`
/// parses them unchanged. CodeWhale has no turn-completion event, so there is
/// no `Stop`/Done mapping (see `session::apply` dispatch comment).
const DEEPSEEK_EVENTS: &[(&str, &str)] = &[
    ("session_start", "SessionStart"),
    ("message_submit", "UserPromptSubmit"),
    ("tool_call_before", "PreToolUse"),
    ("tool_call_after", "PostToolUse"),
    ("session_end", "SessionEnd"),
];

/// Merge roost hooks into a CodeWhale `config.toml` string, with optional extra flags.
///
/// Sets `[hooks] enabled = true` and appends one `[[hooks.hooks]]` table per
/// event. Idempotent (no duplicate roost entries on re-run); preserves the
/// user's own hooks, formatting, and comments via `toml_edit`.
/// `sock` and `origin` are baked into every generated command when given.
pub fn merge_deepseek_hooks_toml_with_flags(
    config_toml: &str,
    roost_bin: &str,
    sock: Option<&str>,
    origin: Option<&str>,
) -> Result<String> {
    use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

    let extra = build_extra_flags(sock, origin);

    let mut doc: DocumentMut = config_toml
        .parse()
        .context("failed to parse CodeWhale config.toml")?;

    // Ensure [hooks] table and enable the feature.
    if !doc.contains_table("hooks") {
        doc["hooks"] = Item::Table(Table::new());
    }
    doc["hooks"]["enabled"] = value(true);

    // Ensure hooks.hooks is an array-of-tables.
    let needs_aot = doc["hooks"]
        .as_table()
        .map(|t| {
            t.get("hooks")
                .and_then(|i| i.as_array_of_tables())
                .is_none()
        })
        .unwrap_or(true);
    if needs_aot {
        doc["hooks"]["hooks"] = Item::ArrayOfTables(ArrayOfTables::new());
    }
    let aot = doc["hooks"]["hooks"]
        .as_array_of_tables_mut()
        .expect("hooks.hooks is an array-of-tables");

    for (cw_event, roost_event) in DEEPSEEK_EVENTS {
        let command = format!("{roost_bin} hook{extra} deepseek {roost_event}");
        let exists = aot
            .iter()
            .any(|t| t.get("command").and_then(|c| c.as_str()) == Some(command.as_str()));
        if !exists {
            let mut t = Table::new();
            t.insert("event", value(*cw_event));
            t.insert("command", value(command));
            aot.push(t);
        }
    }

    Ok(doc.to_string())
}

/// Merge roost hooks into a CodeWhale `config.toml` string.
pub fn merge_deepseek_hooks_toml(config_toml: &str, roost_bin: &str) -> Result<String> {
    merge_deepseek_hooks_toml_with_flags(config_toml, roost_bin, None, None)
}

/// Remove roost-managed `[[hooks.hooks]]` entries from a CodeWhale config.toml.
/// Preserves the user's own hook entries. If no hooks remain after removal,
/// reverts `enabled = false` (so uninstall does not leave the feature on for a
/// user who never had hooks); otherwise leaves `enabled` untouched.
pub fn remove_deepseek_hooks_toml(config_toml: &str) -> Result<String> {
    use toml_edit::{ArrayOfTables, DocumentMut, value};

    let mut doc: DocumentMut = config_toml
        .parse()
        .context("failed to parse CodeWhale config.toml")?;

    let Some(hooks_tbl) = doc.get_mut("hooks").and_then(|i| i.as_table_mut()) else {
        return Ok(doc.to_string());
    };
    if let Some(aot) = hooks_tbl
        .get_mut("hooks")
        .and_then(|i| i.as_array_of_tables_mut())
    {
        let mut kept = ArrayOfTables::new();
        for t in aot.iter() {
            let is_roost = t
                .get("command")
                .and_then(|c| c.as_str())
                .map(|c| c.contains(ROOST_MARKER))
                .unwrap_or(false);
            if !is_roost {
                kept.push(t.clone());
            }
        }
        let empty = kept.is_empty();
        *aot = kept;
        if empty {
            hooks_tbl["enabled"] = value(false);
        }
    }

    Ok(doc.to_string())
}

/// True if a CodeWhale config.toml carries at least one roost hook entry.
fn deepseek_config_has_roost_hook(config_toml: &str) -> bool {
    let Ok(doc) = config_toml.parse::<toml_edit::DocumentMut>() else {
        return false;
    };
    doc.get("hooks")
        .and_then(|h| h.as_table())
        .and_then(|t| t.get("hooks"))
        .and_then(|i| i.as_array_of_tables())
        .map(|aot| {
            aot.iter().any(|t| {
                t.get("command")
                    .and_then(|c| c.as_str())
                    .map(|c| c.contains(ROOST_MARKER))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Resolve the CodeWhale config directory: `~/.codewhale` (preferred) or the
/// legacy `~/.deepseek` compatibility dir. Returns `Ok(None)` if neither exists
/// (CodeWhale not installed / never run).
fn resolve_deepseek_dir() -> Result<Option<PathBuf>> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("HOME not set")?;
    for sub in [".codewhale", ".deepseek"] {
        let p = PathBuf::from(&home).join(sub);
        if p.exists() {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// Install CodeWhale hooks using the real HOME.
/// `sock` and `origin` are baked into commands when given.
pub fn install_deepseek_with_opts(sock: Option<&str>, origin: Option<&str>) -> Result<Option<PathBuf>> {
    let roost_bin = current_exe_path();
    match resolve_deepseek_dir()? {
        Some(dir) => install_deepseek_into_with_opts(&dir, &roost_bin, sock, origin),
        None => Ok(None),
    }
}

/// Install CodeWhale hooks using the real HOME.
/// Returns Ok(None) if no CodeWhale config dir exists (not installed).
pub fn install_deepseek() -> Result<Option<PathBuf>> {
    install_deepseek_with_opts(None, None)
}

/// Install CodeWhale hooks into an explicit config dir (e.g. `~/.codewhale`).
pub fn install_deepseek_into(config_dir: &Path, roost_bin: &str) -> Result<Option<PathBuf>> {
    install_deepseek_into_with_opts(config_dir, roost_bin, None, None)
}

/// Install CodeWhale hooks into an explicit config dir with optional extra flags.
pub fn install_deepseek_into_with_opts(
    config_dir: &Path,
    roost_bin: &str,
    sock: Option<&str>,
    origin: Option<&str>,
) -> Result<Option<PathBuf>> {
    let config_path = config_dir.join("config.toml");
    let existing = if config_path.exists() {
        std::fs::read_to_string(&config_path)?
    } else {
        String::new()
    };
    let updated = merge_deepseek_hooks_toml_with_flags(&existing, roost_bin, sock, origin)?;
    write_file_atomic(&config_path, &updated)?;
    Ok(Some(config_dir.to_path_buf()))
}

/// Uninstall CodeWhale hooks using the real HOME.
pub fn uninstall_deepseek() -> Result<Option<PathBuf>> {
    match resolve_deepseek_dir()? {
        Some(dir) => uninstall_deepseek_from(&dir),
        None => Ok(None),
    }
}

/// Uninstall CodeWhale hooks from an explicit config dir.
pub fn uninstall_deepseek_from(config_dir: &Path) -> Result<Option<PathBuf>> {
    let config_path = config_dir.join("config.toml");
    if config_path.exists() {
        let existing = std::fs::read_to_string(&config_path)?;
        if !existing.trim().is_empty() {
            let cleaned = remove_deepseek_hooks_toml(&existing)?;
            write_file_atomic(&config_path, &cleaned)?;
        }
    }
    Ok(Some(config_dir.to_path_buf()))
}

// ---------------------------------------------------------------------------
// Cursor setup
// ---------------------------------------------------------------------------

/// Cursor hook events roost registers, paired with the roost EventKind name in
/// the command. Cursor uses camelCase lifecycle events; the command argument
/// uses roost's canonical EventKind names so `hook::parse_event_kind` parses
/// them unchanged. Cursor has a `stop` event → Done.
const CURSOR_EVENTS: &[(&str, &str)] = &[
    ("sessionStart", "SessionStart"),
    ("beforeSubmitPrompt", "UserPromptSubmit"),
    ("preToolUse", "PreToolUse"),
    ("postToolUse", "PostToolUse"),
    ("stop", "Stop"),
    ("sessionEnd", "SessionEnd"),
];

/// A roost hook entry in Cursor's hooks.json: `{"command":"…","timeout":5}`.
/// No `failClosed` (fail-open: a roost hook failure never blocks Cursor) and no
/// matcher (roost observes every event).
fn make_cursor_roost_entry(command: &str) -> Value {
    serde_json::json!({ "command": command, "timeout": 5 })
}

fn is_cursor_roost_entry(entry: &Value) -> bool {
    entry
        .get("command")
        .and_then(|c| c.as_str())
        .map(|c| c.contains(ROOST_MARKER))
        .unwrap_or(false)
}

/// Merge roost hooks into a Cursor `hooks.json` value (schema version 1), with optional flags.
/// Idempotent; preserves the user's own hooks in each event array.
/// `sock` and `origin` are baked into every generated command when given.
pub fn merge_cursor_hooks_json_with_flags(
    existing: Option<Value>,
    roost_bin: &str,
    sock: Option<&str>,
    origin: Option<&str>,
) -> Value {
    let extra = build_extra_flags(sock, origin);

    let mut root = match existing {
        Some(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    root.insert("version".to_string(), Value::from(1));

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

    for (cursor_event, roost_event) in CURSOR_EVENTS {
        let command = format!("{roost_bin} hook{extra} cursor {roost_event}");
        let arr = hooks_map
            .entry(*cursor_event)
            .or_insert_with(|| Value::Array(vec![]));
        if let Value::Array(a) = arr {
            let exists = a
                .iter()
                .any(|e| e.get("command").and_then(|c| c.as_str()) == Some(command.as_str()));
            if !exists {
                a.push(make_cursor_roost_entry(&command));
            }
        }
    }

    Value::Object(root)
}

/// Merge roost hooks into a Cursor `hooks.json` value (schema version 1).
/// Idempotent; preserves the user's own hooks in each event array.
pub fn merge_cursor_hooks_json(existing: Option<Value>, roost_bin: &str) -> Value {
    merge_cursor_hooks_json_with_flags(existing, roost_bin, None, None)
}

/// Remove roost-managed entries from a Cursor hooks.json. Preserves user hooks.
pub fn remove_cursor_hooks_json(existing: Value) -> Value {
    let mut root = match existing {
        Value::Object(map) => map,
        other => return other,
    };
    if let Some(Value::Object(hooks_map)) = root.get_mut("hooks") {
        for (cursor_event, _) in CURSOR_EVENTS {
            if let Some(Value::Array(a)) = hooks_map.get_mut(*cursor_event) {
                a.retain(|e| !is_cursor_roost_entry(e));
            }
        }
    }
    Value::Object(root)
}

/// Install Cursor hooks using the real HOME. `sock`/`origin` are baked into commands.
pub fn install_cursor_with_opts(sock: Option<&str>, origin: Option<&str>) -> Result<Option<PathBuf>> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("HOME not set")?;
    install_cursor_into_with_opts(Path::new(&home), &current_exe_path(), sock, origin)
}

/// Install Cursor hooks using the real HOME. Ok(None) if `~/.cursor` is absent.
pub fn install_cursor() -> Result<Option<PathBuf>> {
    install_cursor_with_opts(None, None)
}

/// Install Cursor hooks into `<home>/.cursor/hooks.json`. Ok(None) if absent.
pub fn install_cursor_into(home_dir: &Path, roost_bin: &str) -> Result<Option<PathBuf>> {
    install_cursor_into_with_opts(home_dir, roost_bin, None, None)
}

/// Install Cursor hooks into `<home>/.cursor/hooks.json` with optional extra flags.
pub fn install_cursor_into_with_opts(
    home_dir: &Path,
    roost_bin: &str,
    sock: Option<&str>,
    origin: Option<&str>,
) -> Result<Option<PathBuf>> {
    let cursor_dir = home_dir.join(".cursor");
    if !cursor_dir.exists() {
        return Ok(None);
    }
    let path = cursor_dir.join("hooks.json");
    let existing = read_settings(&path)?;
    let merged = merge_cursor_hooks_json_with_flags(existing, roost_bin, sock, origin);
    write_json_atomic(&path, &merged)?;
    Ok(Some(path))
}

/// Uninstall Cursor hooks using the real HOME.
pub fn uninstall_cursor() -> Result<Option<PathBuf>> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("HOME not set")?;
    uninstall_cursor_from(Path::new(&home))
}

/// Uninstall Cursor hooks from `<home>/.cursor/hooks.json`.
pub fn uninstall_cursor_from(home_dir: &Path) -> Result<Option<PathBuf>> {
    let path = home_dir.join(".cursor").join("hooks.json");
    let existing = read_settings(&path)?;
    let cleaned = match existing {
        Some(v) => remove_cursor_hooks_json(v),
        None => return Ok(None),
    };
    write_json_atomic(&path, &cleaned)?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_has_roost_hook_detects_installed() {
        // Claude: after merging, the settings carry a roost hook entry.
        let claude = merge_claude_hooks(None, "/usr/bin/roost");
        assert!(config_has_roost_hook(&claude, is_roost_entry));
        // Codex: after merging, the hooks file carries a roost group.
        let codex = merge_codex_hooks_json(None, "/usr/bin/roost");
        assert!(config_has_roost_hook(&codex, is_codex_roost_group));
    }

    #[test]
    fn config_has_roost_hook_false_when_absent() {
        let empty = serde_json::json!({});
        assert!(!config_has_roost_hook(&empty, is_roost_entry));
        // A hooks object with only third-party / empty events is not "installed".
        let no_roost = serde_json::json!({
            "hooks": { "SessionStart": [], "PostToolUse": [] }
        });
        assert!(!config_has_roost_hook(&no_roost, is_roost_entry));
    }
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
    fn merge_codex_hooks_pre_tool_use_matches_clarify_only() {
        // PreToolUse is registered solely to catch the clarify tool, so its
        // matcher must be exactly request_user_input (not "" / all tools) to
        // avoid firing on every ordinary tool call.
        let result = merge_codex_hooks_json(None, "/usr/bin/roost");
        let arr = codex_event_arr(&result, "PreToolUse");
        let group = &arr[0];
        assert_eq!(
            group.get("matcher").and_then(|v| v.as_str()),
            Some(crate::session::CLARIFY_TOOL),
            "PreToolUse matcher must target the clarify tool only"
        );
        let cmd = codex_group_command(group).expect("inner command must be present");
        assert_eq!(cmd, "/usr/bin/roost hook codex PreToolUse");
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

    // ---------------------------------------------------------------------------
    // DeepSeek / CodeWhale TOML setup tests
    // ---------------------------------------------------------------------------

    /// Count roost `[[hooks.hooks]]` entries in a parsed CodeWhale config.
    fn deepseek_roost_count(config_toml: &str) -> usize {
        let doc: toml_edit::DocumentMut = config_toml.parse().unwrap();
        doc.get("hooks")
            .and_then(|h| h.as_table())
            .and_then(|t| t.get("hooks"))
            .and_then(|i| i.as_array_of_tables())
            .map(|aot| {
                aot.iter()
                    .filter(|t| {
                        t.get("command")
                            .and_then(|c| c.as_str())
                            .map(|c| c.contains(ROOST_MARKER))
                            .unwrap_or(false)
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    #[test]
    fn merge_deepseek_from_empty_registers_all_events_and_enables() {
        let result = merge_deepseek_hooks_toml("", "/usr/bin/roost").unwrap();
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        assert_eq!(
            doc["hooks"]["enabled"].as_bool(),
            Some(true),
            "hooks must be enabled"
        );
        assert_eq!(
            deepseek_roost_count(&result),
            DEEPSEEK_EVENTS.len(),
            "one roost entry per event"
        );
        // Command uses the deepseek family + roost EventKind names.
        assert!(result.contains("/usr/bin/roost hook deepseek SessionStart"));
        assert!(result.contains("/usr/bin/roost hook deepseek UserPromptSubmit"));
        assert!(result.contains("event = \"tool_call_before\""));
    }

    #[test]
    fn merge_deepseek_is_idempotent() {
        let r1 = merge_deepseek_hooks_toml("", "/usr/bin/roost").unwrap();
        let r2 = merge_deepseek_hooks_toml(&r1, "/usr/bin/roost").unwrap();
        assert_eq!(
            deepseek_roost_count(&r2),
            DEEPSEEK_EVENTS.len(),
            "re-running setup must not duplicate roost entries"
        );
    }

    #[test]
    fn merge_deepseek_preserves_user_hooks_and_config() {
        let existing = "\
provider = \"deepseek\"

[hooks]
enabled = true

[[hooks.hooks]]
event = \"session_start\"
command = \"echo my-own-hook\"
";
        let result = merge_deepseek_hooks_toml(existing, "/usr/bin/roost").unwrap();
        // User's top-level config preserved.
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        assert_eq!(doc["provider"].as_str(), Some("deepseek"));
        // User's own hook preserved.
        assert!(result.contains("echo my-own-hook"), "user hook must survive");
        // roost entries added.
        assert_eq!(deepseek_roost_count(&result), DEEPSEEK_EVENTS.len());
    }

    #[test]
    fn remove_deepseek_strips_roost_keeps_user() {
        let installed = merge_deepseek_hooks_toml(
            "[[hooks.hooks]]\nevent = \"session_start\"\ncommand = \"echo my-own-hook\"\n",
            "/usr/bin/roost",
        )
        .unwrap();
        let cleaned = remove_deepseek_hooks_toml(&installed).unwrap();
        assert_eq!(deepseek_roost_count(&cleaned), 0, "roost entries removed");
        assert!(
            cleaned.contains("echo my-own-hook"),
            "user hook must be preserved"
        );
        // User had a hook, so the feature stays enabled.
        let doc: toml_edit::DocumentMut = cleaned.parse().unwrap();
        assert_eq!(doc["hooks"]["enabled"].as_bool(), Some(true));
    }

    #[test]
    fn remove_deepseek_disables_when_no_hooks_left() {
        // roost-only install: after uninstall, the feature is reverted to false.
        let installed = merge_deepseek_hooks_toml("", "/usr/bin/roost").unwrap();
        let cleaned = remove_deepseek_hooks_toml(&installed).unwrap();
        assert_eq!(deepseek_roost_count(&cleaned), 0);
        let doc: toml_edit::DocumentMut = cleaned.parse().unwrap();
        assert_eq!(
            doc["hooks"]["enabled"].as_bool(),
            Some(false),
            "enable flag reverted when no hooks remain"
        );
    }

    #[test]
    fn deepseek_event_names_parse_correctly() {
        use crate::hook::parse_event_kind;
        for (_cw, roost_event) in DEEPSEEK_EVENTS {
            assert!(
                parse_event_kind(roost_event).is_ok(),
                "DeepSeek event '{roost_event}' written by setup must parse"
            );
        }
    }

    #[test]
    fn deepseek_config_has_roost_hook_detects() {
        let installed = merge_deepseek_hooks_toml("", "/usr/bin/roost").unwrap();
        assert!(deepseek_config_has_roost_hook(&installed));
        assert!(!deepseek_config_has_roost_hook("provider = \"deepseek\"\n"));
    }

    // ---------------------------------------------------------------------------
    // Cursor hooks.json tests
    // ---------------------------------------------------------------------------

    fn cursor_event_arr<'a>(result: &'a Value, event: &str) -> &'a Vec<Value> {
        result
            .get("hooks")
            .and_then(|h| h.get(event))
            .and_then(|v| v.as_array())
            .unwrap_or_else(|| panic!("cursor event {event} missing"))
    }

    #[test]
    fn merge_cursor_from_none_registers_all_events_v1() {
        let r = merge_cursor_hooks_json(None, "/usr/bin/roost");
        assert_eq!(r.get("version").and_then(|v| v.as_u64()), Some(1));
        for (cursor_event, roost_event) in CURSOR_EVENTS {
            let arr = cursor_event_arr(&r, cursor_event);
            let cmd = arr[0].get("command").and_then(|c| c.as_str()).unwrap();
            assert_eq!(cmd, format!("/usr/bin/roost hook cursor {roost_event}"));
        }
        // stop must be registered (→ Done).
        assert!(r.get("hooks").and_then(|h| h.get("stop")).is_some());
    }

    #[test]
    fn merge_cursor_is_idempotent() {
        let r1 = merge_cursor_hooks_json(None, "/usr/bin/roost");
        let r2 = merge_cursor_hooks_json(Some(r1.clone()), "/usr/bin/roost");
        for (cursor_event, _) in CURSOR_EVENTS {
            assert_eq!(
                cursor_event_arr(&r2, cursor_event).len(),
                1,
                "cursor event {cursor_event} duplicated on second merge"
            );
        }
    }

    #[test]
    fn merge_cursor_preserves_user_hooks() {
        let existing = json!({
            "version": 1,
            "hooks": { "afterFileEdit": [{ "command": ".cursor/hooks/format.sh" }] }
        });
        let r = merge_cursor_hooks_json(Some(existing), "/usr/bin/roost");
        // User's afterFileEdit hook preserved.
        let edit = cursor_event_arr(&r, "afterFileEdit");
        assert_eq!(
            edit[0].get("command").and_then(|c| c.as_str()),
            Some(".cursor/hooks/format.sh")
        );
        // roost events added.
        assert!(config_has_roost_hook(&r, is_cursor_roost_entry));
    }

    #[test]
    fn remove_cursor_strips_roost_keeps_user() {
        let existing = json!({
            "version": 1,
            "hooks": { "stop": [{ "command": "my-own-stop-hook" }] }
        });
        let installed = merge_cursor_hooks_json(Some(existing), "/usr/bin/roost");
        let cleaned = remove_cursor_hooks_json(installed);
        let stop = cursor_event_arr(&cleaned, "stop");
        assert_eq!(stop.len(), 1, "only the user's stop hook remains");
        assert_eq!(
            stop[0].get("command").and_then(|c| c.as_str()),
            Some("my-own-stop-hook")
        );
        assert!(!config_has_roost_hook(&cleaned, is_cursor_roost_entry));
    }

    #[test]
    fn cursor_event_names_parse_correctly() {
        use crate::hook::parse_event_kind;
        for (_cursor, roost_event) in CURSOR_EVENTS {
            assert!(
                parse_event_kind(roost_event).is_ok(),
                "Cursor event '{roost_event}' written by setup must parse"
            );
        }
    }

    #[test]
    fn cursor_roost_entry_has_no_fail_closed() {
        // A roost hook must never block Cursor: no failClosed (fail-open default).
        let entry = make_cursor_roost_entry("/usr/bin/roost hook cursor Stop");
        assert!(entry.get("failClosed").is_none());
        assert_eq!(entry.get("timeout").and_then(|t| t.as_u64()), Some(5));
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

    // ── Task A1: --sock / --origin烤入 setup 生成的命令行 ─────────────────────

    /// Helper: 从 Claude hooks result 中提取第一条命令字符串.
    fn claude_cmd(result: &Value, event: &str) -> String {
        result
            .get("hooks")
            .and_then(|h| h.get(event))
            .and_then(|v| v.as_array())
            .and_then(|a| a.last()) // roost entry is appended last
            .and_then(|e| e.get("hooks"))
            .and_then(|h| h.as_array())
            .and_then(|h| h.first())
            .and_then(|h| h.get("command"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string()
    }

    /// Helper: 从 Codex hooks result 提取第一条命令字符串.
    fn codex_cmd(result: &Value, event: &str) -> String {
        result
            .get("hooks")
            .and_then(|h| h.get(event))
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|g| g.get("hooks"))
            .and_then(|h| h.as_array())
            .and_then(|h| h.first())
            .and_then(|h| h.get("command"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string()
    }

    /// Helper: 从 Cursor hooks result 提取第一条命令字符串.
    fn cursor_cmd(result: &Value, cursor_event: &str) -> String {
        result
            .get("hooks")
            .and_then(|h| h.get(cursor_event))
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|e| e.get("command"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string()
    }

    // No flags → commands identical to legacy format (zero regression).
    #[test]
    fn setup_no_flags_claude_command_unchanged() {
        let result = merge_claude_hooks_with_flags(None, "/usr/bin/roost", None, None);
        let cmd = claude_cmd(&result, "SessionStart");
        assert_eq!(cmd, "/usr/bin/roost hook claude SessionStart");
    }

    #[test]
    fn setup_no_flags_codex_command_unchanged() {
        let result = merge_codex_hooks_json_with_flags(None, "/usr/bin/roost", None, None);
        let cmd = codex_cmd(&result, "SessionStart");
        assert_eq!(cmd, "/usr/bin/roost hook codex SessionStart");
    }

    #[test]
    fn setup_no_flags_cursor_command_unchanged() {
        let result = merge_cursor_hooks_json_with_flags(None, "/usr/bin/roost", None, None);
        let cmd = cursor_cmd(&result, "sessionStart");
        assert_eq!(cmd, "/usr/bin/roost hook cursor SessionStart");
    }

    // Both --sock and --origin are baked into generated commands.
    #[test]
    fn setup_with_sock_and_origin_claude() {
        let result = merge_claude_hooks_with_flags(
            None,
            "/usr/bin/roost",
            Some("~/.roost/hub.sock"),
            Some("devbox1"),
        );
        let cmd = claude_cmd(&result, "SessionStart");
        assert!(
            cmd.contains("--sock ~/.roost/hub.sock"),
            "sock flag missing: {cmd}"
        );
        assert!(
            cmd.contains("--origin devbox1"),
            "origin flag missing: {cmd}"
        );
        assert!(
            cmd.ends_with("claude SessionStart"),
            "family/event suffix missing: {cmd}"
        );
    }

    #[test]
    fn setup_with_sock_and_origin_codex() {
        let result = merge_codex_hooks_json_with_flags(
            None,
            "/usr/bin/roost",
            Some("/tmp/hub.sock"),
            Some("remote1"),
        );
        let cmd = codex_cmd(&result, "SessionStart");
        assert!(cmd.contains("--sock /tmp/hub.sock"), "sock flag missing: {cmd}");
        assert!(cmd.contains("--origin remote1"), "origin flag missing: {cmd}");
        assert!(cmd.ends_with("codex SessionStart"), "family/event suffix: {cmd}");
    }

    #[test]
    fn setup_with_sock_and_origin_cursor() {
        let result = merge_cursor_hooks_json_with_flags(
            None,
            "/usr/bin/roost",
            Some("/tmp/hub.sock"),
            Some("remote1"),
        );
        let cmd = cursor_cmd(&result, "sessionStart");
        assert!(cmd.contains("--sock /tmp/hub.sock"), "sock flag missing: {cmd}");
        assert!(cmd.contains("--origin remote1"), "origin flag missing: {cmd}");
        assert!(cmd.ends_with("cursor SessionStart"), "family/event suffix: {cmd}");
    }

    // Only --sock (no --origin).
    #[test]
    fn setup_with_sock_only_claude() {
        let result = merge_claude_hooks_with_flags(
            None,
            "/usr/bin/roost",
            Some("/tmp/hub.sock"),
            None,
        );
        let cmd = claude_cmd(&result, "SessionStart");
        assert!(cmd.contains("--sock /tmp/hub.sock"), "sock flag missing: {cmd}");
        assert!(!cmd.contains("--origin"), "origin should not appear: {cmd}");
    }

    // Only --origin (no --sock).
    #[test]
    fn setup_with_origin_only_codex() {
        let result =
            merge_codex_hooks_json_with_flags(None, "/usr/bin/roost", None, Some("remote1"));
        let cmd = codex_cmd(&result, "SessionStart");
        assert!(!cmd.contains("--sock"), "sock should not appear: {cmd}");
        assert!(cmd.contains("--origin remote1"), "origin flag missing: {cmd}");
    }

    // DeepSeek TOML: --sock / --origin烤进命令.
    #[test]
    fn setup_no_flags_deepseek_command_unchanged() {
        let result = merge_deepseek_hooks_toml_with_flags("", "/usr/bin/roost", None, None).unwrap();
        assert!(
            result.contains("/usr/bin/roost hook deepseek SessionStart"),
            "baseline deepseek cmd missing"
        );
        assert!(!result.contains("--sock"), "unexpected --sock in baseline");
        assert!(!result.contains("--origin"), "unexpected --origin in baseline");
    }

    #[test]
    fn setup_with_flags_deepseek() {
        let result = merge_deepseek_hooks_toml_with_flags(
            "",
            "/usr/bin/roost",
            Some("/tmp/hub.sock"),
            Some("remote1"),
        )
        .unwrap();
        assert!(
            result.contains("--sock /tmp/hub.sock"),
            "sock flag missing in deepseek toml: {result}"
        );
        assert!(
            result.contains("--origin remote1"),
            "origin flag missing in deepseek toml: {result}"
        );
    }

    // Idempotent with flags: second merge doesn't duplicate entries.
    #[test]
    fn setup_with_flags_claude_idempotent() {
        let r1 = merge_claude_hooks_with_flags(None, "/usr/bin/roost", Some("/tmp/h.sock"), Some("box1"));
        let r2 = merge_claude_hooks_with_flags(Some(r1.clone()), "/usr/bin/roost", Some("/tmp/h.sock"), Some("box1"));
        for event in CLAUDE_EVENTS {
            let len1 = r1.get("hooks").and_then(|h| h.get(*event)).and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            let len2 = r2.get("hooks").and_then(|h| h.get(*event)).and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            assert_eq!(len1, len2, "claude event {event} duplicated after second merge with flags");
        }
    }
}
