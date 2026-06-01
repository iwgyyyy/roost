use std::collections::HashMap;
use std::io::{self, Read};

use anyhow::Result;

use crate::protocol::{AgentFamily, EventKind, HookEvent, Request};
use crate::sock;

// ---------------------------------------------------------------------------
// Agent-family process name patterns (lower-cased).
// ---------------------------------------------------------------------------
const AGENT_PROCESS_NAMES: &[&str] = &["claude", "codex", "deepseek", "deepseek-tui", "node", "python"];

/// Resolve the agent's pid by walking the parent-process chain from `start_pid`.
/// Stops when a process name matches a known agent binary (claude/codex).
/// Falls back to the direct parent pid if no match is found.
///
/// `parent_of_fn(pid) -> Option<u32>` returns the ppid of `pid` (None if the
/// process no longer exists or we've hit PID 1/0).
/// `name_of_fn(pid) -> Option<String>` returns the process name of `pid`.
pub fn resolve_agent_pid<PF, NF>(start_pid: u32, parent_of_fn: PF, name_of_fn: NF) -> Option<u32>
where
    PF: Fn(u32) -> Option<u32>,
    NF: Fn(u32) -> Option<String>,
{
    let direct_parent = parent_of_fn(start_pid)?;
    let mut cur = direct_parent;

    // Walk up at most 16 levels to avoid infinite loops.
    for _ in 0..16 {
        if cur == 0 || cur == 1 {
            break;
        }
        if let Some(name) = name_of_fn(cur) {
            let lower = name.to_lowercase();
            // Check executable basename against known agent names.
            let basename = lower.rsplit('/').next().unwrap_or(&lower);
            if AGENT_PROCESS_NAMES
                .iter()
                .any(|&pat| basename.starts_with(pat))
            {
                return Some(cur);
            }
        }
        match parent_of_fn(cur) {
            Some(parent) if parent != cur => cur = parent,
            _ => break,
        }
    }

    // Fallback: return the direct parent of the hook process.
    Some(direct_parent)
}

/// Run the hook subcommand: read stdin JSON, build HookEvent, send to daemon.
/// family: "claude" | "codex"
/// event_name: e.g. "SessionStart", "PreToolUse", etc.
pub fn run_hook(family: &str, event_name: &str) -> Result<()> {
    // CodeWhale (DeepSeek) delivers all context via DEEPSEEK_* env vars and runs
    // session_start synchronously from its raw-mode TUI, leaving the hook's stdin
    // attached to the terminal (which never sends EOF). Reading it would block
    // forever and freeze the TUI, so skip stdin entirely for that family.
    let stdin_data = if matches!(parse_family(family), AgentFamily::Deepseek) {
        String::new()
    } else {
        read_stdin()?
    };
    let event = build_hook_event(family, event_name, &stdin_data)?;

    let path = sock::socket_path();

    // Use connect_hook: write_timeout=200ms, no read_timeout — never block the agent
    let mut stream = match sock::connect_hook(&path) {
        Ok(s) => s,
        Err(_) => {
            // Daemon not available — silently exit, never block the agent
            return Ok(());
        }
    };

    let request = Request::Report {
        event: Box::new(event),
    };
    let json = serde_json::to_string(&request)?;
    if let Err(_e) = sock::send_line(&mut stream, &json) {
        // Silently ignore — never block agent
    }

    Ok(())
}

// ── Phase 4: host detection + locator collection ──────────────────────────────

