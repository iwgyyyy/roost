use serde::{Deserialize, Serialize};

// ── Event summary (peek timeline) ─────────────────────────────────────────────

/// A single summarised event in the peek timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Seconds elapsed since this event was recorded (relative to "now" when serialised).
    pub ts_secs: u64,
    /// Single-line human-readable summary.
    pub summary: String,
    /// Static duration this step took, in seconds (= time until the next event).
    /// `None` for the most-recent event (still in progress or no successor).
    #[serde(default)]
    pub duration_secs: Option<u64>,
}

// ── SessionDetail (returned by Request::Detail) ───────────────────────────────

/// Full detail for one session, returned in response to `Request::Detail`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDetail {
    pub id: String,
    pub family: AgentFamily,
    /// Repository / cwd basename (same as `SessionView::name`).
    pub name: String,
    /// Agent display string, e.g. "✻ Claude Code".
    pub agent_display: String,
    /// Full working directory path.
    pub cwd: String,
    /// State full label string (e.g. "working", "approval").
    pub state: String,
    /// Current action / activity text (if any).
    pub action: Option<String>,
    /// Reason the session is waiting (for approval/question states).
    pub waiting_reason: Option<String>,
    /// Seconds the session has been in its current state.
    pub since_secs: u64,
    /// Recent events in reverse-chronological order (newest first).
    pub recent_events: Vec<Event>,
    /// Origin label: `"local"` for local sessions; device name for remote ones.
    /// Matches `SessionView::origin`.
    #[serde(default = "default_origin")]
    pub origin: String,
    /// Host application name (e.g. `"cursor"`, `"ghostty"`). `None` when unknown.
    #[serde(default)]
    pub host_app: Option<String>,
    /// Whether the remote session is stale / disconnected.
    /// `false` for local sessions or live remote sessions.
    #[serde(default)]
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentFamily {
    Claude,
    Codex,
    Deepseek,
    Cursor,
    Unknown,
}

