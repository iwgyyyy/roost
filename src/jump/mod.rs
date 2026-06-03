//! Phase 4: terminal jump module.
//!
//! Entry point: `jump(target, executor)` — dispatches to host-specific
//! implementations and applies the fallback ladder from spec §14.

pub mod deeplink;
pub mod editors;
pub mod platform;
pub mod target;
pub mod terminals;
pub mod warp;

pub use target::{HostCategory, HostDescriptor, JumpTarget, descriptor_for, normalize_host_name};

// ── Jump outcome ──────────────────────────────────────────────────────────────

/// Result of a jump attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JumpOutcome {
    /// Successfully jumped to the exact pane/tab/conversation.
    Focused { message: String },
    /// Activated the app (window level); inner terminal tab not located.
    Activated { message: String },
    /// Fallback: opened the workspace directory in Finder / file manager.
    OpenedDirectory { message: String },
    /// Jump failed with a user-visible message.
    Failed { message: String },
    /// Host is unknown or unsupported.
    Unsupported { message: String },
}

impl JumpOutcome {
    pub fn message(&self) -> &str {
        match self {
            JumpOutcome::Focused { message }
            | JumpOutcome::Activated { message }
            | JumpOutcome::OpenedDirectory { message }
            | JumpOutcome::Failed { message }
            | JumpOutcome::Unsupported { message } => message,
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(
            self,
            JumpOutcome::Focused { .. } | JumpOutcome::Activated { .. }
        )
    }
}

// ── Jump strategy ─────────────────────────────────────────────────────────────

/// Planned jump strategy, resolved from the target before execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JumpStrategy {
    /// Use a deep-link URL (Codex.app).
    DeepLink { url: String },
    /// Use AppleScript to focus a terminal tab/pane.
    AppleScript { host: String },
    /// Use the WezTerm CLI (`wezterm cli activate-pane --pane-id <id>`).
    WeztermCli { pane_id: String },
    /// Use the Zellij CLI (`zellij action go-to-tab`).
    ZellijCli { session: String },
    /// Use a cmux Unix socket JSON-RPC call to focus a surface.
    CmuxRpc { surface_id: String },
    /// Use tmux CLI (`switch-client`, `select-window`, `select-pane`).
    TmuxCli { target: String },
    /// Use Warp SQLite + keyboard simulation.
    WarpSqlite,
    /// Use an editor CLI (`code -r <ws>`, `zed <ws>`, etc.).
    EditorCli { cli: String, workspace: String },
    /// Open a workspace path in an app by bundle id (`open -b <bundle> <ws>`).
    /// Preferred for editors when a precise bundle id is known: it needs no CLI
    /// on `PATH` and targets the exact app variant (preview / fork).
    OpenInApp { bundle_id: String, workspace: String },
    /// Activate the app by bundle id (`open -b <bundle>`).
    ActivateApp { bundle_id: String },
    /// Open a workspace/directory path via the app.
    OpenDir { path: String },
    /// No usable strategy.
    Unsupported,
    /// Session is running on a remote host; local focus is not possible.
    RemoteUnsupported { origin: String },
}

// ── plan_jump ─────────────────────────────────────────────────────────────────