/// Detect the host application from the process environment.
///
/// Checks (in order):
/// 1. `CODEX_SESSION_ID` present → Codex.app
/// 2. `WEZTERM_PANE` present → wezterm
/// 3. `ZELLIJ_SESSION_NAME` present → zellij
/// 4. `TMUX` present → tmux
/// 5. `TERM_PROGRAM` value (ghost, iterm2, terminal, vscode, cursor …)
/// 6. Editor-specific env vars
pub fn detect_host(env: &HashMap<String, String>) -> String {
    // Codex.app: sets a session env var
    if env.contains_key("CODEX_SESSION_ID") || env.contains_key("CODEX_THREAD_ID") {
        return "codex".to_string();
    }
    // WezTerm
    if env.contains_key("WEZTERM_PANE") {
        return "wezterm".to_string();
    }
    // Zellij
    if env.contains_key("ZELLIJ_SESSION_NAME") || env.contains_key("ZELLIJ") {
        return "zellij".to_string();
    }
    // tmux (inside tmux sets $TMUX)
    if env.contains_key("TMUX") {
        return "tmux".to_string();
    }

    // Editor-hosted terminals via env hints
    // VS Code / forks set VSCODE_INJECTION, VSCODE_CWD or TERM_PROGRAM
    if env.contains_key("VSCODE_INJECTION") || env.contains_key("VSCODE_CWD") {
        // Distinguish forks by VSCODE_IPC_HOOK_CLI path or TERM_PROGRAM
        let tp = env.get("TERM_PROGRAM").map(|s| s.to_lowercase());
        return match tp.as_deref() {
            Some(s) if s.contains("cursor") => "cursor".to_string(),
            Some(s) if s.contains("windsurf") => "windsurf".to_string(),
            Some(s) if s.contains("trae") => "trae".to_string(),
            Some(s) if s.contains("code-insiders") || s.contains("vscode-insiders") => {
                "vscode-insiders".to_string()
            }
            _ => "vscode".to_string(),
        };
    }
    // JetBrains terminal (sets TERMINAL_EMULATOR)
    if let Some(te) = env.get("TERMINAL_EMULATOR") {
        let lower = te.to_lowercase();
        if lower.contains("jetbrains") || lower.contains("intellij") {
            return "jetbrains".to_string();
        }
    }
    // Zed sets ZED_TERM
    if env.contains_key("ZED_TERM") {
        return "zed".to_string();
    }

    // TERM_PROGRAM as fallback
    if let Some(tp) = env.get("TERM_PROGRAM") {
        let lower = tp.to_lowercase();
        if lower.contains("ghostty") {
            return "ghostty".to_string();
        }
        if lower.contains("iterm") || lower.contains("iterm2") {
            return "iterm2".to_string();
        }
        if lower.contains("warp") {
            return "warp".to_string();
        }
        if lower.contains("alacritty") {
            return "alacritty".to_string();
        }
        if lower.contains("hyper") {
            return "hyper".to_string();
        }
        // Includes "Apple_Terminal" or plain "Terminal"
        if lower.contains("terminal") {
            return "terminal".to_string();
        }
    }
    // TERM_PROGRAM_VERSION hints
    if let Some(app) = env.get("__CFBundleIdentifier") {
        let lower = app.to_lowercase();
        if lower.contains("ghostty") {
            return "ghostty".to_string();
        }
        if lower.contains("iterm2") {
            return "iterm2".to_string();
        }
    }

    "unknown".to_string()
}

/// Locator fields extracted from the process environment for jump (§14).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Locators {
    pub terminal_tty: Option<String>,
    pub terminal_session_id: Option<String>,
    pub tmux_target: Option<String>,
    pub wezterm_pane: Option<String>,
    pub zellij_session: Option<String>,
    pub workspace_path: Option<String>,
    pub codex_thread_id: Option<String>,
    pub pane_title: Option<String>,
}

/// Resolve the tty device path for a given file descriptor number.
///
/// Returns `None` if the fd is not a tty or the path cannot be determined.
/// This signature is injectable for testing.
pub type TtyResolver = fn(u32) -> Option<String>;

/// Default `TtyResolver`: calls `ttyname_r(3)` for the given raw fd number.
///
/// Returns `None` if the fd is not a tty or the path cannot be determined.
pub fn real_tty_resolver(fd: u32) -> Option<String> {
    // First check if it's actually a tty using rustix (zero-cost on non-tty fds).
    use rustix::fd::BorrowedFd;
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd as i32) };
    if !rustix::termios::isatty(borrowed) {
        return None;
    }

    // Call libc::ttyname_r to get the device path.
    let mut buf = vec![0u8; 256];
    #[allow(unsafe_code)]
    let ok = unsafe {
        libc::ttyname_r(
            fd as libc::c_int,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
        ) == 0
    };
    if !ok {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    std::str::from_utf8(&buf[..end]).ok().map(|s| s.to_string())
}

/// Collect jump locator handles from the process environment.
/// `tty_resolver` is called with fd numbers 0, 1, 2 (in order) to find the
/// controlling tty when env vars do not supply it.
pub fn collect_locators_with_tty<F>(env: &HashMap<String, String>, tty_resolver: F) -> Locators
where
    F: Fn(u32) -> Option<String>,
{
    // TTY — priority: $ROOST_TTY > $SSH_TTY > real controlling terminal (fd 0/1/2)
    let terminal_tty = env
        .get("ROOST_TTY")
        .or_else(|| env.get("SSH_TTY"))
        .cloned()
        .or_else(|| [0u32, 1, 2].iter().find_map(|&fd| tty_resolver(fd)));

    collect_locators_inner(env, terminal_tty)
}

