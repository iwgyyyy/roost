use std::path::PathBuf;

use roost::setup::{
    install_claude_into, merge_claude_hooks, remove_claude_hooks, uninstall_claude_from,
};

fn make_temp_home() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn claude_settings_path(home: &std::path::Path) -> PathBuf {
    home.join(".claude").join("settings.json")
}

fn write_settings(path: &PathBuf, value: &serde_json::Value) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let content = serde_json::to_string_pretty(value).unwrap();
    std::fs::write(path, content).unwrap();
}

fn read_settings(path: &PathBuf) -> serde_json::Value {
    let content = std::fs::read_to_string(path).unwrap();
    serde_json::from_str(&content).unwrap()
}

const ROOST_BIN: &str = "/usr/local/bin/roost";

const CLAUDE_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "Stop",
    "SessionEnd",
];

fn has_roost_hook(settings: &serde_json::Value, event: &str) -> bool {
    settings
        .get("hooks")
        .and_then(|h| h.get(event))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter().any(|entry| {
                entry
                    .get("hooks")
                    .and_then(|h| h.as_array())
                    .map(|hooks| {
                        hooks.iter().any(|h| {
                            h.get("command")
                                .and_then(|c| c.as_str())
                                .map(|c| c.contains("roost hook "))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn count_roost_hooks(settings: &serde_json::Value, event: &str) -> usize {
    settings
        .get("hooks")
        .and_then(|h| h.get(event))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|entry| {
                    entry
                        .get("hooks")
                        .and_then(|h| h.as_array())
                        .map(|hooks| {
                            hooks.iter().any(|h| {
                                h.get("command")
                                    .and_then(|c| c.as_str())
                                    .map(|c| c.contains("roost hook "))
                                    .unwrap_or(false)
                            })
                        })
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn install_writes_roost_hooks_to_settings() {
    let home = make_temp_home();
    let settings_path = claude_settings_path(home.path());

    // Start with empty settings
    let initial = serde_json::json!({});
    write_settings(&settings_path, &initial);

    // Apply merge
    let existing = Some(read_settings(&settings_path));
    let merged = merge_claude_hooks(existing, ROOST_BIN);
    write_settings(&settings_path, &merged);

    let result = read_settings(&settings_path);
    for event in CLAUDE_EVENTS {
        assert!(
            has_roost_hook(&result, event),
            "event {event} missing roost hook"
        );
    }
}

#[test]
fn install_twice_is_idempotent() {
    let merged1 = merge_claude_hooks(None, ROOST_BIN);
    let merged2 = merge_claude_hooks(Some(merged1.clone()), ROOST_BIN);

    for event in CLAUDE_EVENTS {
        let count1 = count_roost_hooks(&merged1, event);
        let count2 = count_roost_hooks(&merged2, event);
        assert_eq!(
            count1, count2,
            "event {event}: second install should not create duplicates ({count1} → {count2})"
        );
    }
}

#[test]
fn install_preserves_user_hooks() {
    let existing = serde_json::json!({
        "hooks": {
            "SessionStart": [
                {
                    "matcher": "",
                    "hooks": [{"type": "command", "command": "user-custom-hook"}]
                }
            ]
        },
        "model": "claude-3-5-sonnet"
    });

    let merged = merge_claude_hooks(Some(existing), ROOST_BIN);

    // User hook still present
    let arr = merged
        .get("hooks")
        .and_then(|h| h.get("SessionStart"))
        .and_then(|v| v.as_array())
        .unwrap();

    let has_user = arr.iter().any(|e| {
        e.get("hooks")
            .and_then(|h| h.as_array())
            .and_then(|h| h.first())
            .and_then(|h| h.get("command"))
            .and_then(|c| c.as_str())
            == Some("user-custom-hook")
    });
    assert!(has_user, "user hook should be preserved after install");

    // Other settings preserved
    assert_eq!(
        merged.get("model").and_then(|v| v.as_str()),
        Some("claude-3-5-sonnet")
    );
}

#[test]
fn uninstall_removes_roost_hooks_cleanly() {
    // Start fresh, install, then uninstall
    let installed = merge_claude_hooks(None, ROOST_BIN);
    let uninstalled = remove_claude_hooks(installed);

    for event in CLAUDE_EVENTS {
        assert_eq!(
            count_roost_hooks(&uninstalled, event),
            0,
            "event {event} still has roost hook after uninstall"
        );
    }
}

#[test]
fn uninstall_preserves_user_hooks() {
    let existing = serde_json::json!({
        "hooks": {
            "SessionStart": [
                {
                    "matcher": "",
                    "hooks": [{"type": "command", "command": "my-other-tool"}]
                }
            ]
        }
    });

    let installed = merge_claude_hooks(Some(existing), ROOST_BIN);
    let uninstalled = remove_claude_hooks(installed);

    let arr = uninstalled
        .get("hooks")
        .and_then(|h| h.get("SessionStart"))
        .and_then(|v| v.as_array())
        .unwrap();

    assert_eq!(arr.len(), 1, "user hook should remain after uninstall");
    let cmd = arr[0]
        .get("hooks")
        .and_then(|h| h.as_array())
        .and_then(|h| h.first())
        .and_then(|h| h.get("command"))
        .and_then(|c| c.as_str())
        .unwrap();
    assert_eq!(cmd, "my-other-tool");
}

#[test]
fn uninstall_on_fresh_settings_does_not_crash() {
    let empty = serde_json::json!({});
    let result = remove_claude_hooks(empty);
    // Should return cleanly with no hooks section or empty hooks
    assert!(!result.is_null());
}

// ---------------------------------------------------------------------------
// Real-IO integration tests using install_claude() / uninstall_claude()
// These tests mutate HOME via set_var and must run single-threaded.
// ---------------------------------------------------------------------------

/// Call install_claude_into() with a temporary home directory and verify that
/// .claude/settings.json is created and contains roost hook entries.
#[test]
fn real_io_install_creates_settings_with_roost_hooks() {
    let tmp_home = make_temp_home();
    let settings_path = claude_settings_path(tmp_home.path());

    let result = install_claude_into(tmp_home.path(), ROOST_BIN);
    assert!(
        result.is_ok(),
        "install_claude_into() failed: {:?}",
        result.err()
    );

    // File must exist
    assert!(settings_path.exists(), "settings.json was not created");

    let settings = read_settings(&settings_path);
    for event in CLAUDE_EVENTS {
        assert!(
            has_roost_hook(&settings, event),
            "event {event} missing roost hook after real install"
        );
    }
}

/// Calling install_claude_into() twice must not create duplicate hook entries.
#[test]
fn real_io_install_twice_is_idempotent() {
    let tmp_home = make_temp_home();
    let settings_path = claude_settings_path(tmp_home.path());

    install_claude_into(tmp_home.path(), ROOST_BIN).expect("first install");
    install_claude_into(tmp_home.path(), ROOST_BIN).expect("second install");

    let settings = read_settings(&settings_path);
    for event in CLAUDE_EVENTS {
        let count = count_roost_hooks(&settings, event);
        assert_eq!(
            count, 1,
            "event {event}: expected exactly 1 roost hook after idempotent install, got {count}"
        );
    }
}

/// After install + uninstall, roost entries are gone but user entries survive.
#[test]
fn real_io_uninstall_removes_roost_and_preserves_user_hooks() {
    let tmp_home = make_temp_home();
    let settings_path = claude_settings_path(tmp_home.path());

    // Write initial settings with a user-owned hook
    let initial = serde_json::json!({
        "model": "claude-opus-4-5",
        "hooks": {
            "SessionStart": [
                {
                    "matcher": "",
                    "hooks": [{"type": "command", "command": "user-tool start"}]
                }
            ]
        }
    });
    write_settings(&settings_path, &initial);

    install_claude_into(tmp_home.path(), ROOST_BIN).expect("install");
    uninstall_claude_from(tmp_home.path()).expect("uninstall");

    let settings = read_settings(&settings_path);

    // Roost hooks must be gone
    for event in CLAUDE_EVENTS {
        assert_eq!(
            count_roost_hooks(&settings, event),
            0,
            "event {event} still has roost hook after uninstall"
        );
    }

    // User hook must survive
    let arr = settings
        .get("hooks")
        .and_then(|h| h.get("SessionStart"))
        .and_then(|v| v.as_array())
        .expect("SessionStart hooks array should exist");

    let has_user = arr.iter().any(|e| {
        e.get("hooks")
            .and_then(|h| h.as_array())
            .and_then(|h| h.first())
            .and_then(|h| h.get("command"))
            .and_then(|c| c.as_str())
            == Some("user-tool start")
    });
    assert!(
        has_user,
        "user hook was removed by uninstall — should be preserved"
    );

    // Other settings (model) must also survive
    assert_eq!(
        settings.get("model").and_then(|v| v.as_str()),
        Some("claude-opus-4-5"),
        "non-hook settings should survive uninstall"
    );
}
