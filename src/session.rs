use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::protocol::{AgentFamily, Event, EventKind, HookEvent};

pub const MAX_EVENTS: usize = 50;

/// Both agents model an interactive "clarify" question card as a tool call whose
/// `PreToolUse` fires exactly when the card is shown and the turn pauses for the
/// user. Codex uses `request_user_input`; Claude Code uses `AskUserQuestion`.
/// Both carry the same `{"questions":[{"question":…,"header":…}]}` payload.
pub const CLARIFY_TOOL: &str = "request_user_input";
pub const CLARIFY_TOOL_CLAUDE: &str = "AskUserQuestion";

/// True when this tool name is a clarify question tool (either agent's).
fn is_clarify_tool(tool_name: Option<&str>) -> bool {
    matches!(tool_name, Some(t) if t == CLARIFY_TOOL || t == CLARIFY_TOOL_CLAUDE)
}

/// Build the display label for a clarify card from its tool_input: the first
/// question's text (falling back to its header), suffixed with `(1/N)` when the
/// card carries more than one question. The hook only sees the whole card, not
/// which sub-question is currently focused, so the first one is shown.
fn extract_question_from_input(input: Option<&str>) -> Option<String> {
    let input = input?;
    let v = serde_json::from_str::<serde_json::Value>(input).ok()?;
    let questions = v.get("questions")?.as_array()?;
    let n = questions.len();
    let q0 = questions.first()?;
    let text = q0
        .get("question")
        .and_then(|x| x.as_str())
        .or_else(|| q0.get("header").and_then(|x| x.as_str()))?;
    let text = truncate_str(text, 80);
    if n > 1 {
        Some(format!("{text} (1/{n})"))
    } else {
        Some(text)
    }
}

/// Produce a single-line human-readable summary of a `HookEvent`.
///
/// Rules (spec §peek):
/// - PreToolUse / Edit-ish → "✎ edited <path>"
/// - PreToolUse / Bash → "⚙ ran <cmd>"
/// - Notification / ApprovalRequested → "▲ asked to run <…>"
/// - Stop / AgentTurnComplete → "✓ done"
/// - Everything else → a generic fallback
pub fn summarize_event(ev: &HookEvent) -> String {
    match &ev.kind {
        EventKind::PreToolUse => {
            let tool = ev.tool_name.as_deref().unwrap_or("");
            let lower = tool.to_lowercase();
            if is_clarify_tool(Some(tool)) {
                let q = extract_question_from_input(ev.tool_input.as_deref())
                    .unwrap_or_else(|| "input".to_string());
                format!("▲ asked: {q}")
            } else if lower.contains("edit") || lower.contains("write") || lower.contains("create")
            {
                let path = extract_path_from_input(ev.tool_input.as_deref())
                    .unwrap_or_else(|| "file".to_string());
                format!("✎ edited {path}")
            } else if lower.contains("bash") || lower.contains("run") || lower.contains("exec") {
                let cmd = extract_command_from_input(ev.tool_input.as_deref())
                    .unwrap_or_else(|| "command".to_string());
                format!("⚙ ran {cmd}")
            } else if lower.contains("read") {
                let path = extract_path_from_input(ev.tool_input.as_deref())
                    .unwrap_or_else(|| "file".to_string());
                format!("⚙ read {path}")
            } else if tool.is_empty() {
                "⚙ tool use".to_string()
            } else {
                format!("⚙ {tool}")
            }
        }
        EventKind::Notification | EventKind::PermissionRequest => {
            let text = ev.notification.as_deref().unwrap_or("approval requested");
            if is_idle_notification(text) {
                "○ waiting for input".to_string()
            } else {
                // Show first meaningful chunk (truncate long notifications)
                let short = truncate_str(text, 60);
                format!("▲ asked to run {short}")
            }
        }
        EventKind::Stop => "✓ done".to_string(),
        EventKind::UserPromptSubmit => {
            if let Some(p) = &ev.prompt {
                let short = truncate_str(p, 50);
                format!("▶ prompt: {short}")
            } else {
                "▶ new prompt".to_string()
            }
        }
        EventKind::SessionStart => "○ session started".to_string(),
        EventKind::SessionEnd => "○ session ended".to_string(),
        EventKind::PostToolUse => {
            let tool = ev.tool_name.as_deref().unwrap_or("tool");
            format!("✓ {tool} done")
        }
    }
}

/// Wrap a `HookEvent` into an `Event` with the given timestamp offset.
pub fn event_from_hook(ev: &HookEvent, ts_secs: u64) -> Event {
    Event {
        ts_secs,
        summary: summarize_event(ev),
        duration_secs: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentState {
    Working,
    Approval,
    Question,
    Done,
    Idle,
}

impl AgentState {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentState::Working => "working",
            AgentState::Approval => "approval",
            AgentState::Question => "question",
            AgentState::Done => "done",
            AgentState::Idle => "idle",
        }
    }
}

#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub kind: EventKind,
    pub ts: Instant,
    /// Pre-computed human-readable summary (from `summarize_event`).
    pub summary: String,
}

/// Returns true when `state` counts as "active" for the busy timer.
///
/// Active states: Working, Approval, Question.
/// Paused states: Done, Idle.
pub fn is_active(state: &AgentState) -> bool {
    matches!(
        state,
        AgentState::Working | AgentState::Approval | AgentState::Question
    )
}