/// Collect jump locator handles from the process environment.
pub fn collect_locators(env: &HashMap<String, String>) -> Locators {
    // TTY — priority: $ROOST_TTY > $SSH_TTY > real controlling terminal (fd 0/1/2)
    let terminal_tty = env
        .get("ROOST_TTY")
        .or_else(|| env.get("SSH_TTY"))
        .cloned()
        .or_else(|| [0u32, 1, 2].iter().find_map(|&fd| real_tty_resolver(fd)));

    collect_locators_inner(env, terminal_tty)
}

fn collect_locators_inner(env: &HashMap<String, String>, terminal_tty: Option<String>) -> Locators {
    // iTerm2 / Ghostty session id
    let terminal_session_id = env
        .get("ITERM_SESSION_ID")
        .or_else(|| env.get("GHOSTTY_SESSION_ID"))
        .cloned();

    // tmux: $TMUX = socket,pid,session-id  →  $TMUX_PANE = pane target
    let tmux_target = env.get("TMUX").map(|tmux_env| {
        let pane = env
            .get("TMUX_PANE")
            .cloned()
            .unwrap_or_else(|| "$0".to_string());
        format!("{tmux_env}|{pane}")
    });

    let wezterm_pane = env.get("WEZTERM_PANE").cloned();
    let zellij_session = env.get("ZELLIJ_SESSION_NAME").cloned();

    let workspace_path = env
        .get("VSCODE_WORKSPACE_FOLDER")
        .or_else(|| env.get("JETBRAINS_PROJECT_ROOT"))
        .or_else(|| env.get("ROOST_WORKSPACE"))
        .cloned();

    let codex_thread_id = env
        .get("CODEX_THREAD_ID")
        .or_else(|| env.get("CODEX_SESSION_ID"))
        .cloned();

    let pane_title = env.get("ROOST_PANE_TITLE").cloned();

    Locators {
        terminal_tty,
        terminal_session_id,
        tmux_target,
        wezterm_pane,
        zellij_session,
        workspace_path,
        codex_thread_id,
        pane_title,
    }
}

/// Read all of stdin (hook JSON payload).
fn read_stdin() -> Result<String> {
    let mut buf = String::new();
    // Safety net for every family: if stdin is a terminal there is no piped hook
    // payload, so don't block on read_to_string waiting for an EOF that an
    // interactive TUI will never send.
    use std::io::IsTerminal;
    if io::stdin().is_terminal() {
        return Ok(String::new());
    }
    io::stdin().read_to_string(&mut buf)?;
    Ok(buf)
}

