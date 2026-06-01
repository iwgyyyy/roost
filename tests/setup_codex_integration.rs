/// Integration tests for Codex hooks installation / uninstallation.
/// Uses a temporary directory to avoid touching the real ~/.codex.
use std::path::PathBuf;

use roost::setup::{install_codex_into, uninstall_codex_from};

const ROOST_BIN: &str = "/usr/local/bin/roost";

fn make_temp_codex_home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    // Create the .codex subdirectory to simulate Codex being installed.
    std::fs::create_dir_all(dir.path().join(".codex")).unwrap();
    dir
}

fn hooks_path(home: &std::path::Path) -> PathBuf {
    home.join(".codex").join("hooks.json")
}

fn config_path(home: &std::path::Path) -> PathBuf {
    home.join(".codex").join("config.toml")
}

fn read_json(path: &PathBuf) -> serde_json::Value {
    let content = std::fs::read_to_string(path).unwrap();
    serde_json::from_str(&content).unwrap()
}

fn read_text(path: &PathBuf) -> String {
    std::fs::read_to_string(path).unwrap()
}

const CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "Stop",
    "PostToolUse",
    "PermissionRequest",
];

/// Check if hooks.json (nested schema) has a roost-managed group for the given event.
/// Looks inside `hooks.<event>` for a group whose inner `hooks[0].command` matches.
fn has_roost_codex_hook(root: &serde_json::Value, event: &str, bin: &str) -> bool {
    let expected_cmd = format!("{} hook codex {}", bin, event);
    root.get("hooks")
        .and_then(|h| h.get(event))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter().any(|group| {
                group
                    .get("hooks")
                    .and_then(|h| h.as_array())
                    .and_then(|h| h.first())
                    .and_then(|h| h.get("command"))
                    .and_then(|c| c.as_str())
                    .map(|c| c == expected_cmd)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Count how many roost-managed groups exist for the given event (idempotency check).
fn count_roost_codex_hooks(root: &serde_json::Value, event: &str, bin: &str) -> usize {
    let expected_cmd = format!("{} hook codex {}", bin, event);
    root.get("hooks")
        .and_then(|h| h.get(event))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|group| {
                    group
                        .get("hooks")
                        .and_then(|h| h.as_array())
                        .and_then(|h| h.first())
                        .and_then(|h| h.get("command"))
                        .and_then(|c| c.as_str())
                        .map(|c| c == expected_cmd)
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0)
}

/// After install, hooks.json contains all roost codex hook entries.
#[test]
fn install_writes_all_codex_hooks() {
    let home = make_temp_codex_home();
    let result = install_codex_into(home.path(), ROOST_BIN).unwrap();
    assert!(
        result.is_some(),
        "install_codex_into should return Some(dir) when .codex exists"
    );

    let hooks = read_json(&hooks_path(home.path()));
    for event in CODEX_EVENTS {
        assert!(
            has_roost_codex_hook(&hooks, event, ROOST_BIN),
            "codex event {event} missing after install"
        );
    }
}

/// After install, config.toml has `[features] hooks = true`.
#[test]
fn install_enables_hooks_feature_in_config_toml() {
    let home = make_temp_codex_home();
    install_codex_into(home.path(), ROOST_BIN).unwrap();

    let toml_content = read_text(&config_path(home.path()));
    let doc: toml_edit::DocumentMut = toml_content.parse().unwrap();
    assert_eq!(
        doc["features"]["hooks"].as_bool(),
        Some(true),
        "config.toml should have [features] hooks = true"
    );
}

/// Install twice must not duplicate hook entries.
#[test]
fn install_twice_is_idempotent() {
    let home = make_temp_codex_home();
    install_codex_into(home.path(), ROOST_BIN).unwrap();
    install_codex_into(home.path(), ROOST_BIN).unwrap();

    let root = read_json(&hooks_path(home.path()));
    for event in CODEX_EVENTS {
        let count = count_roost_codex_hooks(&root, event, ROOST_BIN);
        assert_eq!(
            count, 1,
            "event {event}: expected 1 roost hook group, got {count}"
        );
    }
}

/// After install + uninstall, roost hooks are gone.
#[test]
fn uninstall_removes_codex_hooks() {
    let home = make_temp_codex_home();
    install_codex_into(home.path(), ROOST_BIN).unwrap();
    uninstall_codex_from(home.path(), ROOST_BIN).unwrap();

    let hooks = read_json(&hooks_path(home.path()));
    for event in CODEX_EVENTS {
        assert!(
            !has_roost_codex_hook(&hooks, event, ROOST_BIN),
            "codex event {event} still has roost hook after uninstall"
        );
    }
}

/// Uninstall preserves user-defined entries in hooks.json (correct nested schema).
#[test]
fn uninstall_preserves_user_codex_hooks() {
    let home = make_temp_codex_home();

    // Write a user-owned hook using the correct Codex nested schema.
    let initial = serde_json::json!({
        "hooks": {
            "SessionStart": [
                {
                    "hooks": [{"type": "command", "command": "user-hook-tool", "timeout": 10}]
                }
            ]
        }
    });
    let path = hooks_path(home.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

    install_codex_into(home.path(), ROOST_BIN).unwrap();
    uninstall_codex_from(home.path(), ROOST_BIN).unwrap();

    let root = read_json(&path);
    let arr = root
        .get("hooks")
        .and_then(|h| h.get("SessionStart"))
        .and_then(|v| v.as_array())
        .expect("hooks.SessionStart must exist");
    let has_user = arr.iter().any(|group| {
        group
            .get("hooks")
            .and_then(|h| h.as_array())
            .and_then(|h| h.first())
            .and_then(|h| h.get("command"))
            .and_then(|c| c.as_str())
            == Some("user-hook-tool")
    });
    assert!(has_user, "user hook group should survive uninstall");
}

/// After uninstall, config.toml has `hooks = false`.
#[test]
fn uninstall_disables_hooks_feature() {
    let home = make_temp_codex_home();
    install_codex_into(home.path(), ROOST_BIN).unwrap();
    uninstall_codex_from(home.path(), ROOST_BIN).unwrap();

    let toml_content = read_text(&config_path(home.path()));
    let doc: toml_edit::DocumentMut = toml_content.parse().unwrap();
    assert_eq!(
        doc["features"]["hooks"].as_bool(),
        Some(false),
        "config.toml should have [features] hooks = false after uninstall"
    );
}

/// Uninstall preserves other TOML config keys.
#[test]
fn uninstall_preserves_user_toml_settings() {
    let home = make_temp_codex_home();

    // Pre-populate config.toml with user settings.
    let initial_toml = "[model]\nname = \"o4-mini\"\n\n[features]\nsandbox = true\n";
    let cfg = config_path(home.path());
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    std::fs::write(&cfg, initial_toml).unwrap();

    install_codex_into(home.path(), ROOST_BIN).unwrap();
    uninstall_codex_from(home.path(), ROOST_BIN).unwrap();

    let toml_content = read_text(&cfg);
    let doc: toml_edit::DocumentMut = toml_content.parse().unwrap();
    assert_eq!(
        doc["model"]["name"].as_str(),
        Some("o4-mini"),
        "model.name should be preserved"
    );
    assert_eq!(
        doc["features"]["sandbox"].as_bool(),
        Some(true),
        "features.sandbox should be preserved"
    );
}

/// Returns None (without error) when the Codex directory does not exist.
#[test]
fn install_returns_none_when_codex_not_installed() {
    let home = tempfile::tempdir().unwrap();
    // No .codex directory here.
    let result = install_codex_into(home.path(), ROOST_BIN).unwrap();
    assert!(
        result.is_none(),
        "should return None when .codex dir does not exist"
    );
}