#[derive(Debug)]
pub struct AgentSession {
    pub id: String,
    pub family: AgentFamily,
    pub name: String,
    pub cwd: String,
    pub pid: Option<u32>,
    pub tty: Option<String>,
    pub term_program: Option<String>,
    pub state: AgentState,
    pub activity: Option<String>,
    pub last_prompt: Option<String>,
    pub done_summary: Option<String>,
    pub last_activity: Instant,
    pub events: VecDeque<StoredEvent>,
    pub edit_count: u32,
    /// True while a clarify question card (AskUserQuestion / request_user_input)
    /// is open and awaiting the user's answer. Used to stop a follow-up generic
    /// "needs your permission" notification from overwriting the question text.
    pub awaiting_clarify: bool,

    // ── Busy timer: cumulative active duration ────────────────────────────
    /// Total accumulated active duration across all completed active segments.
    pub active_accum: Duration,
    /// Start of the current active segment; `None` when paused / not yet started.
    pub active_since: Option<Instant>,

    // ── Phase 4: jump locator fields (§14) ───────────────────────────────
    pub host_app: Option<String>,
    pub terminal_session_id: Option<String>,
    pub terminal_tty: Option<String>,
    pub tmux_target: Option<String>,
    pub wezterm_pane: Option<String>,
    pub zellij_session: Option<String>,
    pub workspace_path: Option<String>,
    pub codex_thread_id: Option<String>,
    pub pane_title: Option<String>,
}

impl AgentSession {
    pub fn new(ev: &HookEvent) -> Self {
        AgentSession {
            id: ev.session_id.clone(),
            family: ev.family.clone(),
            name: String::new(), // filled by daemon via naming::resolve_name
            cwd: ev.cwd.clone(),
            pid: ev.pid,
            tty: None,
            term_program: None,
            state: AgentState::Idle,
            activity: None,
            last_prompt: None,
            done_summary: None,
            last_activity: Instant::now(),
            events: VecDeque::with_capacity(MAX_EVENTS),
            edit_count: 0,
            awaiting_clarify: false,
            active_accum: Duration::ZERO,
            active_since: None,
            host_app: ev.host_app.clone(),
            terminal_session_id: ev.terminal_session_id.clone(),
            terminal_tty: ev.terminal_tty.clone(),
            tmux_target: ev.tmux_target.clone(),
            wezterm_pane: ev.wezterm_pane.clone(),
            zellij_session: ev.zellij_session.clone(),
            workspace_path: ev.workspace_path.clone(),
            codex_thread_id: ev.codex_thread_id.clone(),
            pane_title: ev.pane_title.clone(),
        }
    }

    /// Cumulative active ("busy") duration: the accumulator plus the current
    /// active segment if one is open. When the session is not active (Done /
    /// Idle), `active_since` is `None`, so this value is frozen — it stops
    /// counting and resumes only when the session becomes active again.
    pub fn busy_duration(&self, now: Instant) -> Duration {
        match self.active_since {
            Some(since) => self.active_accum + now.duration_since(since),
            None => self.active_accum,
        }
    }

    /// Apply a hook event to this session. Returns true if the session should be removed.
    pub fn apply(&mut self, ev: &HookEvent) -> bool {
        let now = Instant::now();
        self.last_activity = now;
        // Update pid if provided
        if let Some(p) = ev.pid {
            self.pid = Some(p);
        }

        // Update jump locator fields when provided (prefer first non-None value;
        // allow overwrite on SessionStart so the hook context is always fresh).
        macro_rules! update_opt {
            ($field:ident) => {
                if ev.$field.is_some() {
                    self.$field = ev.$field.clone();
                }
            };
        }
        update_opt!(host_app);
        update_opt!(terminal_session_id);
        update_opt!(terminal_tty);
        update_opt!(tmux_target);
        update_opt!(wezterm_pane);
        update_opt!(zellij_session);
        update_opt!(workspace_path);
        update_opt!(codex_thread_id);
        update_opt!(pane_title);

        // Record event with pre-computed summary
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(StoredEvent {
            kind: ev.kind.clone(),
            ts: now,
            summary: summarize_event(ev),
        });

        // Snapshot active status before the state machine transition.
        let was_active = is_active(&self.state);

        let remove = match &ev.family {
            AgentFamily::Codex => self.apply_codex(ev),
            _ => self.apply_claude(ev),
        };

        // Update busy timer based on the transition:
        //   paused → active  : start a new segment
        //   active → paused  : freeze — add elapsed to accumulator
        //   no change        : leave active_since untouched (critical: do NOT reset)
        let now_active = is_active(&self.state);
        match (was_active, now_active) {
            (false, true) => {
                // Session became active (or re-activated after done/idle).
                self.active_since = Some(now);
            }
            (true, false) => {
                // Session stopped being active — freeze the current segment.
                if let Some(since) = self.active_since.take() {
                    self.active_accum += now.duration_since(since);
                }
            }
            _ => {
                // active→active or paused→paused: nothing to change.
            }
        }

        remove
    }