/// Build a HookEvent from the family string, event name string, and raw stdin JSON.
pub fn build_hook_event(family: &str, event_name: &str, stdin_json: &str) -> Result<HookEvent> {
    let family = parse_family(family);
    let kind = parse_event_kind(event_name)?;

    // Parse the stdin JSON payload (Claude hook format)
    let payload: serde_json::Value = if stdin_json.trim().is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(stdin_json)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()))
    };

    // CodeWhale (DeepSeek TUI) passes hook context via DEEPSEEK_* environment
    // variables rather than a stdin JSON payload, so for that family every field
    // is read from the environment. Other families keep reading the stdin JSON.
    let is_deepseek = matches!(family, AgentFamily::Deepseek);
    let is_cursor = matches!(family, AgentFamily::Cursor);
    let env_str = |key: &str| std::env::var(key).ok().filter(|s| !s.is_empty());

    let session_id = if is_deepseek {
        env_str("DEEPSEEK_SESSION_ID")
    } else {
        // Cursor sends `session_id` (== conversation_id) in its stdin JSON, so the
        // standard extraction covers Claude / Codex / Cursor alike.
        extract_string(&payload, &["session_id", "sessionId", "id"])
    }
    .unwrap_or_else(|| format!("unknown-{}", std::process::id()));

    let cwd = if is_deepseek {
        env_str("DEEPSEEK_WORKSPACE")
    } else if is_cursor {
        // Cursor has no `cwd` field; it sends a `workspace_roots` array. Use the
        // first root (the primary workspace) for naming / jump.
        payload
            .get("workspace_roots")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        extract_string(&payload, &["cwd", "working_directory", "workingDirectory"])
    }
    .or_else(|| {
        std::env::current_dir()
            .ok()
            .and_then(|p| p.to_str().map(|s| s.to_string()))
    })
    .unwrap_or_else(|| "/".to_string());

    // Resolve agent pid only for SessionStart — pid is stable within a session,
    // and resolving it (fork ps on macOS, /proc read on Linux) on every hook event
    // would add 20-50 ms latency to high-frequency PreToolUse/PostToolUse hooks.
    let pid = if matches!(kind, EventKind::SessionStart) {
        get_agent_pid()
    } else {
        None
    };

    // Extract tool info for PreToolUse/PostToolUse
    let tool_name = if is_deepseek {
        env_str("DEEPSEEK_TOOL_NAME")
    } else {
        extract_string(&payload, &["tool_name", "toolName", "tool"])
    };

    // tool_input is a JSON string. For DeepSeek it arrives as DEEPSEEK_TOOL_ARGS
    // (already JSON), which feeds clarify detection / activity text the same way.
    let tool_input = if is_deepseek {
        env_str("DEEPSEEK_TOOL_ARGS")
    } else {
        payload
            .get("tool_input")
            .or_else(|| payload.get("toolInput"))
            .map(|input| serde_json::to_string(input).unwrap_or_default())
    };

    // Extract prompt for UserPromptSubmit
    let prompt = if is_deepseek {
        env_str("DEEPSEEK_MESSAGE")
    } else {
        extract_string(
            &payload,
            &["prompt", "message", "user_message", "userMessage"],
        )
    };

    // Extract notification text (DeepSeek has no notification event roost uses).
    let notification = if is_deepseek {
        None
    } else {
        extract_string(&payload, &["message", "notification", "text", "content"])
    };

    // ── Phase 4: collect jump locators + host from real env. Captured on the
    // low-frequency events only (SessionStart and UserPromptSubmit) so the hot
    // PreToolUse/PostToolUse path adds nothing; capturing on UserPromptSubmit
    // (not just SessionStart) means a session whose SessionStart the daemon
    // missed — daemon restarted, or hooks installed after the agent started —
    // recovers its host/locators the next time the user submits a prompt.
    // The env map is built once and shared by detect_host + collect_locators.
    let (
        host_app,
        host_bundle_id,
        terminal_session_id,
        terminal_tty,
        tmux_target,
        wezterm_pane,
        zellij_session,
        workspace_path,
        codex_thread_id,
        pane_title,
    ) = if matches!(
        kind,
        EventKind::SessionStart | EventKind::UserPromptSubmit
    ) {
        let env: HashMap<String, String> = std::env::vars().collect();
        // Cursor's agent hook runs in Cursor's own (VS Code-derived) environment,
        // where detect_host would see VSCODE_* vars but no TERM_PROGRAM=cursor and
        // misreport "vscode" (shown as "Code"). We know the host from the family,
        // so pin it to "cursor".
        let host = if is_cursor {
            "cursor".to_string()
        } else {
            detect_host(&env)
        };
        let loc = collect_locators(&env);
        // Precise host bundle id (macOS): exact app identity, e.g. a preview or
        // fork variant the static descriptor doesn't know.
        let bundle = env
            .get("__CFBundleIdentifier")
            .filter(|s| !s.is_empty())
            .cloned();

        // For Codex.app: thread id = session_id
        let codex_tid = if host == "codex" {
            Some(session_id.clone())
        } else {
            loc.codex_thread_id
        };

        (
            Some(host),
            bundle,
            loc.terminal_session_id,
            loc.terminal_tty,
            loc.tmux_target,
            loc.wezterm_pane,
            loc.zellij_session,
            loc.workspace_path,
            codex_tid,
            loc.pane_title,
        )
    } else {
        (None, None, None, None, None, None, None, None, None, None)
    };

    Ok(HookEvent {
        kind,
        family,
        session_id,
        cwd,
        pid,
        tool_name,
        tool_input,
        prompt,
        notification,
        host_app,
        host_bundle_id,
        terminal_session_id,
        terminal_tty,
        tmux_target,
        wezterm_pane,
        zellij_session,
        workspace_path,
        codex_thread_id,
        pane_title,
    })
}

fn extract_string(v: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = v.get(key).and_then(|v| v.as_str())
            && !s.is_empty()
        {
            return Some(s.to_string());
        }
    }
    None
}