impl AgentFamily {
    /// Unicode glyph for this family.
    ///
    /// Returns `(icon_str, display_label)`. Colours live in `tui::theme` to
    /// avoid pulling rendering concerns into the protocol layer.
    pub fn icon(&self) -> &'static str {
        match self {
            AgentFamily::Claude => "✻",
            AgentFamily::Codex => "⬡",
            AgentFamily::Deepseek => "≈",
            AgentFamily::Cursor => "❯",
            AgentFamily::Unknown => "◆",
        }
    }

    /// Short display label for this family (e.g. "Claude Code").
    pub fn display_label(&self) -> &'static str {
        match self {
            AgentFamily::Claude => "Claude Code",
            AgentFamily::Codex => "Codex",
            AgentFamily::Deepseek => "DeepSeek",
            AgentFamily::Cursor => "Cursor",
            AgentFamily::Unknown => "Unknown",
        }
    }

    /// Short family label for TUI display (fits in ~6 display columns).
    pub fn short_label(&self) -> &'static str {
        match self {
            AgentFamily::Claude => "Claude",
            AgentFamily::Codex => "Codex",
            AgentFamily::Deepseek => "DeepSeek",
            AgentFamily::Cursor => "Cursor",
            AgentFamily::Unknown => "agent",
        }
    }

    /// Label shown in the card top border (left-upper corner).
    /// Uses short, recognisable names; DeepSeek (CodeWhale) shows "CodeWhale".
    pub fn card_label(&self) -> &'static str {
        match self {
            AgentFamily::Claude => "Claude",
            AgentFamily::Codex => "Codex",
            AgentFamily::Deepseek => "CodeWhale",
            AgentFamily::Cursor => "Cursor",
            AgentFamily::Unknown => "agent",
        }
    }

    /// Stable machine identifier (matches the serde `snake_case` representation).
    /// Used to persist the family in the history database as TEXT.
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentFamily::Claude => "claude",
            AgentFamily::Codex => "codex",
            AgentFamily::Deepseek => "deepseek",
            AgentFamily::Cursor => "cursor",
            AgentFamily::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    // ---- Shared / Claude ----
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Notification,
    Stop,
    SessionEnd,
    // ---- Codex-specific ----
    /// Codex PermissionRequest hook event (replaces legacy approval-requested).
    PermissionRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEvent {
    pub kind: EventKind,
    pub family: AgentFamily,
    pub session_id: String,
    pub cwd: String,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub notification: Option<String>,

    // ── Phase 4: jump locator fields (§14) ───────────────────────────────
    /// The detected host application (e.g. "ghostty", "iterm2", "cursor", "warp").
    #[serde(default)]
    pub host_app: Option<String>,
    /// Precise macOS bundle id of the host app (from `__CFBundleIdentifier`),
    /// e.g. "dev.zed.Zed-Preview". Used for accurate `open -b` jump regardless
    /// of stable/preview/fork variants. `None` on Linux.
    #[serde(default)]
    pub host_bundle_id: Option<String>,
    /// Terminal session id provided by the host (iTerm2 / Ghostty).
    #[serde(default)]
    pub terminal_session_id: Option<String>,
    /// TTY device path (e.g. "/dev/ttys003").
    #[serde(default)]
    pub terminal_tty: Option<String>,
    /// tmux target string: "session:window.pane" plus socket path.
    #[serde(default)]
    pub tmux_target: Option<String>,
    /// WezTerm pane id (from $WEZTERM_PANE).
    #[serde(default)]
    pub wezterm_pane: Option<String>,
    /// Zellij session name (from $ZELLIJ_SESSION_NAME).
    #[serde(default)]
    pub zellij_session: Option<String>,
    /// Workspace / project path for editor-hosted terminals.
    #[serde(default)]
    pub workspace_path: Option<String>,
    /// Codex.app thread id (= session_id when host is Codex.app).
    #[serde(default)]
    pub codex_thread_id: Option<String>,
    /// Pane title at hook time.
    #[serde(default)]
    pub pane_title: Option<String>,

    // ── Remote-fleet fields ──────────────────────────────────────────────────
    /// Origin label of the machine this hook was fired on.
    /// `None` on local hooks (no `--origin` flag / `ROOST_ORIGIN` env);
    /// set to a non-empty string for remote agents.
    #[serde(default)]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Report { event: Box<HookEvent> },
    List,
    Detail { session_id: String },
    /// Notify the daemon that a new remote has been added.
    ///
    /// The daemon will call `Supervisor::add` to start a tunnel watcher
    /// immediately, without needing a restart.  If the daemon is not
    /// running, the `roost add` command writes the entry to `remotes.json`
    /// and the watcher is started on next daemon boot.
    AddRemote {
        target: String,
        origin: String,
        remote_sock: String,
        arch: String,
    },
    /// Notify the daemon to stop and remove the tunnel for a remote.
    ///
    /// `origin` identifies the remote to remove (the registry key).
    /// The daemon will call `Supervisor::remove(origin)` immediately.
    RemoveRemote { origin: String },
    /// Query the daemon for all registered remotes and their tunnel states.
    ///
    /// The daemon replies with a JSON array of [`RemoteStatus`] entries.
    ListRemotes,
}

/// Tunnel connectivity state as reported by the running daemon.
///
/// Mirrors [`crate::remote_supervisor::ConnState`] but lives in the protocol
/// layer so it can be serialized without depending on the supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelState {
    /// SSH tunnel is up.
    Up,
    /// SSH tunnel exited; waiting to reconnect.
    Reconnecting,
    /// Reconnect attempts exhausted; no longer trying.
    Down,
    /// Daemon is not running or did not report state for this remote.
    Unknown,
}

impl std::fmt::Display for TunnelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TunnelState::Up => write!(f, "up"),
            TunnelState::Reconnecting => write!(f, "reconnecting"),
            TunnelState::Down => write!(f, "down"),
            TunnelState::Unknown => write!(f, "unknown"),
        }
    }
}