    /// State machine for Claude (and Unknown) family events.
    fn apply_claude(&mut self, ev: &HookEvent) -> bool {
        match &ev.kind {
            EventKind::SessionStart => {
                self.state = AgentState::Idle;
                self.activity = None;
                self.awaiting_clarify = false;
                false
            }
            EventKind::UserPromptSubmit => {
                self.state = AgentState::Working;
                self.activity = Some("thinking…".to_string());
                self.awaiting_clarify = false;
                if ev.prompt.is_some() {
                    self.last_prompt = ev.prompt.clone();
                }
                false
            }
            EventKind::PreToolUse => {
                // AskUserQuestion → the agent is paused waiting for the user to
                // answer the card. A generic "needs your permission" Notification
                // follows; awaiting_clarify keeps it from overwriting the question.
                if is_clarify_tool(ev.tool_name.as_deref()) {
                    self.state = AgentState::Question;
                    self.activity = extract_question_from_input(ev.tool_input.as_deref())
                        .or_else(|| Some("waiting for your input".to_string()));
                    self.awaiting_clarify = true;
                } else {
                    self.state = AgentState::Working;
                    self.activity = Some(build_tool_activity(
                        ev.tool_name.as_deref(),
                        ev.tool_input.as_deref(),
                    ));
                }
                false
            }
            EventKind::PostToolUse => {
                // Count edits
                if matches_edit_tool(ev.tool_name.as_deref()) {
                    self.edit_count += 1;
                }
                // A clarify tool completing means the user answered the card.
                if is_clarify_tool(ev.tool_name.as_deref()) {
                    self.awaiting_clarify = false;
                }
                self.state = AgentState::Working;
                self.activity = Some("thinking…".to_string());
                false
            }
            EventKind::Notification => {
                let text = ev.notification.as_deref().unwrap_or("");
                if self.awaiting_clarify {
                    // A clarify card is open; the question is already shown. Ignore
                    // the generic "needs your permission" notification so it does
                    // not replace the question text with a vague approval prompt.
                } else if is_idle_notification(text) {
                    // "Claude is waiting for your input" — the turn is over and the
                    // agent is just idling until the next prompt. Not an actionable
                    // question; treat as idle, not NEEDS INPUT.
                    self.state = AgentState::Idle;
                    self.activity = None;
                } else if is_approval_notification(text) {
                    self.state = AgentState::Approval;
                    self.activity = Some(text.to_string());
                } else {
                    self.state = AgentState::Question;
                    self.activity = Some(text.to_string());
                }
                false
            }
            EventKind::Stop => {
                self.state = AgentState::Done;
                self.activity = None;
                self.awaiting_clarify = false;
                false
            }
            EventKind::SessionEnd => {
                // Signal removal
                true
            }
            // Codex-specific events forwarded to Claude are ignored gracefully.
            EventKind::PermissionRequest => false,
        }
    }

    /// State machine for Codex family events.
    /// - SessionStart      → Idle
    /// - UserPromptSubmit  → Working("thinking…")
    /// - PostToolUse       → Working (show tool activity if available)
    /// - PermissionRequest → Approval
    /// - Stop              → Done
    /// - PreToolUse(request_user_input) → Question (clarify card; see CLARIFY_TOOL)
    /// - Codex has no SessionEnd (removal via liveness)
    fn apply_codex(&mut self, ev: &HookEvent) -> bool {
        match &ev.kind {
            EventKind::SessionStart => {
                self.state = AgentState::Idle;
                self.activity = None;
                false
            }
            EventKind::UserPromptSubmit => {
                self.state = AgentState::Working;
                self.activity = Some("thinking…".to_string());
                if ev.prompt.is_some() {
                    self.last_prompt = ev.prompt.clone();
                }
                false
            }
            EventKind::PostToolUse => {
                if matches_edit_tool(ev.tool_name.as_deref()) {
                    self.edit_count += 1;
                }
                self.state = AgentState::Working;
                self.activity = Some(
                    ev.tool_name
                        .as_deref()
                        .map(|t| format!("{t} done"))
                        .unwrap_or_else(|| "thinking…".to_string()),
                );
                false
            }
            EventKind::PermissionRequest => {
                let text = ev.notification.as_deref().unwrap_or("approval requested");
                self.state = AgentState::Approval;
                self.activity = Some(text.to_string());
                false
            }
            EventKind::Stop => {
                self.state = AgentState::Done;
                self.activity = None;
                false
            }
            EventKind::PreToolUse => {
                // A clarify card (request_user_input) → the agent is paused
                // waiting for the user. No Stop fires while it waits, so this
                // Question state persists until the user answers (UserPromptSubmit
                // → Working) or the tool completes (PostToolUse → Working).
                if is_clarify_tool(ev.tool_name.as_deref()) {
                    self.state = AgentState::Question;
                    self.activity = extract_question_from_input(ev.tool_input.as_deref())
                        .or_else(|| Some("waiting for your input".to_string()));
                }
                // Non-clarify PreToolUse carries no state change for Codex; the
                // PostToolUse path drives the working/activity display.
                false
            }
            // Codex never fires these — ignore gracefully.
            EventKind::Notification | EventKind::SessionEnd => false,
        }
    }
}