fn parse_family(s: &str) -> AgentFamily {
    match s.to_lowercase().as_str() {
        "claude" => AgentFamily::Claude,
        "codex" => AgentFamily::Codex,
        "deepseek" | "codewhale" => AgentFamily::Deepseek,
        "cursor" => AgentFamily::Cursor,
        _ => AgentFamily::Unknown,
    }
}

pub fn parse_event_kind(s: &str) -> Result<EventKind> {
    match s {
        "SessionStart" => Ok(EventKind::SessionStart),
        "UserPromptSubmit" => Ok(EventKind::UserPromptSubmit),
        "PreToolUse" => Ok(EventKind::PreToolUse),
        "PostToolUse" => Ok(EventKind::PostToolUse),
        "Notification" => Ok(EventKind::Notification),
        "Stop" => Ok(EventKind::Stop),
        "SessionEnd" => Ok(EventKind::SessionEnd),
        // Codex-specific events (real Codex hook event names)
        "PermissionRequest" => Ok(EventKind::PermissionRequest),
        other => anyhow::bail!("unknown event kind: {other}"),
    }
}

fn get_agent_pid() -> Option<u32> {
    let self_pid = rustix::process::getpid().as_raw_nonzero().get() as u32;
    resolve_agent_pid(self_pid, real_ppid, real_process_name)
}

/// Get the parent pid of `pid` using procfs (Linux) or sysctl (macOS).
fn real_ppid(pid: u32) -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        macos_ppid(pid)
    }
    #[cfg(target_os = "linux")]
    {
        linux_ppid(pid)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = pid;
        None
    }
}

/// Get the process name (basename of executable) for `pid`.
fn real_process_name(pid: u32) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        macos_process_name(pid)
    }
    #[cfg(target_os = "linux")]
    {
        linux_process_name(pid)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = pid;
        None
    }
}

#[cfg(target_os = "macos")]
fn macos_ppid(pid: u32) -> Option<u32> {
    // Use `ps -o ppid= -p <pid>` — lightweight and doesn't require unsafe sysctl bindings.
    let out = std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let s = String::from_utf8(out.stdout).ok()?;
    s.trim().parse::<u32>().ok()
}

