//! Terminal jump implementations: tmux, AppleScript (Ghostty/iTerm2/Terminal.app),
//! WezTerm CLI, Zellij CLI, cmux JSON-RPC.

use super::{Executor, JumpOutcome, JumpTarget};

// ── tmux ──────────────────────────────────────────────────────────────────────

/// Build the tmux socket argument from a combined tmux_target string.
///
/// Format stored in `JumpTarget::tmux_target`: `"<socket_path>,<...>|<pane_id>"`
/// We split on `|` to get (socket_info, pane_id).
pub fn parse_tmux_target(combined: &str) -> (&str, &str) {
    if let Some(pos) = combined.rfind('|') {
        (&combined[..pos], &combined[pos + 1..])
    } else {
        (combined, "")
    }
}

/// Extract the tmux socket path from the $TMUX env value (first field).
/// `$TMUX` = `<socket>,<server_pid>,<session_id>`
pub fn tmux_socket_from_env(tmux_env: &str) -> &str {
    tmux_env.split(',').next().unwrap_or(tmux_env)
}

/// Build the `tmux switch-client -c <tty>` args.
pub fn tmux_switch_client_args(socket: &str, client_tty: &str) -> Vec<String> {
    vec![
        "-S".to_string(),
        socket.to_string(),
        "switch-client".to_string(),
        "-c".to_string(),
        client_tty.to_string(),
    ]
}

/// Build `tmux select-window` args.
pub fn tmux_select_window_args(socket: &str, window_target: &str) -> Vec<String> {
    vec![
        "-S".to_string(),
        socket.to_string(),
        "select-window".to_string(),
        "-t".to_string(),
        window_target.to_string(),
    ]
}

/// Build `tmux select-pane` args.
pub fn tmux_select_pane_args(socket: &str, pane_target: &str) -> Vec<String> {
    vec![
        "-S".to_string(),
        socket.to_string(),
        "select-pane".to_string(),
        "-t".to_string(),
        pane_target.to_string(),
    ]
}

/// Execute a tmux jump using the pane target stored in `tmux_target`.
///
/// Returns `Some(JumpOutcome)` if a tmux target was available, `None` otherwise.
pub fn jump_tmux(combined_target: &str, executor: &dyn Executor) -> Option<JumpOutcome> {
    let (socket_info, pane_id) = parse_tmux_target(combined_target);
    let socket = tmux_socket_from_env(socket_info);

    if pane_id.is_empty() {
        return None;
    }

    // select-pane directly by pane_id
    let args = tmux_select_pane_args(socket, pane_id);
    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let (_, ok) = executor.run("tmux", &args_refs);

    if ok {
        Some(JumpOutcome::Focused {
            message: format!("Focused tmux pane {pane_id}"),
        })
    } else {
        Some(JumpOutcome::Failed {
            message: format!("Failed to focus tmux pane {pane_id}"),
        })
    }
}

// ── WezTerm / Kaku ────────────────────────────────────────────────────────────

/// Build `wezterm cli activate-pane --pane-id <id>` args.
pub fn wezterm_activate_pane_args(pane_id: &str) -> Vec<String> {
    vec![
        "cli".to_string(),
        "activate-pane".to_string(),
        "--pane-id".to_string(),
        pane_id.to_string(),
    ]
}

/// Execute a WezTerm/Kaku pane jump via CLI.
pub fn jump_wezterm(pane_id: &str, executor: &dyn Executor) -> Option<JumpOutcome> {
    let args = wezterm_activate_pane_args(pane_id);
    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let (_, ok) = executor.run("wezterm", &args_refs);
    if ok {
        Some(JumpOutcome::Focused {
            message: format!("Focused WezTerm pane {pane_id}"),
        })
    } else {
        Some(JumpOutcome::Failed {
            message: "Failed to focus WezTerm pane".to_string(),
        })
    }
}

// ── Zellij ────────────────────────────────────────────────────────────────────