/// Status of a single registered remote, returned in response to [`Request::ListRemotes`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteStatus {
    /// Human-readable label (registry key).
    pub origin: String,
    /// SSH target, e.g. `user@devbox1`.
    pub target: String,
    /// Architecture tag.
    pub arch: String,
    /// When this remote was first added (ISO-8601).
    pub added_at: String,
    /// Current tunnel state as known by the daemon.
    pub tunnel: TunnelState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionView {
    pub id: String,
    pub family: AgentFamily,
    /// Repository / cwd basename (末段目录名). Preserved for backwards compatibility.
    pub name: String,
    /// Full working directory path (e.g. `/home/dev/acme/payments-api`).
    /// Defaults to empty string when deserializing old JSON that lacks this field.
    #[serde(default)]
    pub cwd: String,
    pub state: String,
    pub activity: Option<String>,
    pub last_activity_secs: u64,
    /// Cumulative active duration (working/approval/question) in seconds.
    /// Increments while the session is active; freezes when done/idle;
    /// resumes from the frozen value when the session becomes active again.
    #[serde(default)]
    pub busy_secs: u64,
    /// First prompt the user submitted in this session (used as mid column summary).
    #[serde(default)]
    pub last_prompt: Option<String>,
    /// Seconds since the user's most recent prompt (UserPromptSubmit). `None` if
    /// the user has not prompted yet. Drives stable within-group ordering
    /// (smallest = most recent on top); changes only when the user sends a message.
    #[serde(default)]
    pub last_prompt_age_secs: Option<u64>,

    // ── Phase 4: jump locator fields ────────────────────────────────────
    #[serde(default)]
    pub host_app: Option<String>,
    #[serde(default)]
    pub host_bundle_id: Option<String>,
    #[serde(default)]
    pub terminal_session_id: Option<String>,
    #[serde(default)]
    pub terminal_tty: Option<String>,
    #[serde(default)]
    pub tmux_target: Option<String>,
    #[serde(default)]
    pub wezterm_pane: Option<String>,
    #[serde(default)]
    pub zellij_session: Option<String>,
    #[serde(default)]
    pub workspace_path: Option<String>,
    #[serde(default)]
    pub codex_thread_id: Option<String>,
    #[serde(default)]
    pub pane_title: Option<String>,

    // ── Fleet display fields ─────────────────────────────────────────────────
    /// The machine this session runs on. Defaults to `"local"` for sessions
    /// observed on the local machine; remote sessions will carry a host label.
    #[serde(default = "default_origin")]
    pub origin: String,
    /// Whether this session is considered stale / disconnected. Defaults to
    /// `false` (connected). Filled by remote-fleet tracking in a later phase.
    #[serde(default)]
    pub stale: bool,
}