/// Determine the best `JumpStrategy` for `target`, following the fallback
/// ladder from spec §14.
///
/// This is a pure function: it inspects the target's fields and descriptor,
/// and returns the *best available* strategy. Execution is separate.
pub fn plan_jump(target: &JumpTarget) -> JumpStrategy {
    use crate::jump::target::HostCategory;

    // Remote sessions cannot be focused from this machine.
    if target.origin != "local" {
        return JumpStrategy::RemoteUnsupported {
            origin: target.origin.clone(),
        };
    }

    let descriptor = descriptor_for(&target.host);
    let category = descriptor
        .map(|d| &d.category)
        .unwrap_or(&HostCategory::Unknown);

    match category {
        // ── Desktop app deep link (Codex.app) ────────────────────────────
        HostCategory::DesktopApp => {
            if let Some(ref thread_id) = target.codex_thread_id {
                JumpStrategy::DeepLink {
                    url: format!("codex://threads/{thread_id}"),
                }
            } else if let Some(bundle) = descriptor.and_then(|d| d.bundle_id) {
                JumpStrategy::ActivateApp {
                    bundle_id: bundle.to_string(),
                }
            } else {
                JumpStrategy::Unsupported
            }
        }
        // ── Multiplexers ─────────────────────────────────────────────────
        HostCategory::Tmux => {
            if let Some(ref tmux_target) = target.tmux_target {
                JumpStrategy::TmuxCli {
                    target: tmux_target.clone(),
                }
            } else {
                JumpStrategy::Unsupported
            }
        }
        HostCategory::Zellij => {
            if let Some(ref sess) = target.zellij_session {
                JumpStrategy::ZellijCli {
                    session: sess.clone(),
                }
            } else {
                JumpStrategy::Unsupported
            }
        }
        // ── Warp ─────────────────────────────────────────────────────────
        HostCategory::Warp => JumpStrategy::WarpSqlite,
        // ── Pure terminals (AppleScript / CLI) ───────────────────────────
        HostCategory::Terminal => {
            let name = &target.host;
            if matches!(name.as_str(), "wezterm" | "kaku")
                && let Some(pane_id) = target.wezterm_pane.as_deref()
            {
                return JumpStrategy::WeztermCli {
                    pane_id: pane_id.to_string(),
                };
            }
            if name == "cmux" {
                if let Some(sid) = target.terminal_session_id.as_deref() {
                    return JumpStrategy::CmuxRpc {
                        surface_id: sid.to_string(),
                    };
                }
                // No surface id captured → activate the app by bundle id.
                if let Some(bundle) = descriptor.and_then(|d| d.bundle_id) {
                    return JumpStrategy::ActivateApp {
                        bundle_id: bundle.to_string(),
                    };
                }
                return JumpStrategy::Unsupported;
            }
            // AppleScript path for Ghostty / iTerm2 / Terminal.app / others
            if let Some(bundle) = descriptor.and_then(|d| d.bundle_id) {
                // If we have a bundle id, prefer AppleScript (gives us tab-level focus).
                // We also know it's a supported app so AppleScript is safe.
                let _ = bundle;
                JumpStrategy::AppleScript {
                    host: target.host.clone(),
                }
            } else {
                JumpStrategy::Unsupported
            }
        }
        // ── Editors ───────────────────────────────────────────────────────
        HostCategory::Editor => {
            let ws = target.workspace_path.clone().or_else(|| {
                if !target.cwd.is_empty() {
                    Some(target.cwd.clone())
                } else {
                    None
                }
            });
            if let Some(workspace) = ws {
                // Prefer the precise host bundle id (macOS): `open -b <bundle> <ws>`
                // needs no CLI on PATH and targets the exact app variant
                // (e.g. Zed Preview, VS Code Insiders). Falls back to the editor
                // CLI when no bundle id was captured (e.g. on Linux).
                if let Some(bundle) = target.host_bundle_id.clone() {
                    JumpStrategy::OpenInApp {
                        bundle_id: bundle,
                        workspace,
                    }
                } else {
                    let cli = editor_cli_for(&target.host);
                    JumpStrategy::EditorCli { cli, workspace }
                }
            } else if let Some(bundle) = target
                .host_bundle_id
                .as_deref()
                .or_else(|| descriptor.and_then(|d| d.bundle_id))
            {
                JumpStrategy::ActivateApp {
                    bundle_id: bundle.to_string(),
                }
            } else {
                JumpStrategy::Unsupported
            }
        }
        HostCategory::Unknown => {
            // Try to activate by bundle id if we know it
            if let Some(bundle) = descriptor.and_then(|d| d.bundle_id) {
                JumpStrategy::ActivateApp {
                    bundle_id: bundle.to_string(),
                }
            } else {
                JumpStrategy::Unsupported
            }
        }
    }
}