#[cfg(target_os = "macos")]
fn macos_process_name(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let s = String::from_utf8(out.stdout).ok()?;
    let name = s.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[cfg(target_os = "linux")]
fn linux_ppid(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("PPid:") {
            return rest.trim().parse::<u32>().ok();
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn linux_process_name(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::EventKind;

    // ---------------------------------------------------------------------------
    // resolve_agent_pid tests
    // ---------------------------------------------------------------------------

    #[test]
    fn resolve_agent_pid_finds_claude_ancestor() {
        // Process tree: hook(100) -> shell(99) -> claude(98) -> init(1)
        let parent_of = |pid: u32| -> Option<u32> {
            match pid {
                100 => Some(99),
                99 => Some(98),
                98 => Some(1),
                _ => None,
            }
        };
        let name_of = |pid: u32| -> Option<String> {
            match pid {
                99 => Some("bash".to_string()),
                98 => Some("claude".to_string()),
                _ => None,
            }
        };
        let result = resolve_agent_pid(100, parent_of, name_of);
        assert_eq!(result, Some(98));
    }

    #[test]
    fn resolve_agent_pid_finds_codex_ancestor() {
        // Process tree: hook(200) -> shell(199) -> codex(198) -> init(1)
        let parent_of = |pid: u32| -> Option<u32> {
            match pid {
                200 => Some(199),
                199 => Some(198),
                198 => Some(1),
                _ => None,
            }
        };
        let name_of = |pid: u32| -> Option<String> {
            match pid {
                199 => Some("sh".to_string()),
                198 => Some("codex".to_string()),
                _ => None,
            }
        };
        let result = resolve_agent_pid(200, parent_of, name_of);
        assert_eq!(result, Some(198));
    }

    #[test]
    fn resolve_agent_pid_falls_back_to_direct_parent_when_no_match() {
        // No agent-named process in chain; fallback to direct parent.
        let parent_of = |pid: u32| -> Option<u32> {
            match pid {
                300 => Some(299),
                299 => Some(1),
                _ => None,
            }
        };
        let name_of = |pid: u32| -> Option<String> {
            match pid {
                299 => Some("bash".to_string()),
                _ => None,
            }
        };
        let result = resolve_agent_pid(300, parent_of, name_of);
        assert_eq!(result, Some(299));
    }

    #[test]
    fn resolve_agent_pid_none_when_no_parent() {
        // start_pid has no parent.
        let parent_of = |_pid: u32| -> Option<u32> { None };
        let name_of = |_pid: u32| -> Option<String> { None };
        let result = resolve_agent_pid(1, parent_of, name_of);
        assert_eq!(result, None);
    }

    // ---------------------------------------------------------------------------
    // parse_event_kind tests (Codex events)
    // ---------------------------------------------------------------------------

    #[test]
    fn parse_codex_event_kinds() {
        assert!(matches!(
            parse_event_kind("PermissionRequest").unwrap(),
            EventKind::PermissionRequest
        ));
    }

    #[test]
    fn parse_family_cases() {
        assert_eq!(parse_family("claude"), AgentFamily::Claude);
        assert_eq!(parse_family("Claude"), AgentFamily::Claude);
        assert_eq!(parse_family("CODEX"), AgentFamily::Codex);
        assert_eq!(parse_family("deepseek"), AgentFamily::Deepseek);
        assert_eq!(parse_family("CodeWhale"), AgentFamily::Deepseek);
        assert_eq!(parse_family("cursor"), AgentFamily::Cursor);
        assert_eq!(parse_family("unknown_agent"), AgentFamily::Unknown);
    }

    #[test]
    fn parse_event_kind_cases() {
        assert!(matches!(
            parse_event_kind("SessionStart").unwrap(),
            EventKind::SessionStart
        ));
        assert!(matches!(
            parse_event_kind("PreToolUse").unwrap(),
            EventKind::PreToolUse
        ));
        assert!(parse_event_kind("BogusEvent").is_err());
    }

    #[test]
    fn build_hook_event_session_start() {
        let json = r#"{"session_id":"test-sess","cwd":"/tmp/project"}"#;
        let ev = build_hook_event("claude", "SessionStart", json).unwrap();
        assert_eq!(ev.session_id, "test-sess");
        assert_eq!(ev.cwd, "/tmp/project");
        assert_eq!(ev.family, AgentFamily::Claude);
        assert!(matches!(ev.kind, EventKind::SessionStart));
    }

    #[test]
    fn build_hook_event_pre_tool_use() {
        let json = r#"{"session_id":"s1","cwd":"/repo","tool_name":"Edit","tool_input":{"path":"src/main.rs"}}"#;
        let ev = build_hook_event("claude", "PreToolUse", json).unwrap();
        assert_eq!(ev.tool_name.as_deref(), Some("Edit"));
        assert!(ev.tool_input.is_some());
    }

    #[test]
    fn build_hook_event_cursor_reads_session_workspace_and_tool() {
        // Cursor stdin JSON: session_id present, no `cwd` (uses workspace_roots[]),
        // standard tool_name/tool_input. The command arg is roost's EventKind name.
        let json = r#"{"session_id":"38d4-cursor","prompt":"hi","tool_name":"Grep","tool_input":{"pattern":"clarify"},"workspace_roots":["/Users/yi/project/web","/Users/yi/project/api"],"hook_event_name":"preToolUse"}"#;
        let ev = build_hook_event("cursor", "PreToolUse", json).unwrap();
        assert_eq!(ev.family, AgentFamily::Cursor);
        assert_eq!(ev.session_id, "38d4-cursor");
        // cwd resolves to the first workspace root.
        assert_eq!(ev.cwd, "/Users/yi/project/web");
        assert_eq!(ev.tool_name.as_deref(), Some("Grep"));
        assert!(ev.tool_input.is_some());
    }

    #[test]
    fn build_hook_event_empty_stdin_uses_cwd_env() {
        let ev = build_hook_event("claude", "SessionStart", "").unwrap();
        // Should not panic, should have some cwd
        assert!(!ev.cwd.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Pid-resolution hot-path guard: only SessionStart resolves pid
    // ---------------------------------------------------------------------------

    /// Non-SessionStart events must NOT attempt pid resolution (no ps fork).
    /// We verify this at the build_hook_event level: for every high-frequency
    /// event kind, the returned pid is None (because no fork happened).
    /// (SessionStart pid is tested separately — it may be None in test env too,
    /// but the important invariant is that the *other* events are always None.)
    #[test]
    fn non_session_start_events_have_no_pid() {
        let json = r#"{"session_id":"s","cwd":"/tmp"}"#;
        for event_name in &[
            "PreToolUse",
            "PostToolUse",
            "Notification",
            "Stop",
            "SessionEnd",
            "UserPromptSubmit",
            "PermissionRequest",
        ] {
            let ev = build_hook_event("claude", event_name, json).unwrap();
            assert!(
                ev.pid.is_none(),
                "event {event_name} should not resolve pid (pid={:?})",
                ev.pid
            );
        }
    }

    // ── detect_host tests ──────────────────────────────────────────────────────

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn detect_host_codex_by_codex_session_id() {
        let e = env(&[("CODEX_SESSION_ID", "abc123")]);
        assert_eq!(detect_host(&e), "codex");
    }

    #[test]
    fn detect_host_codex_by_codex_thread_id() {
        let e = env(&[("CODEX_THREAD_ID", "t-1")]);
        assert_eq!(detect_host(&e), "codex");
    }

    #[test]
    fn detect_host_wezterm_by_wezterm_pane() {
        let e = env(&[("WEZTERM_PANE", "3")]);
        assert_eq!(detect_host(&e), "wezterm");
    }

    #[test]
    fn detect_host_zellij_by_session_name() {
        let e = env(&[("ZELLIJ_SESSION_NAME", "my-sess")]);
        assert_eq!(detect_host(&e), "zellij");
    }

    #[test]
    fn detect_host_tmux_by_tmux_env() {
        let e = env(&[("TMUX", "/tmp/tmux-1234/default,12345,0")]);
        assert_eq!(detect_host(&e), "tmux");
    }

    #[test]
    fn detect_host_vscode_by_injection() {
        let e = env(&[("VSCODE_INJECTION", "1")]);
        assert_eq!(detect_host(&e), "vscode");
    }

    #[test]
    fn detect_host_cursor_by_term_program_with_vscode_injection() {
        let e = env(&[("VSCODE_INJECTION", "1"), ("TERM_PROGRAM", "cursor")]);
        assert_eq!(detect_host(&e), "cursor");
    }

    #[test]
    fn detect_host_windsurf_by_term_program_with_vscode_injection() {
        let e = env(&[("VSCODE_INJECTION", "1"), ("TERM_PROGRAM", "windsurf")]);
        assert_eq!(detect_host(&e), "windsurf");
    }

    #[test]
    fn detect_host_jetbrains_by_terminal_emulator() {
        let e = env(&[("TERMINAL_EMULATOR", "JetBrains-JediTerm")]);
        assert_eq!(detect_host(&e), "jetbrains");
    }

    #[test]
    fn detect_host_zed_by_zed_term() {
        let e = env(&[("ZED_TERM", "1")]);
        assert_eq!(detect_host(&e), "zed");
    }

    #[test]
    fn detect_host_ghostty_by_term_program() {
        let e = env(&[("TERM_PROGRAM", "ghostty")]);
        assert_eq!(detect_host(&e), "ghostty");
    }

    #[test]
    fn detect_host_iterm2_by_term_program() {
        let e = env(&[("TERM_PROGRAM", "iTerm.app")]);
        assert_eq!(detect_host(&e), "iterm2");
    }

    #[test]
    fn detect_host_warp_by_term_program() {
        let e = env(&[("TERM_PROGRAM", "WarpTerminal")]);
        assert_eq!(detect_host(&e), "warp");
    }

    #[test]
    fn detect_host_terminal_app_by_term_program() {
        let e = env(&[("TERM_PROGRAM", "Apple_Terminal")]);
        assert_eq!(detect_host(&e), "terminal");
    }

    #[test]
    fn detect_host_unknown_when_no_hints() {
        let e = env(&[]);
        assert_eq!(detect_host(&e), "unknown");
    }

    // WezTerm wins over TMUX when both are set (inner multiplexer)
    #[test]
    fn detect_host_wezterm_wins_over_tmux() {
        let e = env(&[
            ("WEZTERM_PANE", "5"),
            ("TMUX", "/tmp/tmux-1234/default,12345,0"),
        ]);
        assert_eq!(detect_host(&e), "wezterm");
    }

    // ── collect_locators tests ─────────────────────────────────────────────────

    #[test]
    fn collect_locators_tmux_target_combines_tmux_and_pane() {
        let e = env(&[("TMUX", "/tmp/tmux-1000/default,42,0"), ("TMUX_PANE", "%3")]);
        let loc = collect_locators(&e);
        let target = loc.tmux_target.expect("tmux_target should be set");
        assert!(
            target.contains("/tmp/tmux-1000/default,42,0"),
            "should contain socket: {target}"
        );
        assert!(target.contains("%3"), "should contain pane id: {target}");
    }

    #[test]
    fn collect_locators_tmux_fallback_pane() {
        let e = env(&[("TMUX", "/tmp/tmux-1000/default,42,0")]);
        let loc = collect_locators(&e);
        let target = loc.tmux_target.expect("tmux_target should be set");
        assert!(
            target.contains("$0"),
            "fallback pane should be $0: {target}"
        );
    }

    #[test]
    fn collect_locators_no_tmux_when_no_env() {
        let e = env(&[]);
        let loc = collect_locators(&e);
        assert!(loc.tmux_target.is_none());
    }

    #[test]
    fn collect_locators_wezterm_pane() {
        let e = env(&[("WEZTERM_PANE", "7")]);
        let loc = collect_locators(&e);
        assert_eq!(loc.wezterm_pane.as_deref(), Some("7"));
    }

    #[test]
    fn collect_locators_zellij_session() {
        let e = env(&[("ZELLIJ_SESSION_NAME", "dev")]);
        let loc = collect_locators(&e);
        assert_eq!(loc.zellij_session.as_deref(), Some("dev"));
    }

    #[test]
    fn collect_locators_iterm_session_id() {
        let e = env(&[("ITERM_SESSION_ID", "w0t0p0:ABCD-1234")]);
        let loc = collect_locators(&e);
        assert_eq!(loc.terminal_session_id.as_deref(), Some("w0t0p0:ABCD-1234"));
    }

    #[test]
    fn collect_locators_ghostty_session_id() {
        let e = env(&[("GHOSTTY_SESSION_ID", "ghost-42")]);
        let loc = collect_locators(&e);
        assert_eq!(loc.terminal_session_id.as_deref(), Some("ghost-42"));
    }

    #[test]
    fn collect_locators_codex_thread_id() {
        let e = env(&[("CODEX_THREAD_ID", "thread-abc")]);
        let loc = collect_locators(&e);
        assert_eq!(loc.codex_thread_id.as_deref(), Some("thread-abc"));
    }

    #[test]
    fn collect_locators_workspace_from_vscode() {
        let e = env(&[("VSCODE_WORKSPACE_FOLDER", "/home/user/project")]);
        let loc = collect_locators(&e);
        assert_eq!(loc.workspace_path.as_deref(), Some("/home/user/project"));
    }

    #[test]
    fn collect_locators_empty_env_all_none() {
        let e = env(&[]);
        // Use the injectable form so the test never calls real_tty_resolver,
        // which might find a real tty in the test process environment.
        let loc = collect_locators_with_tty(&e, |_fd| None);
        assert_eq!(loc, Locators::default());
    }

    // ── terminal_tty injection tests ───────────────────────────────────────────

    /// ROOST_TTY env var takes priority over fd fallback.
    #[test]
    fn collect_locators_tty_env_overrides_fd() {
        let e = env(&[("ROOST_TTY", "/dev/ttys001")]);
        // Provide a fd resolver that would return a different path
        let loc = collect_locators_with_tty(&e, |_fd| Some("/dev/ttys999".to_string()));
        assert_eq!(
            loc.terminal_tty.as_deref(),
            Some("/dev/ttys001"),
            "env var should win over fd resolver"
        );
    }

    /// When no env var is set, fall back to the fd resolver (try fd 0 first).
    #[test]
    fn collect_locators_tty_fd_fallback_when_no_env() {
        let e = env(&[]);
        let loc = collect_locators_with_tty(&e, |fd| {
            if fd == 0 {
                Some("/dev/ttys042".to_string())
            } else {
                None
            }
        });
        assert_eq!(
            loc.terminal_tty.as_deref(),
            Some("/dev/ttys042"),
            "should fall back to fd 0 tty path"
        );
    }

    /// When neither env var nor fd resolver produces a value, terminal_tty is None.
    #[test]
    fn collect_locators_tty_none_when_no_env_and_no_fd() {
        let e = env(&[]);
        let loc = collect_locators_with_tty(&e, |_fd| None);
        assert_eq!(
            loc.terminal_tty, None,
            "terminal_tty should be None when no env and no fd tty"
        );
    }
}
