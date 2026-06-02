//! Notification layer: pure logic + macOS side-effects.
//!
//! Provides edge-detection (`trigger_for_transition`), state→sound mapping
//! (`sound_for`), notification text generation (`notification_text`), sound-
//! file path lookup (`sound_file`), and a best-effort macOS notifier (`fire`).
//!
//! All pure functions are unit-tested below. `fire` is macOS-only; on other
//! platforms it is a no-op. No panics, no blocking — spawn failures are silently
//! ignored (fail-open).

use std::path::PathBuf;

use crate::protocol::AgentFamily;
use crate::session::AgentState;

// ── Pure logic ────────────────────────────────────────────────────────────────

/// Returns `Some(new)` when the transition `old → new` should trigger a
/// notification: the states must differ, and `new` must be one of the
/// actionable states (`Question`, `Approval`, `Done`).
pub fn trigger_for_transition(old: AgentState, new: AgentState) -> Option<AgentState> {
    if old != new && matches!(new, AgentState::Question | AgentState::Approval | AgentState::Done) {
        Some(new)
    } else {
        None
    }
}

/// Maps an actionable agent state to its macOS system sound name.
///
/// - `Question`  → `"Submarine"` (needs your input)
/// - `Approval`  → `"Bottle"`   (needs permission)
/// - `Done`      → `"Ping"`     (finished)
/// - everything else → `None`
pub fn sound_for(state: AgentState) -> Option<&'static str> {
    match state {
        AgentState::Question => Some("Submarine"),
        AgentState::Approval => Some("Bottle"),
        AgentState::Done => Some("Ping"),
        _ => None,
    }
}

/// Returns the `(title, body)` pair for a macOS notification.
///
/// `title` is always `"roost"`.
///
/// `body` format:
/// - With `activity`: `"<family label> · <name> — <activity>"`
/// - Without `activity` and state is `Done`: `"<family label> · <name> 完成"`
/// - Without `activity` otherwise: `"<family label> · <name> 需要你"`
///
/// **Precondition**: `state` should be one of `Question`, `Approval`, or
/// `Done`. This is guaranteed by `trigger_for_transition`, which is the only
/// upstream caller that decides whether a notification fires.
pub fn notification_text(
    family: &AgentFamily,
    name: &str,
    activity: Option<&str>,
    state: AgentState,
) -> (String, String) {
    let title = "roost".to_string();
    let label = family.display_label();
    let body = match activity {
        Some(act) => format!("{label} · {name} — {act}"),
        None => {
            if matches!(state, AgentState::Done) {
                format!("{label} · {name} 完成")
            } else {
                format!("{label} · {name} 需要你")
            }
        }
    };
    (title, body)
}

/// Returns the path to a macOS system sound file.
///
/// e.g. `sound_file("Submarine")` → `/System/Library/Sounds/Submarine.aiff`
///
/// **Precondition**: `name` must be a bare sound name with no path separators
/// (`/`) or traversal sequences (`..`). In practice `name` comes from
/// `sound_for`, which returns only `"Submarine"`, `"Bottle"`, or `"Ping"`.
pub fn sound_file(name: &str) -> PathBuf {
    PathBuf::from(format!("/System/Library/Sounds/{name}.aiff"))
}

/// Wraps a string as an AppleScript string literal, escaping `\`, `"`, `\n`,
/// and `\r`. AppleScript string literals cannot contain bare newlines or
/// carriage returns — they cause `osascript` to silently fail.
fn applescript_str(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("\"{escaped}\"")
}

// ── macOS side-effects ────────────────────────────────────────────────────────