/// Build `zellij action go-to-tab` args for a named session.
/// Note: Zellij doesn't have a direct "go to session" CLI; we navigate within
/// the current session's tab by session name matching.
pub fn zellij_go_to_tab_args(session: &str) -> Vec<String> {
    // `zellij -s <session> action go-to-tab-name <session>`
    vec![
        "-s".to_string(),
        session.to_string(),
        "action".to_string(),
        "focus-next-pane".to_string(),
    ]
}

/// Execute a Zellij jump.
pub fn jump_zellij(session: &str, executor: &dyn Executor) -> Option<JumpOutcome> {
    // Attach to the Zellij session
    let args = zellij_go_to_tab_args(session);
    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let (_, ok) = executor.run("zellij", &args_refs);
    if ok {
        Some(JumpOutcome::Activated {
            message: format!("Attached to Zellij session '{session}'"),
        })
    } else {
        Some(JumpOutcome::Failed {
            message: format!("Failed to attach to Zellij session '{session}'"),
        })
    }
}

// ── cmux ──────────────────────────────────────────────────────────────────────

/// Build the cmux `surface.focus` JSON-RPC request body.
pub fn cmux_focus_rpc_body(surface_id: &str) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "surface.focus",
        "params": { "surface_id": surface_id },
        "id": 1
    })
    .to_string()
}