fn default_origin() -> String {
    "local".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_event_roundtrip() {
        let ev = HookEvent {
            kind: EventKind::PreToolUse,
            family: AgentFamily::Claude,
            session_id: "sess-123".to_string(),
            cwd: "/home/user/project".to_string(),
            pid: Some(42),
            tool_name: Some("Edit".to_string()),
            tool_input: Some(r#"{"path":"src/main.rs"}"#.to_string()),
            prompt: None,
            notification: None,
            host_app: None,
            host_bundle_id: None,
            terminal_session_id: None,
            terminal_tty: None,
            tmux_target: None,
            wezterm_pane: None,
            zellij_session: None,
            workspace_path: None,
            codex_thread_id: None,
            pane_title: None,
            origin: None,
        };

        let json = serde_json::to_string(&ev).unwrap();
        let decoded: HookEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.kind, EventKind::PreToolUse);
        assert_eq!(decoded.family, AgentFamily::Claude);
        assert_eq!(decoded.session_id, "sess-123");
        assert_eq!(decoded.cwd, "/home/user/project");
        assert_eq!(decoded.pid, Some(42));
        assert_eq!(decoded.tool_name.as_deref(), Some("Edit"));
        assert_eq!(decoded.notification, None);
    }

    #[test]
    fn hook_event_optional_fields_default() {
        let json = r#"{"kind":"session_start","family":"claude","session_id":"s1","cwd":"/tmp"}"#;
        let ev: HookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.kind, EventKind::SessionStart);
        assert_eq!(ev.pid, None);
        assert_eq!(ev.tool_name, None);
        assert_eq!(ev.prompt, None);
    }

    #[test]
    fn request_report_tagged() {
        let ev = HookEvent {
            kind: EventKind::Stop,
            family: AgentFamily::Codex,
            session_id: "s2".to_string(),
            cwd: "/tmp".to_string(),
            pid: None,
            tool_name: None,
            tool_input: None,
            prompt: None,
            notification: None,
            host_app: None,
            host_bundle_id: None,
            terminal_session_id: None,
            terminal_tty: None,
            tmux_target: None,
            wezterm_pane: None,
            zellij_session: None,
            workspace_path: None,
            codex_thread_id: None,
            pane_title: None,
            origin: None,
        };
        let req = Request::Report {
            event: Box::new(ev),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""type":"report""#));
        let _decoded: Request = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn request_list_tagged() {
        let req = Request::List;
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""type":"list""#));
        let _decoded: Request = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn session_view_roundtrip() {
        let sv = SessionView {
            id: "s1".to_string(),
            family: AgentFamily::Claude,
            name: "my-repo".to_string(),
            cwd: "/home/user/dev/my-repo".to_string(),
            state: "working".to_string(),
            activity: Some("思考中…".to_string()),
            last_activity_secs: 5,
            busy_secs: 0,
            last_prompt: None,
            last_prompt_age_secs: None,
            host_app: None,
            host_bundle_id: None,
            terminal_session_id: None,
            terminal_tty: None,
            tmux_target: None,
            wezterm_pane: None,
            zellij_session: None,
            workspace_path: None,
            codex_thread_id: None,
            pane_title: None,
            origin: "local".to_string(),
            stale: false,
        };
        let json = serde_json::to_string(&sv).unwrap();
        let decoded: SessionView = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.name, "my-repo");
        assert_eq!(decoded.cwd, "/home/user/dev/my-repo");
        assert_eq!(decoded.activity.as_deref(), Some("思考中…"));
    }

    /// cwd 字段不出现在旧 JSON 中时，应反序列化为默认空串（向后兼容）。
    #[test]
    fn session_view_cwd_defaults_to_empty_when_absent() {
        // Old JSON without "cwd" field must deserialize without error and yield "".
        let json = r#"{
            "id":"s1","family":"claude","name":"repo","state":"idle",
            "last_activity_secs":0
        }"#;
        let sv: SessionView = serde_json::from_str(json).unwrap();
        assert_eq!(sv.cwd, "", "missing cwd must default to empty string");
        // name (basename) must still be intact
        assert_eq!(sv.name, "repo");
    }

    /// name 仍是末段（basename），cwd 是完整路径，两者独立。
    #[test]
    fn session_view_name_is_basename_cwd_is_full_path() {
        let sv = SessionView {
            id: "s2".to_string(),
            family: AgentFamily::Claude,
            name: "payments-api".to_string(),
            cwd: "/home/dev/acme/payments-api".to_string(),
            state: "working".to_string(),
            activity: None,
            last_activity_secs: 0,
            busy_secs: 0,
            last_prompt: None,
            last_prompt_age_secs: None,
            host_app: None,
            host_bundle_id: None,
            terminal_session_id: None,
            terminal_tty: None,
            tmux_target: None,
            wezterm_pane: None,
            zellij_session: None,
            workspace_path: None,
            codex_thread_id: None,
            pane_title: None,
            origin: "local".to_string(),
            stale: false,
        };
        assert_eq!(sv.name, "payments-api", "name must remain the basename");
        assert_eq!(sv.cwd, "/home/dev/acme/payments-api", "cwd must be full path");
    }

    // ── HookEvent.origin roundtrip tests (Task A1) ────────────────────────────

    #[test]
    fn hook_event_origin_serializes_when_set() {
        let ev = HookEvent {
            kind: EventKind::SessionStart,
            family: AgentFamily::Claude,
            session_id: "s".to_string(),
            cwd: "/tmp".to_string(),
            pid: None,
            tool_name: None,
            tool_input: None,
            prompt: None,
            notification: None,
            host_app: None,
            host_bundle_id: None,
            terminal_session_id: None,
            terminal_tty: None,
            tmux_target: None,
            wezterm_pane: None,
            zellij_session: None,
            workspace_path: None,
            codex_thread_id: None,
            pane_title: None,
            origin: Some("devbox1".to_string()),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let decoded: HookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.origin.as_deref(), Some("devbox1"));
    }

    #[test]
    fn hook_event_origin_defaults_to_none_when_absent() {
        // Old JSON without "origin" field must deserialize without error and
        // yield None (backwards compatibility).
        let json = r#"{"kind":"session_start","family":"claude","session_id":"s","cwd":"/tmp"}"#;
        let ev: HookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.origin, None);
    }

    // ── New protocol variants (Task A6) ──────────────────────────────────────

    #[test]
    fn request_add_remote_roundtrip() {
        let req = Request::AddRemote {
            target: "user@devbox1".to_string(),
            origin: "devbox1".to_string(),
            remote_sock: "~/.roost/hub.sock".to_string(),
            arch: "linux-x86_64".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""type":"add_remote""#), "type tag: {json}");
        let decoded: Request = serde_json::from_str(&json).unwrap();
        match decoded {
            Request::AddRemote { target, origin, remote_sock, arch } => {
                assert_eq!(target, "user@devbox1");
                assert_eq!(origin, "devbox1");
                assert_eq!(remote_sock, "~/.roost/hub.sock");
                assert_eq!(arch, "linux-x86_64");
            }
            other => panic!("expected AddRemote, got {other:?}"),
        }
    }

    #[test]
    fn request_remove_remote_roundtrip() {
        let req = Request::RemoveRemote { origin: "devbox1".to_string() };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""type":"remove_remote""#), "type tag: {json}");
        let decoded: Request = serde_json::from_str(&json).unwrap();
        match decoded {
            Request::RemoveRemote { origin } => assert_eq!(origin, "devbox1"),
            other => panic!("expected RemoveRemote, got {other:?}"),
        }
    }

    #[test]
    fn request_list_remotes_roundtrip() {
        let req = Request::ListRemotes;
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""type":"list_remotes""#), "type tag: {json}");
        let _decoded: Request = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn tunnel_state_all_variants_roundtrip() {
        for (variant, expected_str) in [
            (TunnelState::Up, "up"),
            (TunnelState::Reconnecting, "reconnecting"),
            (TunnelState::Down, "down"),
            (TunnelState::Unknown, "unknown"),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            // serde should produce a JSON string equal to the snake_case variant
            let inner = json.trim_matches('"');
            assert_eq!(inner, expected_str, "serde output for {variant:?}: {json}");
            let decoded: TunnelState = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, variant);
            // Display impl
            assert_eq!(variant.to_string(), expected_str);
        }
    }

    #[test]
    fn remote_status_roundtrip() {
        let rs = RemoteStatus {
            origin: "devbox1".to_string(),
            target: "user@devbox1".to_string(),
            arch: "linux-x86_64".to_string(),
            added_at: "2026-06-03T00:00:00Z".to_string(),
            tunnel: TunnelState::Up,
        };
        let json = serde_json::to_string(&rs).unwrap();
        let decoded: RemoteStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.origin, "devbox1");
        assert_eq!(decoded.tunnel, TunnelState::Up);
    }
}