/// Fire a best-effort macOS notification and/or sound for the given agent state.
///
/// - `banner`: if true, spawn `osascript` to display a notification banner.
/// - `sound`: if true, look up the state's sound, check the file exists, then
///   spawn `afplay` (skipped silently if no file or no sound for state).
///
/// All spawns are detached (stdin/stdout/stderr → null, no `.wait()`).
/// Spawn failures are silently ignored. Non-macOS: no-op.
#[cfg(target_os = "macos")]
pub fn fire(
    family: &AgentFamily,
    name: &str,
    activity: Option<&str>,
    state: AgentState,
    banner: bool,
    sound: bool,
) {
    use std::process::{Command, Stdio};

    let (title, body) = notification_text(family, name, activity, state);

    if banner {
        let script = format!(
            "display notification {} with title {}",
            applescript_str(&body),
            applescript_str(&title),
        );
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }

    if sound && let Some(snd_name) = sound_for(state) {
        let path = sound_file(snd_name);
        if path.exists() {
            let _ = Command::new("afplay")
                .arg(&path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
    }
}

/// Non-macOS no-op.
#[cfg(not(target_os = "macos"))]
pub fn fire(
    _family: &AgentFamily,
    _name: &str,
    _activity: Option<&str>,
    _state: AgentState,
    _banner: bool,
    _sound: bool,
) {
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AgentFamily;
    use crate::session::AgentState;

    // ── trigger_for_transition ────────────────────────────────────────────────

    #[test]
    fn working_to_question_triggers() {
        assert_eq!(
            trigger_for_transition(AgentState::Working, AgentState::Question),
            Some(AgentState::Question)
        );
    }

    #[test]
    fn idle_to_approval_triggers() {
        assert_eq!(
            trigger_for_transition(AgentState::Idle, AgentState::Approval),
            Some(AgentState::Approval)
        );
    }

    #[test]
    fn working_to_done_triggers() {
        assert_eq!(
            trigger_for_transition(AgentState::Working, AgentState::Done),
            Some(AgentState::Done)
        );
    }

    #[test]
    fn question_to_question_no_trigger() {
        // Same state → no edge.
        assert_eq!(
            trigger_for_transition(AgentState::Question, AgentState::Question),
            None
        );
    }

    #[test]
    fn working_to_idle_no_trigger() {
        // Idle is not a notification-worthy target state.
        assert_eq!(
            trigger_for_transition(AgentState::Working, AgentState::Idle),
            None
        );
    }

    #[test]
    fn done_to_done_no_trigger() {
        assert_eq!(
            trigger_for_transition(AgentState::Done, AgentState::Done),
            None
        );
    }

    #[test]
    fn approval_to_question_triggers() {
        // Switching between two needs-input states still counts as an edge.
        assert_eq!(
            trigger_for_transition(AgentState::Approval, AgentState::Question),
            Some(AgentState::Question)
        );
    }

    // ── sound_for ─────────────────────────────────────────────────────────────

    #[test]
    fn sound_for_question() {
        assert_eq!(sound_for(AgentState::Question), Some("Submarine"));
    }

    #[test]
    fn sound_for_approval() {
        assert_eq!(sound_for(AgentState::Approval), Some("Bottle"));
    }

    #[test]
    fn sound_for_done() {
        assert_eq!(sound_for(AgentState::Done), Some("Ping"));
    }

    #[test]
    fn sound_for_working_is_none() {
        assert_eq!(sound_for(AgentState::Working), None);
    }

    #[test]
    fn sound_for_idle_is_none() {
        assert_eq!(sound_for(AgentState::Idle), None);
    }

    // ── notification_text ─────────────────────────────────────────────────────

    #[test]
    fn notification_text_title_is_always_roost() {
        let (title, _) = notification_text(
            &AgentFamily::Claude,
            "my-repo",
            None,
            AgentState::Done,
        );
        assert_eq!(title, "roost");
    }

    #[test]
    fn notification_text_with_activity_contains_dash_separator() {
        let (_, body) = notification_text(
            &AgentFamily::Claude,
            "my-repo",
            Some("editing src/main.rs"),
            AgentState::Question,
        );
        // Should contain family label, name, and activity separated by " — "
        assert!(
            body.contains("Claude Code"),
            "body should contain family label: {body:?}"
        );
        assert!(
            body.contains("my-repo"),
            "body should contain name: {body:?}"
        );
        assert!(
            body.contains(" — editing src/main.rs"),
            "body should contain activity with separator: {body:?}"
        );
    }

    #[test]
    fn notification_text_no_activity_done_says_wan_cheng() {
        let (_, body) = notification_text(
            &AgentFamily::Codex,
            "proj",
            None,
            AgentState::Done,
        );
        assert!(
            body.contains("完成"),
            "Done without activity should say '完成': {body:?}"
        );
        assert!(
            body.contains("Codex"),
            "body should contain family label: {body:?}"
        );
    }

    #[test]
    fn notification_text_no_activity_question_says_xu_yao_ni() {
        let (_, body) = notification_text(
            &AgentFamily::Claude,
            "proj",
            None,
            AgentState::Question,
        );
        assert!(
            body.contains("需要你"),
            "Question without activity should say '需要你': {body:?}"
        );
    }

    #[test]
    fn notification_text_no_activity_approval_says_xu_yao_ni() {
        let (_, body) = notification_text(
            &AgentFamily::Claude,
            "proj",
            None,
            AgentState::Approval,
        );
        assert!(
            body.contains("需要你"),
            "Approval without activity should say '需要你': {body:?}"
        );
    }

    #[test]
    fn notification_text_uses_display_label() {
        // Deepseek family should use its display_label in the body.
        let (_, body) = notification_text(
            &AgentFamily::Deepseek,
            "my-proj",
            None,
            AgentState::Done,
        );
        assert!(
            body.starts_with("DeepSeek"),
            "body should start with family display label: {body:?}"
        );
    }

    // ── applescript_str ───────────────────────────────────────────────────────

    #[test]
    fn applescript_str_wraps_in_double_quotes() {
        assert_eq!(applescript_str("hello"), r#""hello""#);
    }

    #[test]
    fn applescript_str_escapes_double_quotes() {
        // A string containing a " must have it escaped as \"
        let result = applescript_str(r#"say "hi""#);
        assert_eq!(result, r#""say \"hi\"""#);
    }

    #[test]
    fn applescript_str_escapes_backslash() {
        // A backslash must become \\
        let result = applescript_str(r"path\to\file");
        assert_eq!(result, r#""path\\to\\file""#);
    }

    #[test]
    fn applescript_str_escapes_backslash_then_quote() {
        // A \" sequence: backslash becomes \\, then quote becomes \"
        let result = applescript_str("a\\\"b");
        assert_eq!(result, r#""a\\\"b""#);
    }

    #[test]
    fn applescript_str_empty_string() {
        assert_eq!(applescript_str(""), r#""""#);
    }

    #[test]
    fn applescript_str_escapes_newline() {
        // Bare \n must become the two-character sequence \n inside the literal.
        let result = applescript_str("line1\nline2");
        assert_eq!(result, r#""line1\nline2""#);
    }

    #[test]
    fn applescript_str_escapes_carriage_return() {
        // Bare \r must become the two-character sequence \r inside the literal.
        let result = applescript_str("line1\rline2");
        assert_eq!(result, r#""line1\rline2""#);
    }

    // ── sound_file ────────────────────────────────────────────────────────────

    #[test]
    fn sound_file_returns_correct_path() {
        let p = sound_file("Submarine");
        assert_eq!(p, PathBuf::from("/System/Library/Sounds/Submarine.aiff"));
    }

    #[test]
    fn sound_file_ping() {
        let p = sound_file("Ping");
        assert_eq!(p, PathBuf::from("/System/Library/Sounds/Ping.aiff"));
    }
}