/// CLI binary name for editor-hosted terminal jump.
fn editor_cli_for(host: &str) -> String {
    match host {
        "vscode" => "code".to_string(),
        "vscode-insiders" => "code-insiders".to_string(),
        "cursor" => "cursor".to_string(),
        "windsurf" => "windsurf".to_string(),
        "trae" => "trae".to_string(),
        "zed" => "zed".to_string(),
        // JetBrains: try to infer; generic fallback uses idea
        "jetbrains" => "idea".to_string(),
        _ => host.to_string(),
    }
}

// ── ExecutorTrait ─────────────────────────────────────────────────────────────

/// Abstraction over OS-level execution for testability.
///
/// The real implementation in `platform.rs` calls `Command::new` / `open` /
/// `osascript`; test implementations record calls and return mock results.
pub trait Executor: Send + Sync {
    /// Run a command with arguments. Returns (stdout, success).
    fn run(&self, cmd: &str, args: &[&str]) -> (String, bool);
    /// Open a URL (macOS `open <url>`).
    fn open_url(&self, url: &str) -> bool;
    /// Activate an app by bundle id (macOS `open -b <bundle>`).
    fn activate_app(&self, bundle_id: &str) -> bool;
    /// Open a path in an app by bundle id (macOS `open -b <bundle> <path>`).
    /// Default returns false (non-macOS / executors that don't support it).
    fn open_path_in_app(&self, _bundle_id: &str, _path: &str) -> bool {
        false
    }
    /// Run an AppleScript string and return (stdout, success).
    fn run_applescript(&self, script: &str) -> (String, bool);
    /// Open a directory path in Finder / file manager.
    fn open_dir(&self, path: &str) -> bool;
    /// Send a single-line JSON-RPC body to a Unix-domain socket (used by cmux).
    /// Returns whether the write succeeded. Default returns false for executors
    /// without socket support.
    fn send_unix_rpc(&self, _socket_path: &str, _body: &str) -> bool {
        false
    }
}

// ── jump ─────────────────────────────────────────────────────────────────────

/// Execute a jump using the given executor.
///
/// Applies the spec §14 fallback ladder:
///   exact pane → activate app → open dir via app → activate app → Finder → unsupported
pub fn jump(target: &JumpTarget, executor: &dyn Executor) -> JumpOutcome {
    let strategy = plan_jump(target);
    execute_strategy(strategy, target, executor)
}

fn execute_strategy(
    strategy: JumpStrategy,
    target: &JumpTarget,
    executor: &dyn Executor,
) -> JumpOutcome {
    match strategy {
        JumpStrategy::DeepLink { ref url } => {
            if executor.open_url(url) {
                JumpOutcome::Focused {
                    message: format!("Opened {url}"),
                }
            } else {
                // Fallback: activate app
                fallback_activate(target, executor)
            }
        }
        JumpStrategy::TmuxCli {
            target: ref tmux_cli_target,
        } => terminals::jump_tmux(tmux_cli_target, executor)
            .unwrap_or_else(|| fallback_activate(target, executor)),
        JumpStrategy::ZellijCli { ref session } => terminals::jump_zellij(session, executor)
            .unwrap_or_else(|| fallback_activate(target, executor)),
        JumpStrategy::WeztermCli { ref pane_id } => terminals::jump_wezterm(pane_id, executor)
            .unwrap_or_else(|| fallback_activate(target, executor)),
        JumpStrategy::CmuxRpc { ref surface_id } => terminals::jump_cmux(surface_id, executor)
            .unwrap_or_else(|| fallback_activate(target, executor)),
        JumpStrategy::WarpSqlite => warp::jump_warp(target, executor),
        JumpStrategy::AppleScript { ref host } => {
            terminals::jump_applescript(host, target, executor)
                .unwrap_or_else(|| fallback_activate(target, executor))
        }
        JumpStrategy::EditorCli {
            ref cli,
            ref workspace,
        } => editors::jump_editor(cli, workspace, executor),
        JumpStrategy::OpenInApp {
            ref bundle_id,
            ref workspace,
        } => {
            if executor.open_path_in_app(bundle_id, workspace) {
                JumpOutcome::Activated {
                    message: format!(
                        "Opened {workspace} in {bundle_id} — embedded terminal tab is best-effort"
                    ),
                }
            } else {
                fallback_activate(target, executor)
            }
        }
        JumpStrategy::ActivateApp { ref bundle_id } => {
            if executor.activate_app(bundle_id) {
                JumpOutcome::Activated {
                    message: format!("Activated {bundle_id}"),
                }
            } else {
                fallback_dir(target, executor)
            }
        }
        JumpStrategy::OpenDir { ref path } => {
            if executor.open_dir(path) {
                JumpOutcome::OpenedDirectory {
                    message: format!("Opened directory {path}"),
                }
            } else {
                JumpOutcome::Failed {
                    message: "Could not open directory".to_string(),
                }
            }
        }
        JumpStrategy::Unsupported => JumpOutcome::Unsupported {
            message: format!("Jump not supported for host '{}'", target.host),
        },
        JumpStrategy::RemoteUnsupported { ref origin } => JumpOutcome::Unsupported {
            message: format!(
                "agent on {origin} — SSH into {origin} to interact with this session"
            ),
        },
    }
}

