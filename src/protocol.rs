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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentFamily {
    Claude,
    Codex,
    Deepseek,
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
            AgentFamily::Unknown => "◆",
        }
    }

    /// Short display label for this family (e.g. "Claude Code").
    pub fn display_label(&self) -> &'static str {
        match self {
            AgentFamily::Claude => "Claude Code",
            AgentFamily::Codex => "Codex",
            AgentFamily::Deepseek => "DeepSeek",
            AgentFamily::Unknown => "Unknown",
        }
    }

    /// Short family label for TUI display (fits in ~6 display columns).
    pub fn short_label(&self) -> &'static str {
        match self {
            AgentFamily::Claude => "Claude",
            AgentFamily::Codex => "Codex",
            AgentFamily::Deepseek => "DeepSeek",
            AgentFamily::Unknown => "agent",
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Report { event: Box<HookEvent> },
    List,
    Detail { session_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionView {
    pub id: String,
    pub family: AgentFamily,
    pub name: String,
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
        };
        let json = serde_json::to_string(&sv).unwrap();
        let decoded: SessionView = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.name, "my-repo");
        assert_eq!(decoded.activity.as_deref(), Some("思考中…"));
    }
}