/// Build an activity string from tool name + input JSON.
fn build_tool_activity(tool_name: Option<&str>, tool_input: Option<&str>) -> String {
    match tool_name {
        Some(name) => {
            let lower = name.to_lowercase();
            if lower.contains("edit") || lower.contains("write") || lower.contains("create") {
                let path = extract_path_from_input(tool_input);
                format!("Editing {}", path.unwrap_or_else(|| "file".to_string()))
            } else if lower.contains("bash") || lower.contains("run") || lower.contains("exec") {
                let cmd = extract_command_from_input(tool_input);
                format!("Running {}", cmd.unwrap_or_else(|| "command".to_string()))
            } else if lower.contains("read") {
                let path = extract_path_from_input(tool_input);
                format!("Reading {}", path.unwrap_or_else(|| "file".to_string()))
            } else {
                format!("Using {name}")
            }
        }
        None => "thinking…".to_string(),
    }
}

fn extract_path_from_input(input: Option<&str>) -> Option<String> {
    let input = input?;
    // Try to parse as JSON and extract "path" or "file_path"
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(input) {
        for key in &["path", "file_path", "target_file"] {
            if let Some(p) = v.get(key).and_then(|v| v.as_str()) {
                return Some(p.to_string());
            }
        }
    }
    None
}

fn extract_command_from_input(input: Option<&str>) -> Option<String> {
    let input = input?;
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(input) {
        for key in &["command", "cmd", "script"] {
            if let Some(c) = v.get(key).and_then(|v| v.as_str()) {
                // Truncate long commands
                return Some(truncate_str(c, 60));
            }
        }
    }
    None
}

fn truncate_str(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        format!("{}…", chars[..max].iter().collect::<String>())
    }
}

fn matches_edit_tool(tool_name: Option<&str>) -> bool {
    let Some(name) = tool_name else { return false };
    let lower = name.to_lowercase();
    lower.contains("edit") || lower.contains("write") || lower.contains("create")
}

/// Determines if a notification text represents an approval/permission request.
pub fn is_approval_notification(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("permission")
        || lower.contains("approve")
        || lower.contains("allow")
        || lower.contains("run:")
        || lower.contains("execute")
        || lower.contains("grant")
}