fn fallback_activate(target: &JumpTarget, executor: &dyn Executor) -> JumpOutcome {
    let descriptor = descriptor_for(&target.host);
    // Prefer the precise captured bundle id over the descriptor's static one,
    // so preview / fork variants activate the right app.
    let bundle = target
        .host_bundle_id
        .as_deref()
        .or_else(|| descriptor.and_then(|d| d.bundle_id));
    if bundle.is_some_and(|b| executor.activate_app(b)) {
        return JumpOutcome::Activated {
            message: format!(
                "Activated {}",
                descriptor.map(|d| d.display_name).unwrap_or(&target.host)
            ),
        };
    }
    fallback_dir(target, executor)
}

fn fallback_dir(target: &JumpTarget, executor: &dyn Executor) -> JumpOutcome {
    let dir = target
        .workspace_path
        .as_deref()
        .or(if target.cwd.is_empty() {
            None
        } else {
            Some(target.cwd.as_str())
        });
    if dir.is_some_and(|path| executor.open_dir(path)) {
        let path = dir.unwrap();
        return JumpOutcome::OpenedDirectory {
            message: format!("Opened {path} in Finder"),
        };
    }
    JumpOutcome::Failed {
        message: format!("No jump path available for '{}'", target.host),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ── Mock executor ─────────────────────────────────────────────────────

    #[derive(Default)]
    struct MockExecutor {
        calls: Mutex<Vec<String>>,
        /// Commands that should succeed (substring match on cmd+args joined).
        succeed_patterns: Vec<String>,
    }

    impl MockExecutor {
        fn new_succeed_all() -> Self {
            MockExecutor {
                succeed_patterns: vec!["".to_string()], // empty pattern matches all
                ..Default::default()
            }
        }

        fn new_fail_all() -> Self {
            MockExecutor::default()
        }

        fn recorded_calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn record(&self, s: String) {
            self.calls.lock().unwrap().push(s);
        }

        fn matches_succeed(&self, key: &str) -> bool {
            self.succeed_patterns
                .iter()
                .any(|p| p.is_empty() || key.contains(p.as_str()))
        }
    }

    impl Executor for MockExecutor {
        fn run(&self, cmd: &str, args: &[&str]) -> (String, bool) {
            let key = format!("{cmd} {}", args.join(" "));
            self.record(format!("run:{key}"));
            let ok = self.matches_succeed(&key);
            (String::new(), ok)
        }

        fn open_url(&self, url: &str) -> bool {
            self.record(format!("open_url:{url}"));
            self.matches_succeed(url)
        }

        fn activate_app(&self, bundle_id: &str) -> bool {
            self.record(format!("activate_app:{bundle_id}"));
            self.matches_succeed(bundle_id)
        }

        fn open_path_in_app(&self, bundle_id: &str, path: &str) -> bool {
            let key = format!("{bundle_id} {path}");
            self.record(format!("open_path_in_app:{key}"));
            self.matches_succeed(&key)
        }

        fn run_applescript(&self, script: &str) -> (String, bool) {
            self.record(format!("applescript:{script}"));
            let ok = self.matches_succeed(script);
            (String::new(), ok)
        }

        fn open_dir(&self, path: &str) -> bool {
            self.record(format!("open_dir:{path}"));
            self.matches_succeed(path)
        }

        fn send_unix_rpc(&self, socket_path: &str, body: &str) -> bool {
            let key = format!("{socket_path} {body}");
            self.record(format!("send_unix_rpc:{key}"));
            self.matches_succeed(&key)
        }
    }

    fn make_target(host: &str) -> JumpTarget {
        JumpTarget {
            session_id: "sess-1".to_string(),
            host: host.to_string(),
            host_bundle_id: None,
            terminal_session_id: Some("iterm-sess".to_string()),
            terminal_tty: Some("/dev/ttys001".to_string()),
            tmux_target: Some("/tmp/tmux-1000/default,42,0|%3".to_string()),
            wezterm_pane: Some("7".to_string()),
            zellij_session: Some("dev".to_string()),
            workspace_path: Some("/home/user/project".to_string()),
            codex_thread_id: Some("thread-abc".to_string()),
            pane_title: None,
            cwd: "/home/user/project".to_string(),
            origin: "local".to_string(),
        }
    }

    // ── plan_jump tests ─────────────────────────────────────────────────

    #[test]
    fn plan_jump_codex_with_thread_id_gives_deep_link() {
        let t = make_target("codex");
        let s = plan_jump(&t);
        assert!(
            matches!(s, JumpStrategy::DeepLink { ref url } if url.contains("thread-abc")),
            "expected DeepLink with thread id, got: {s:?}"
        );
    }

    #[test]
    fn plan_jump_codex_no_thread_id_gives_activate_app() {
        let mut t = make_target("codex");
        t.codex_thread_id = None;
        let s = plan_jump(&t);
        assert!(
            matches!(s, JumpStrategy::ActivateApp { .. }),
            "expected ActivateApp fallback, got: {s:?}"
        );
    }

    #[test]
    fn plan_jump_tmux_with_target_gives_tmux_cli() {
        let t = make_target("tmux");
        let s = plan_jump(&t);
        assert!(
            matches!(s, JumpStrategy::TmuxCli { .. }),
            "expected TmuxCli, got: {s:?}"
        );
    }

    #[test]
    fn plan_jump_tmux_no_target_gives_unsupported() {
        let mut t = make_target("tmux");
        t.tmux_target = None;
        let s = plan_jump(&t);
        assert_eq!(s, JumpStrategy::Unsupported);
    }

    #[test]
    fn plan_jump_zellij_gives_zellij_cli() {
        let t = make_target("zellij");
        let s = plan_jump(&t);
        assert!(
            matches!(s, JumpStrategy::ZellijCli { .. }),
            "expected ZellijCli, got: {s:?}"
        );
    }

    #[test]
    fn plan_jump_wezterm_with_pane_gives_wezterm_cli() {
        let t = make_target("wezterm");
        let s = plan_jump(&t);
        assert!(
            matches!(s, JumpStrategy::WeztermCli { .. }),
            "expected WeztermCli, got: {s:?}"
        );
    }

    #[test]
    fn plan_jump_cmux_with_surface_id_gives_cmux_rpc() {
        // make_target sets terminal_session_id, which carries the cmux surface id.
        let t = make_target("cmux");
        let s = plan_jump(&t);
        assert!(
            matches!(s, JumpStrategy::CmuxRpc { ref surface_id } if surface_id == "iterm-sess"),
            "expected CmuxRpc with surface id, got: {s:?}"
        );
    }

    #[test]
    fn plan_jump_cmux_no_surface_id_falls_back_to_activate_app() {
        let mut t = make_target("cmux");
        t.terminal_session_id = None;
        let s = plan_jump(&t);
        assert!(
            matches!(s, JumpStrategy::ActivateApp { ref bundle_id } if bundle_id == "com.cmuxterm.app"),
            "expected ActivateApp fallback when no surface id, got: {s:?}"
        );
    }

    #[test]
    fn cmux_rpc_success_gives_focused_and_sends_socket() {
        let t = make_target("cmux");
        let exec = MockExecutor::new_succeed_all();
        let outcome = jump(&t, &exec);
        assert!(
            matches!(outcome, JumpOutcome::Focused { .. }),
            "expected Focused, got: {outcome:?}"
        );
        let calls = exec.recorded_calls();
        assert!(
            calls.iter().any(|c| c.starts_with("send_unix_rpc:")),
            "should have sent a unix-socket RPC; calls: {calls:?}"
        );
    }

    #[test]
    fn cmux_rpc_socket_failure_falls_back_to_activate_app() {
        // Socket write fails; cmux has a bundle id, so we activate the app.
        let t = make_target("cmux");
        let exec = MockExecutor {
            succeed_patterns: vec!["com.cmuxterm.app".to_string()],
            ..Default::default()
        };
        let outcome = jump(&t, &exec);
        assert!(
            matches!(outcome, JumpOutcome::Activated { .. }),
            "expected Activated fallback, got: {outcome:?}"
        );
    }

    #[test]
    fn plan_jump_warp_gives_warp_sqlite() {
        let t = make_target("warp");
        let s = plan_jump(&t);
        assert_eq!(s, JumpStrategy::WarpSqlite);
    }

    #[test]
    fn plan_jump_ghostty_gives_applescript() {
        let t = make_target("ghostty");
        let s = plan_jump(&t);
        assert!(
            matches!(s, JumpStrategy::AppleScript { .. }),
            "expected AppleScript, got: {s:?}"
        );
    }

    #[test]
    fn plan_jump_cursor_with_workspace_gives_editor_cli() {
        let t = make_target("cursor");
        let s = plan_jump(&t);
        assert!(
            matches!(s, JumpStrategy::EditorCli { ref cli, .. } if cli == "cursor"),
            "expected EditorCli cursor, got: {s:?}"
        );
    }

    #[test]
    fn plan_jump_zed_with_workspace_gives_editor_cli() {
        let t = make_target("zed");
        let s = plan_jump(&t);
        assert!(
            matches!(s, JumpStrategy::EditorCli { ref cli, .. } if cli == "zed"),
            "expected EditorCli zed, got: {s:?}"
        );
    }

    #[test]
    fn plan_jump_editor_with_bundle_id_prefers_open_in_app() {
        // A precise host bundle id (e.g. Zed Preview) takes priority over the
        // editor CLI, so jump works without the `zed` CLI on PATH.
        let mut t = make_target("zed");
        t.host_bundle_id = Some("dev.zed.Zed-Preview".to_string());
        let s = plan_jump(&t);
        assert!(
            matches!(
                s,
                JumpStrategy::OpenInApp { ref bundle_id, .. }
                    if bundle_id == "dev.zed.Zed-Preview"
            ),
            "expected OpenInApp with preview bundle, got: {s:?}"
        );
    }

    #[test]
    fn editor_open_in_app_success_gives_activated() {
        let mut t = make_target("zed");
        t.host_bundle_id = Some("dev.zed.Zed-Preview".to_string());
        let exec = MockExecutor::new_succeed_all();
        let outcome = jump(&t, &exec);
        assert!(
            matches!(outcome, JumpOutcome::Activated { .. }),
            "expected Activated, got: {outcome:?}"
        );
        let calls = exec.recorded_calls();
        assert!(
            calls
                .iter()
                .any(|c| c.contains("open_path_in_app:dev.zed.Zed-Preview")),
            "should have called open_path_in_app with the preview bundle; calls: {calls:?}"
        );
    }

    #[test]
    fn plan_jump_unknown_host_gives_unsupported() {
        let t = make_target("unknown");
        let s = plan_jump(&t);
        assert_eq!(s, JumpStrategy::Unsupported);
    }

    // ── fallback ladder tests ───────────────────────────────────────────

    #[test]
    fn codex_deep_link_success_gives_focused() {
        let t = make_target("codex");
        let exec = MockExecutor::new_succeed_all();
        let outcome = jump(&t, &exec);
        assert!(
            matches!(outcome, JumpOutcome::Focused { .. }),
            "expected Focused, got: {outcome:?}"
        );
        let calls = exec.recorded_calls();
        assert!(
            calls
                .iter()
                .any(|c| c.contains("codex://threads/thread-abc")),
            "should have called open_url with codex deep link; calls: {calls:?}"
        );
    }

    #[test]
    fn codex_deep_link_failure_falls_back_to_activate_app() {
        let mut t = make_target("codex");
        t.workspace_path = None;
        t.cwd = String::new();
        let exec = MockExecutor::new_fail_all();
        let outcome = jump(&t, &exec);
        let calls = exec.recorded_calls();
        assert!(
            calls.iter().any(|c| c.contains("activate_app")),
            "should have called activate_app as fallback; calls: {calls:?}"
        );
        // All fail → Failed
        assert!(
            matches!(outcome, JumpOutcome::Failed { .. }),
            "expected Failed when all fallbacks fail, got: {outcome:?}"
        );
    }

    #[test]
    fn fallback_ladder_opens_dir_when_activate_fails() {
        // Unknown host with workspace path; activate fails but open_dir succeeds.
        let mut t = make_target("unknown");
        t.host = "iterm2".to_string(); // has bundle_id
        t.workspace_path = Some("/home/user/project".to_string());

        // succeed_patterns match against the path string — use a substring of the path
        let exec = MockExecutor {
            succeed_patterns: vec!["/home/user/project".to_string()],
            ..Default::default()
        };
        let outcome = jump(&t, &exec);
        assert!(
            matches!(outcome, JumpOutcome::OpenedDirectory { .. }),
            "expected OpenedDirectory fallback, got: {outcome:?}"
        );
    }

    #[test]
    fn unsupported_host_gives_unsupported_outcome() {
        let t = make_target("unknown");
        let exec = MockExecutor::new_fail_all();
        let outcome = jump(&t, &exec);
        assert!(
            matches!(outcome, JumpOutcome::Unsupported { .. }),
            "expected Unsupported, got: {outcome:?}"
        );
    }

    // ── Remote origin degradation tests ────────────────────────────────────

    #[test]
    fn plan_jump_remote_origin_gives_remote_unsupported_strategy() {
        // Any well-known host on a remote origin should degrade before any
        // platform-specific path is attempted.
        for host in &["ghostty", "tmux", "cursor", "codex", "warp"] {
            let mut t = make_target(host);
            t.origin = "devbox1".to_string();
            let s = plan_jump(&t);
            assert!(
                matches!(s, JumpStrategy::RemoteUnsupported { .. }),
                "host={host}: expected RemoteUnsupported, got: {s:?}"
            );
        }
    }

    #[test]
    fn plan_jump_local_origin_not_degraded() {
        // Local sessions must continue to walk the normal strategy ladder.
        let t = make_target("ghostty"); // origin = "local"
        let s = plan_jump(&t);
        assert!(
            matches!(s, JumpStrategy::AppleScript { .. }),
            "expected AppleScript for local ghostty, got: {s:?}"
        );
    }

    #[test]
    fn jump_remote_origin_returns_unsupported_outcome_with_hint() {
        let mut t = make_target("ghostty");
        t.origin = "devbox1".to_string();
        let exec = MockExecutor::new_succeed_all();
        let outcome = jump(&t, &exec);
        // Must be Unsupported (not Focused / Activated / etc.)
        assert!(
            matches!(outcome, JumpOutcome::Unsupported { .. }),
            "expected Unsupported, got: {outcome:?}"
        );
        // Message must name the remote host so the user knows where to SSH.
        let msg = outcome.message();
        assert!(
            msg.contains("devbox1"),
            "message should mention the remote origin 'devbox1'; got: {msg:?}"
        );
    }

    #[test]
    fn jump_remote_origin_does_not_invoke_executor() {
        // No OS-level calls must happen for a remote session.
        let mut t = make_target("ghostty");
        t.origin = "devbox1".to_string();
        let exec = MockExecutor::new_succeed_all();
        let _outcome = jump(&t, &exec);
        let calls = exec.recorded_calls();
        assert!(
            calls.is_empty(),
            "expected zero executor calls for remote session, got: {calls:?}"
        );
    }
}