/// Resolve the cmux control-socket path.
///
/// cmux writes its live socket path to `/tmp/cmux-last-socket-path` on startup;
/// prefer that, then the default support-dir socket, then a legacy `/tmp` path.
/// Returns a best-guess path regardless of existence — a dead path just makes
/// the RPC fail, and the caller falls back to activating the app.
pub fn resolve_cmux_socket_path() -> String {
    if let Ok(contents) = std::fs::read_to_string("/tmp/cmux-last-socket-path") {
        let p = contents.trim();
        if !p.is_empty() {
            return p.to_string();
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let default = format!("{home}/Library/Application Support/cmux/cmux.sock");
        if std::path::Path::new(&default).exists() {
            return default;
        }
    }
    "/tmp/cmux.sock".to_string()
}

/// Execute a cmux jump: focus the surface via a Unix-socket JSON-RPC call, then
/// bring the app to the front. Returns `None` (so the caller falls back to
/// activating the app) when the socket write fails.
pub fn jump_cmux(surface_id: &str, executor: &dyn Executor) -> Option<JumpOutcome> {
    let socket = resolve_cmux_socket_path();
    let body = cmux_focus_rpc_body(surface_id);
    if executor.send_unix_rpc(&socket, &body) {
        // The RPC focuses the surface inside cmux; activating brings cmux forward.
        let _ = executor.activate_app("com.cmuxterm.app");
        Some(JumpOutcome::Focused {
            message: format!("Focused cmux surface {surface_id}"),
        })
    } else {
        None
    }
}

// ── AppleScript helpers ───────────────────────────────────────────────────────

/// Escape a string for embedding inside an AppleScript string literal.
///
/// AppleScript string literals are delimited by `"`. Inside them:
/// - `\` → `\\`  (though AS doesn't use backslash escaping, some osascript versions do)
/// - `"` → `\" ` is not valid AS; instead we concatenate: `" & quote & "`
///
/// We use the concatenation approach as it is universally safe.
pub fn applescript_escape(s: &str) -> String {
    // Replace embedded double-quotes with AppleScript's `quote` variable
    s.replace('"', "\" & quote & \"")
}

/// Build an AppleScript to activate iTerm2 and select the tab/session matching
/// `session_id` or `tty`.
pub fn iterm2_focus_script(session_id: Option<&str>, tty: Option<&str>) -> String {
    let match_clause = if let Some(sid) = session_id {
        format!(
            r#"if tty of current_session is "{sid}" or \
                (exists (unique ID of current_session)) and (unique ID of current_session is "{sid}") then"#,
            sid = applescript_escape(sid)
        )
    } else if let Some(t) = tty {
        format!(
            r#"if tty of current_session is "{t}" then"#,
            t = applescript_escape(t)
        )
    } else {
        // No specific locator — just activate
        return r#"tell application "iTerm2" to activate"#.to_string();
    };

    format!(
        r#"tell application "iTerm2"
  activate
  repeat with win in windows
    repeat with tab in tabs of win
      set current_session to current session of tab
      {match_clause}
        tell win
          select
        end tell
        select tab
        select current_session
        return
      end if
    end repeat
  end repeat
end tell"#
    )
}

/// Build an AppleScript to activate Ghostty and focus the tab matching `session_id`
/// or `tty`.
pub fn ghostty_focus_script(session_id: Option<&str>, tty: Option<&str>) -> String {
    let locator = session_id
        .map(|s| format!("session-id:{}", applescript_escape(s)))
        .or_else(|| tty.map(|t| format!("tty:{}", applescript_escape(t))))
        .unwrap_or_else(|| "any".to_string());
    format!(
        r#"tell application "Ghostty"
  activate
  -- locator: {locator}
  -- Ghostty AppleScript support is limited; best-effort activate only
end tell"#
    )
}

/// Build an AppleScript to activate Terminal.app and select the tab matching `tty`.
pub fn terminal_app_focus_script(tty: Option<&str>) -> String {
    if let Some(t) = tty {
        format!(
            r#"tell application "Terminal"
  activate
  repeat with win in windows
    repeat with tab in tabs of win
      if tty of tab is "{t}" then
        set selected of tab to true
        set frontmost of win to true
        return
      end if
    end repeat
  end repeat
end tell"#,
            t = applescript_escape(t)
        )
    } else {
        r#"tell application "Terminal" to activate"#.to_string()
    }
}

/// Execute an AppleScript-based terminal jump.
pub fn jump_applescript(
    host: &str,
    target: &JumpTarget,
    executor: &dyn Executor,
) -> Option<JumpOutcome> {
    let script = match host {
        "iterm2" => iterm2_focus_script(
            target.terminal_session_id.as_deref(),
            target.terminal_tty.as_deref(),
        ),
        "ghostty" => ghostty_focus_script(
            target.terminal_session_id.as_deref(),
            target.terminal_tty.as_deref(),
        ),
        "terminal" => terminal_app_focus_script(target.terminal_tty.as_deref()),
        _ => {
            // Generic: just activate via bundle id
            return None;
        }
    };

    let (_, ok) = executor.run_applescript(&script);
    if ok {
        Some(JumpOutcome::Focused {
            message: format!("Focused {} tab", host),
        })
    } else {
        // Return None to let the caller apply the fallback ladder
        None
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_tmux_target ─────────────────────────────────────────────────

    #[test]
    fn parse_tmux_target_splits_on_pipe() {
        let (socket_info, pane) = parse_tmux_target("/tmp/tmux-1000/default,42,0|%3");
        assert_eq!(socket_info, "/tmp/tmux-1000/default,42,0");
        assert_eq!(pane, "%3");
    }

    #[test]
    fn parse_tmux_target_no_pipe() {
        let (socket_info, pane) = parse_tmux_target("/tmp/tmux-1000/default,42,0");
        assert_eq!(socket_info, "/tmp/tmux-1000/default,42,0");
        assert_eq!(pane, "");
    }

    #[test]
    fn tmux_socket_from_env_extracts_first_field() {
        let socket = tmux_socket_from_env("/tmp/tmux-1000/default,42,0");
        assert_eq!(socket, "/tmp/tmux-1000/default");
    }

    // ── tmux command construction ─────────────────────────────────────────

    #[test]
    fn tmux_switch_client_args_correct() {
        let args = tmux_switch_client_args("/tmp/tmux-1000/default", "/dev/ttys001");
        assert_eq!(
            args,
            vec![
                "-S",
                "/tmp/tmux-1000/default",
                "switch-client",
                "-c",
                "/dev/ttys001"
            ]
        );
    }

    #[test]
    fn tmux_select_window_args_correct() {
        let args = tmux_select_window_args("/tmp/tmux-1000/default", "main:1");
        assert_eq!(
            args,
            vec![
                "-S",
                "/tmp/tmux-1000/default",
                "select-window",
                "-t",
                "main:1"
            ]
        );
    }

    #[test]
    fn tmux_select_pane_args_correct() {
        let args = tmux_select_pane_args("/tmp/tmux-1000/default", "%3");
        assert_eq!(
            args,
            vec!["-S", "/tmp/tmux-1000/default", "select-pane", "-t", "%3"]
        );
    }

    // ── WezTerm command construction ─────────────────────────────────────

    #[test]
    fn wezterm_activate_pane_args_correct() {
        let args = wezterm_activate_pane_args("7");
        assert_eq!(args, vec!["cli", "activate-pane", "--pane-id", "7"]);
    }

    // ── Zellij command construction ───────────────────────────────────────

    #[test]
    fn zellij_go_to_tab_args_correct() {
        let args = zellij_go_to_tab_args("dev");
        assert_eq!(args, vec!["-s", "dev", "action", "focus-next-pane"]);
    }

    // ── cmux JSON-RPC ─────────────────────────────────────────────────────

    #[test]
    fn cmux_focus_rpc_body_is_valid_json() {
        let body = cmux_focus_rpc_body("surface-1");
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(v["method"], "surface.focus");
        assert_eq!(v["params"]["surface_id"], "surface-1");
        assert_eq!(v["jsonrpc"], "2.0");
    }

    // ── AppleScript escape ────────────────────────────────────────────────

    #[test]
    fn applescript_escape_no_special_chars() {
        assert_eq!(applescript_escape("hello world"), "hello world");
    }

    #[test]
    fn applescript_escape_double_quote() {
        let result = applescript_escape(r#"say "hello""#);
        assert!(
            result.contains("& quote &"),
            "double-quote should be replaced: {result}"
        );
        assert!(!result.contains("\\\""), "should not use backslash escape");
    }

    #[test]
    fn applescript_escape_multiple_quotes() {
        let result = applescript_escape(r#"a"b"c"#);
        // Should have two occurrences of & quote &
        assert_eq!(result.matches("& quote &").count(), 2);
    }

    // ── iTerm2 AppleScript ────────────────────────────────────────────────

    #[test]
    fn iterm2_focus_script_with_session_id() {
        let script = iterm2_focus_script(Some("w0t0p0:ABCD"), None);
        assert!(script.contains("iTerm2"), "should target iTerm2");
        assert!(script.contains("w0t0p0:ABCD"), "should contain session id");
        assert!(script.contains("activate"), "should activate app");
    }

    #[test]
    fn iterm2_focus_script_with_tty() {
        let script = iterm2_focus_script(None, Some("/dev/ttys003"));
        assert!(script.contains("/dev/ttys003"));
    }

    #[test]
    fn iterm2_focus_script_no_locator_just_activates() {
        let script = iterm2_focus_script(None, None);
        assert_eq!(script, r#"tell application "iTerm2" to activate"#);
    }

    #[test]
    fn iterm2_script_escapes_quotes_in_session_id() {
        let script = iterm2_focus_script(Some("abc\"def"), None);
        assert!(
            script.contains("& quote &"),
            "should escape embedded quotes: {script}"
        );
    }

    // ── Terminal.app AppleScript ──────────────────────────────────────────

    #[test]
    fn terminal_app_focus_script_with_tty() {
        let script = terminal_app_focus_script(Some("/dev/ttys001"));
        assert!(script.contains("Terminal"));
        assert!(script.contains("/dev/ttys001"));
    }

    #[test]
    fn terminal_app_focus_script_no_tty_just_activates() {
        let script = terminal_app_focus_script(None);
        assert_eq!(script, r#"tell application "Terminal" to activate"#);
    }

    // ── Ghostty AppleScript ───────────────────────────────────────────────

    #[test]
    fn ghostty_focus_script_contains_ghostty() {
        let script = ghostty_focus_script(Some("ghost-42"), None);
        assert!(script.contains("Ghostty"));
        assert!(script.contains("ghost-42"));
    }
}