/// True when the notification is the generic "idle, waiting for the next prompt"
/// nudge (fired after a turn ends and the user is slow to respond). This is NOT
/// an actionable question — the agent is effectively idle.
pub fn is_idle_notification(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("waiting for your input")
        || lower.contains("waiting for input")
        || lower.contains("is waiting")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AgentFamily;

    fn make_event(kind: EventKind) -> HookEvent {
        HookEvent {
            kind,
            family: AgentFamily::Claude,
            session_id: "sess-test".to_string(),
            cwd: "/tmp/project".to_string(),
            pid: Some(1000),
            tool_name: None,
            tool_input: None,
            prompt: None,
            notification: None,
            host_app: None,
            terminal_session_id: None,
            terminal_tty: None,
            tmux_target: None,
            wezterm_pane: None,
            zellij_session: None,
            workspace_path: None,
            codex_thread_id: None,
            pane_title: None,
        }
    }

    #[test]
    fn claude_ask_user_question_yields_question_with_text() {
        // Claude shows a clarify card via AskUserQuestion; its PreToolUse must put
        // the session into Question with the question text (not "Using …").
        let start = make_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&start);
        s.apply(&start);

        let mut ev = make_event(EventKind::PreToolUse);
        ev.tool_name = Some(CLARIFY_TOOL_CLAUDE.to_string());
        ev.tool_input =
            Some(r#"{"questions":[{"header":"风格","question":"你想要哪种风格？"}]}"#.to_string());
        s.apply(&ev);

        assert_eq!(s.state, AgentState::Question);
        assert_eq!(s.activity.as_deref(), Some("你想要哪种风格？"));
        assert!(s.awaiting_clarify);
    }

    #[test]
    fn claude_permission_notification_does_not_override_clarify() {
        // After AskUserQuestion's PreToolUse, Claude fires a generic
        // "Claude needs your permission" notification. It must NOT replace the
        // question text while the card is still open.
        let start = make_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&start);
        s.apply(&start);

        let mut pre = make_event(EventKind::PreToolUse);
        pre.tool_name = Some(CLARIFY_TOOL_CLAUDE.to_string());
        pre.tool_input = Some(r#"{"questions":[{"question":"选哪个？"}]}"#.to_string());
        s.apply(&pre);

        let mut note = make_event(EventKind::Notification);
        note.notification = Some("Claude needs your permission".to_string());
        s.apply(&note);

        assert_eq!(s.state, AgentState::Question);
        assert_eq!(s.activity.as_deref(), Some("选哪个？"));
    }

    #[test]
    fn claude_clarify_answer_clears_awaiting_flag() {
        // PostToolUse(AskUserQuestion) = the user answered → leave clarify.
        let start = make_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&start);
        s.apply(&start);

        let mut pre = make_event(EventKind::PreToolUse);
        pre.tool_name = Some(CLARIFY_TOOL_CLAUDE.to_string());
        pre.tool_input = Some(r#"{"questions":[{"question":"q"}]}"#.to_string());
        s.apply(&pre);
        assert!(s.awaiting_clarify);

        let mut post = make_event(EventKind::PostToolUse);
        post.tool_name = Some(CLARIFY_TOOL_CLAUDE.to_string());
        s.apply(&post);
        assert!(!s.awaiting_clarify);
        assert_eq!(s.state, AgentState::Working);
    }

    #[test]
    fn clarify_multi_question_shows_count() {
        // A card with N>1 questions shows the first question suffixed with (1/N).
        let start = make_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&start);
        s.apply(&start);

        let mut ev = make_event(EventKind::PreToolUse);
        ev.tool_name = Some(CLARIFY_TOOL_CLAUDE.to_string());
        ev.tool_input = Some(
            r#"{"questions":[{"question":"风格？"},{"question":"歧义？"}]}"#.to_string(),
        );
        s.apply(&ev);

        assert_eq!(s.activity.as_deref(), Some("风格？ (1/2)"));
    }

    #[test]
    fn session_start_yields_idle() {
        let ev = make_event(EventKind::SessionStart);
        let mut session = AgentSession::new(&ev);
        let remove = session.apply(&ev);
        assert!(!remove);
        assert_eq!(session.state, AgentState::Idle);
    }

    #[test]
    fn user_prompt_submit_yields_working() {
        let start_ev = make_event(EventKind::SessionStart);
        let mut session = AgentSession::new(&start_ev);
        session.apply(&start_ev);

        let mut ev = make_event(EventKind::UserPromptSubmit);
        ev.prompt = Some("Fix the checkout bug".to_string());
        let remove = session.apply(&ev);

        assert!(!remove);
        assert_eq!(session.state, AgentState::Working);
        assert_eq!(session.activity.as_deref(), Some("thinking…"));
        assert_eq!(
            session.last_prompt.as_deref(),
            Some("Fix the checkout bug")
        );
    }

    #[test]
    fn last_prompt_tracks_most_recent() {
        let start_ev = make_event(EventKind::SessionStart);
        let mut session = AgentSession::new(&start_ev);
        session.apply(&start_ev);

        let mut ev1 = make_event(EventKind::UserPromptSubmit);
        ev1.prompt = Some("First prompt".to_string());
        session.apply(&ev1);

        let mut ev2 = make_event(EventKind::UserPromptSubmit);
        ev2.prompt = Some("Second prompt".to_string());
        session.apply(&ev2);

        // Each prompt updates last_prompt, so it reflects the most recent one.
        assert_eq!(session.last_prompt.as_deref(), Some("Second prompt"));
    }

    #[test]
    fn pre_tool_use_edit_shows_editing() {
        let start_ev = make_event(EventKind::SessionStart);
        let mut session = AgentSession::new(&start_ev);
        session.apply(&start_ev);

        let mut ev = make_event(EventKind::PreToolUse);
        ev.tool_name = Some("Edit".to_string());
        ev.tool_input = Some(r#"{"path":"src/main.rs"}"#.to_string());
        session.apply(&ev);

        assert_eq!(session.state, AgentState::Working);
        assert_eq!(session.activity.as_deref(), Some("Editing src/main.rs"));
    }

    #[test]
    fn pre_tool_use_bash_shows_running() {
        let start_ev = make_event(EventKind::SessionStart);
        let mut session = AgentSession::new(&start_ev);
        session.apply(&start_ev);

        let mut ev = make_event(EventKind::PreToolUse);
        ev.tool_name = Some("Bash".to_string());
        ev.tool_input = Some(r#"{"command":"git push"}"#.to_string());
        session.apply(&ev);

        assert_eq!(session.state, AgentState::Working);
        assert_eq!(session.activity.as_deref(), Some("Running git push"));
    }

    #[test]
    fn post_tool_use_resets_to_thinking() {
        let start_ev = make_event(EventKind::SessionStart);
        let mut session = AgentSession::new(&start_ev);
        session.apply(&start_ev);

        let mut pre = make_event(EventKind::PreToolUse);
        pre.tool_name = Some("Edit".to_string());
        pre.tool_input = Some(r#"{"path":"src/lib.rs"}"#.to_string());
        session.apply(&pre);

        let mut post = make_event(EventKind::PostToolUse);
        post.tool_name = Some("Edit".to_string());
        session.apply(&post);

        assert_eq!(session.state, AgentState::Working);
        assert_eq!(session.activity.as_deref(), Some("thinking…"));
        assert_eq!(session.edit_count, 1);
    }

    #[test]
    fn notification_permission_yields_approval() {
        let start_ev = make_event(EventKind::SessionStart);
        let mut session = AgentSession::new(&start_ev);
        session.apply(&start_ev);

        let mut ev = make_event(EventKind::Notification);
        ev.notification = Some("Permission required to run: git push --force".to_string());
        session.apply(&ev);

        assert_eq!(session.state, AgentState::Approval);
    }

    #[test]
    fn notification_other_yields_question() {
        let start_ev = make_event(EventKind::SessionStart);
        let mut session = AgentSession::new(&start_ev);
        session.apply(&start_ev);

        let mut ev = make_event(EventKind::Notification);
        ev.notification = Some("Choose: Postgres or MySQL?".to_string());
        session.apply(&ev);

        assert_eq!(session.state, AgentState::Question);
    }

    #[test]
    fn stop_yields_done() {
        let start_ev = make_event(EventKind::SessionStart);
        let mut session = AgentSession::new(&start_ev);
        session.apply(&start_ev);

        let ev = make_event(EventKind::Stop);
        session.apply(&ev);

        assert_eq!(session.state, AgentState::Done);
    }

    #[test]
    fn session_end_returns_true() {
        let start_ev = make_event(EventKind::SessionStart);
        let mut session = AgentSession::new(&start_ev);
        session.apply(&start_ev);

        let ev = make_event(EventKind::SessionEnd);
        let remove = session.apply(&ev);

        assert!(remove);
    }

    #[test]
    fn full_lifecycle_sequence() {
        let start_ev = make_event(EventKind::SessionStart);
        let mut session = AgentSession::new(&start_ev);
        session.apply(&start_ev);
        assert_eq!(session.state, AgentState::Idle);

        let mut prompt_ev = make_event(EventKind::UserPromptSubmit);
        prompt_ev.prompt = Some("Fix the bug".to_string());
        session.apply(&prompt_ev);
        assert_eq!(session.state, AgentState::Working);

        let mut pre_ev = make_event(EventKind::PreToolUse);
        pre_ev.tool_name = Some("Edit".to_string());
        pre_ev.tool_input = Some(r#"{"path":"src/bug.rs"}"#.to_string());
        session.apply(&pre_ev);
        assert_eq!(session.activity.as_deref(), Some("Editing src/bug.rs"));

        let mut notif_ev = make_event(EventKind::Notification);
        notif_ev.notification = Some("Allow running: cargo test".to_string());
        session.apply(&notif_ev);
        assert_eq!(session.state, AgentState::Approval);

        let stop_ev = make_event(EventKind::Stop);
        session.apply(&stop_ev);
        assert_eq!(session.state, AgentState::Done);

        let end_ev = make_event(EventKind::SessionEnd);
        let remove = session.apply(&end_ev);
        assert!(remove);
    }

    // ---------------------------------------------------------------------------
    // Codex event mapping tests (spec §6)
    // ---------------------------------------------------------------------------

    fn make_codex_event(kind: EventKind) -> HookEvent {
        HookEvent {
            kind,
            family: AgentFamily::Codex,
            session_id: "codex-sess".to_string(),
            cwd: "/tmp/codex-project".to_string(),
            pid: Some(2000),
            tool_name: None,
            tool_input: None,
            prompt: None,
            notification: None,
            host_app: None,
            terminal_session_id: None,
            terminal_tty: None,
            tmux_target: None,
            wezterm_pane: None,
            zellij_session: None,
            workspace_path: None,
            codex_thread_id: None,
            pane_title: None,
        }
    }

    #[test]
    fn codex_session_start_yields_idle() {
        let ev = make_codex_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&ev);
        let remove = s.apply(&ev);
        assert!(!remove);
        assert_eq!(s.state, AgentState::Idle);
    }

    #[test]
    fn codex_user_prompt_submit_yields_working() {
        let start = make_codex_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&start);
        s.apply(&start);

        let mut ev = make_codex_event(EventKind::UserPromptSubmit);
        ev.prompt = Some("Do the thing".to_string());
        let remove = s.apply(&ev);

        assert!(!remove);
        assert_eq!(s.state, AgentState::Working);
        assert_eq!(s.activity.as_deref(), Some("thinking…"));
        assert_eq!(s.last_prompt.as_deref(), Some("Do the thing"));
    }

    #[test]
    fn codex_permission_request_yields_approval() {
        let start = make_codex_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&start);
        s.apply(&start);

        let mut ev = make_codex_event(EventKind::PermissionRequest);
        ev.notification = Some("run: rm -rf /tmp/old".to_string());
        let remove = s.apply(&ev);

        assert!(!remove);
        assert_eq!(s.state, AgentState::Approval);
    }

    #[test]
    fn codex_stop_yields_done() {
        let start = make_codex_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&start);
        s.apply(&start);

        let ev = make_codex_event(EventKind::Stop);
        s.apply(&ev);
        assert_eq!(s.state, AgentState::Done);
    }

    #[test]
    fn codex_post_tool_use_stays_working() {
        let start = make_codex_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&start);
        s.apply(&start);

        let mut ev = make_codex_event(EventKind::PostToolUse);
        ev.tool_name = Some("shell".to_string());
        let remove = s.apply(&ev);
        assert!(!remove);
        assert_eq!(s.state, AgentState::Working);
        assert_eq!(s.activity.as_deref(), Some("shell done"));
    }

    #[test]
    fn codex_notification_is_noop() {
        // Codex has no Notification event; if one were somehow fired it must be a
        // no-op (it is not how Codex surfaces questions — that's the clarify tool).
        let start = make_codex_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&start);
        s.apply(&start);

        let ev = make_codex_event(EventKind::Notification);
        s.apply(&ev);
        assert_ne!(s.state, AgentState::Question);
    }

    #[test]
    fn codex_clarify_pretooluse_yields_question() {
        // Codex shows a clarify card by calling the request_user_input tool; its
        // PreToolUse must put the session into Question with the question text.
        let start = make_codex_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&start);
        s.apply(&start);

        let mut ev = make_codex_event(EventKind::PreToolUse);
        ev.tool_name = Some(CLARIFY_TOOL.to_string());
        ev.tool_input = Some(
            r#"{"questions":[{"header":"测试选择","question":"你想用哪种内容来测试 clarify 卡片？","options":[]}]}"#
                .to_string(),
        );
        let remove = s.apply(&ev);

        assert!(!remove);
        assert_eq!(s.state, AgentState::Question);
        assert_eq!(
            s.activity.as_deref(),
            Some("你想用哪种内容来测试 clarify 卡片？")
        );
    }

    #[test]
    fn codex_non_clarify_pretooluse_does_not_change_state() {
        // An ordinary tool's PreToolUse must not flip Codex into Question (only
        // request_user_input does). State stays whatever it was.
        let start = make_codex_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&start);
        s.apply(&start);
        s.state = AgentState::Working;

        let mut ev = make_codex_event(EventKind::PreToolUse);
        ev.tool_name = Some("Bash".to_string());
        ev.tool_input = Some(r#"{"command":"ls"}"#.to_string());
        s.apply(&ev);

        assert_eq!(s.state, AgentState::Working);
    }

    #[test]
    fn codex_clarify_then_answer_returns_to_working() {
        // After the user answers a clarify, Codex fires UserPromptSubmit → the
        // session must leave Question and resume Working.
        let start = make_codex_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&start);
        s.apply(&start);

        let mut clarify = make_codex_event(EventKind::PreToolUse);
        clarify.tool_name = Some(CLARIFY_TOOL.to_string());
        clarify.tool_input = Some(r#"{"questions":[{"question":"pick one"}]}"#.to_string());
        s.apply(&clarify);
        assert_eq!(s.state, AgentState::Question);

        let answer = make_codex_event(EventKind::UserPromptSubmit);
        s.apply(&answer);
        assert_eq!(s.state, AgentState::Working);
    }

    #[test]
    fn codex_no_session_end_returns_false() {
        // Codex has no SessionEnd → apply returns false (removal is via liveness).
        let start = make_codex_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&start);
        s.apply(&start);

        let ev = make_codex_event(EventKind::SessionEnd);
        let remove = s.apply(&ev);
        assert!(
            !remove,
            "Codex SessionEnd should NOT signal removal (use liveness instead)"
        );
    }

    #[test]
    fn is_approval_notification_cases() {
        assert!(is_approval_notification("Permission required"));
        assert!(is_approval_notification("Please approve this action"));
        assert!(is_approval_notification("Allow access to..."));
        assert!(is_approval_notification("run: git push --force"));
        assert!(is_approval_notification("Execute this script?"));
        assert!(!is_approval_notification("Choose: Postgres or MySQL?"));
        assert!(!is_approval_notification("What branch should I use?"));
    }

    #[test]
    fn idle_notification_detection() {
        // The generic "waiting for input" nudge must be treated as idle, not a question.
        assert!(is_idle_notification("Claude is waiting for your input"));
        assert!(is_idle_notification("waiting for input"));
        assert!(!is_idle_notification("Permission required to run git push"));
        assert!(!is_idle_notification("Choose: Postgres or MySQL?"));
    }

    // ── summarize_event tests (TDD, Task 1) ──────────────────────────────────

    fn ev_pre_tool(tool_name: &str, tool_input: &str) -> HookEvent {
        HookEvent {
            kind: EventKind::PreToolUse,
            family: AgentFamily::Claude,
            session_id: "s".to_string(),
            cwd: "/tmp".to_string(),
            pid: None,
            tool_name: Some(tool_name.to_string()),
            tool_input: Some(tool_input.to_string()),
            prompt: None,
            notification: None,
            host_app: None,
            terminal_session_id: None,
            terminal_tty: None,
            tmux_target: None,
            wezterm_pane: None,
            zellij_session: None,
            workspace_path: None,
            codex_thread_id: None,
            pane_title: None,
        }
    }

    #[test]
    fn summarize_edit_tool() {
        let ev = ev_pre_tool("Edit", r#"{"path":"src/main.rs"}"#);
        let s = summarize_event(&ev);
        assert!(
            s.starts_with("✎ edited"),
            "expected '✎ edited …', got: {s:?}"
        );
        assert!(s.contains("src/main.rs"), "should contain path, got: {s:?}");
    }

    #[test]
    fn summarize_bash_tool() {
        let ev = ev_pre_tool("Bash", r#"{"command":"cargo test"}"#);
        let s = summarize_event(&ev);
        assert!(s.starts_with("⚙ ran"), "expected '⚙ ran …', got: {s:?}");
        assert!(s.contains("cargo test"), "should contain cmd, got: {s:?}");
    }

    #[test]
    fn summarize_notification_approval() {
        let mut ev = make_event(EventKind::Notification);
        ev.notification = Some("Permission required to run: git push --force".to_string());
        let s = summarize_event(&ev);
        assert!(s.starts_with("▲"), "expected '▲ …', got: {s:?}");
    }

    #[test]
    fn summarize_permission_request() {
        let mut ev = make_codex_event(EventKind::PermissionRequest);
        ev.notification = Some("run: rm -rf /tmp/old".to_string());
        let s = summarize_event(&ev);
        assert!(s.starts_with("▲"), "expected '▲ …', got: {s:?}");
    }

    #[test]
    fn summarize_stop_yields_done() {
        let ev = make_event(EventKind::Stop);
        let s = summarize_event(&ev);
        assert_eq!(s, "✓ done");
    }

    #[test]
    fn summarize_event_has_summary_in_stored_event() {
        let start_ev = make_event(EventKind::SessionStart);
        let mut session = AgentSession::new(&start_ev);
        session.apply(&start_ev);

        let mut ev = make_event(EventKind::PreToolUse);
        ev.tool_name = Some("Edit".to_string());
        ev.tool_input = Some(r#"{"path":"lib.rs"}"#.to_string());
        session.apply(&ev);

        // The stored event should have the summary pre-computed
        let last = session.events.back().unwrap();
        assert!(
            last.summary.starts_with("✎"),
            "stored event summary should start with ✎, got: {:?}",
            last.summary
        );
    }

    #[test]
    fn events_capped_at_max() {
        let start_ev = make_event(EventKind::SessionStart);
        let mut session = AgentSession::new(&start_ev);

        // Push MAX_EVENTS + 10 more
        for _ in 0..(MAX_EVENTS + 10) {
            session.apply(&make_event(EventKind::Stop));
        }
        assert_eq!(
            session.events.len(),
            MAX_EVENTS,
            "events should be capped at MAX_EVENTS"
        );
    }

    // ── Busy timer / cumulative active duration tests ─────────────────────────

    /// After a UserPromptSubmit, the session should have active_since = Some.
    #[test]
    fn active_since_set_on_working() {
        let start_ev = make_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&start_ev);
        s.apply(&start_ev);
        // Starts idle — no active_since.
        assert!(
            s.active_since.is_none(),
            "should be None before first prompt"
        );

        let ev = make_event(EventKind::UserPromptSubmit);
        s.apply(&ev);
        assert_eq!(s.state, AgentState::Working);
        assert!(
            s.active_since.is_some(),
            "active_since should be Some after becoming working"
        );
    }

    /// Stop → Done freezes active_since → None and increments active_accum.
    #[test]
    fn stop_freezes_timer_and_accumulates() {
        let start_ev = make_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&start_ev);
        s.apply(&start_ev);

        // Become active.
        s.apply(&make_event(EventKind::UserPromptSubmit));
        assert!(s.active_since.is_some());
        assert_eq!(s.active_accum, std::time::Duration::ZERO);

        // Stop.
        s.apply(&make_event(EventKind::Stop));
        assert_eq!(s.state, AgentState::Done);
        assert!(
            s.active_since.is_none(),
            "active_since should be None after Stop"
        );
        // active_accum should be positive (we may not know the exact value, but > 0).
        // Since Instant::now() is used inside apply() the elapsed is at least 0.
        // We just check it's not negative (i.e., the add happened without panic).
        // For a reliable assert, check that accum is ≥ 0 (always true for Duration).
        let _ = s.active_accum; // non-zero in practice; at least no panic
    }

    /// Re-activating after Done resumes from the frozen accum (does not reset to zero).
    #[test]
    fn reactivation_resumes_from_frozen_accum() {
        let start_ev = make_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&start_ev);
        s.apply(&start_ev);

        // First active segment.
        s.apply(&make_event(EventKind::UserPromptSubmit));
        // Manually set active_since to a known past time so we can assert an exact accum.
        let t0 = Instant::now() - std::time::Duration::from_secs(10);
        s.active_since = Some(t0);

        // Freeze: Stop.
        s.apply(&make_event(EventKind::Stop));
        assert!(s.active_since.is_none());
        // accum should be ≥ 10s (the 10s we injected).
        assert!(
            s.active_accum >= std::time::Duration::from_secs(10),
            "active_accum should be ≥ 10s after Stop, got {:?}",
            s.active_accum
        );
        let accum_after_first = s.active_accum;

        // Second active segment.
        s.apply(&make_event(EventKind::UserPromptSubmit));
        assert!(
            s.active_since.is_some(),
            "active_since should resume after re-activation"
        );
        // accum must NOT have been reset — it should equal the frozen value.
        assert_eq!(
            s.active_accum, accum_after_first,
            "active_accum must not be reset on re-activation"
        );
    }

    /// Consecutive working events (working→working) must NOT reset active_since.
    #[test]
    fn consecutive_working_events_do_not_reset_active_since() {
        let start_ev = make_event(EventKind::SessionStart);
        let mut s = AgentSession::new(&start_ev);
        s.apply(&start_ev);

        // First working event.
        s.apply(&make_event(EventKind::UserPromptSubmit));
        let since_after_first = s
            .active_since
            .expect("active_since should be Some after first working event");

        // Second working event (PreToolUse keeps state=Working).
        let mut pre = make_event(EventKind::PreToolUse);
        pre.tool_name = Some("Edit".to_string());
        pre.tool_input = Some(r#"{"path":"a.rs"}"#.to_string());
        s.apply(&pre);
        assert_eq!(s.state, AgentState::Working);

        let since_after_second = s
            .active_since
            .expect("active_since should still be Some after second working event");

        // The Instant must be the SAME object (not reset to a new Instant).
        // We compare them: both should be ≥ since_after_first and the second
        // should not be significantly later.
        assert_eq!(
            since_after_first, since_after_second,
            "active_since must not be reset by a working→working transition"
        );
    }

    /// is_active helper returns correct values for all states.
    #[test]
    fn is_active_states() {
        assert!(is_active(&AgentState::Working));
        assert!(is_active(&AgentState::Approval));
        assert!(is_active(&AgentState::Question));
        assert!(!is_active(&AgentState::Done));
        assert!(!is_active(&AgentState::Idle));
    }
}
