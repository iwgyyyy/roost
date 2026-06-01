use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};

use crate::liveness;
use crate::naming::resolve_name;
use crate::protocol::{Event, Request, SessionDetail, SessionView};
use crate::session::AgentSession;
use crate::sock;

/// Per-session liveness metadata maintained by the daemon (separate from AgentSession
/// to avoid cluttering the session model).
///
/// Note: `SessionEnd` (Claude) is handled synchronously in the event path —
/// the session is removed immediately when the event arrives, before the
/// liveness thread ever sees it.  Therefore there is no `ended` flag here:
/// the liveness thread only handles the "process died silently" and "timeout"
/// cases described in spec §9.
#[derive(Debug, Default)]
pub struct LivenessMeta {
    /// Number of consecutive `kill -0` failures.
    pub consecutive_dead: u32,
}

pub struct DaemonState {
    pub sessions: HashMap<String, AgentSession>,
    pub liveness: HashMap<String, LivenessMeta>,
}

impl Default for DaemonState {
    fn default() -> Self {
        Self::new()
    }
}

impl DaemonState {
    pub fn new() -> Self {
        DaemonState {
            sessions: HashMap::new(),
            liveness: HashMap::new(),
        }
    }

    /// Build a `SessionDetail` for the given session id, or `None` if not found.
    pub fn session_detail(&self, session_id: &str, now: Instant) -> Option<SessionDetail> {
        let s = self.sessions.get(session_id)?;
        // `since` mirrors the list's busy timer: cumulative active time that
        // freezes once the session is Done/Idle (rather than counting up forever
        // as "time since last activity" did).
        let elapsed = s.busy_duration(now).as_secs();

        // Build recent events in reverse-chronological order (newest first).
        // duration_secs for each event = time until the next (older) event,
        // i.e. events[i].ts - events[i+1].ts in the original (chron) order.
        // In the reversed slice: events[i].ts > events[i+1].ts (older = larger ts_secs),
        // so duration_secs[i] = ts_secs[i+1] - ts_secs[i].  The last element in
        // the reversed list (oldest event) gets `None` as its duration is unknown.
        // The *first* element in the reversed list (most recent / current) also gets
        // `None` because it is still in progress.
        let stored: Vec<_> = s.events.iter().rev().collect();
        let ts_vals: Vec<u64> = stored
            .iter()
            .map(|se| now.duration_since(se.ts).as_secs())
            .collect();
        let recent_events: Vec<Event> = stored
            .iter()
            .enumerate()
            .map(|(i, se)| {
                let ts_secs = ts_vals[i];
                // Each completed step's timer FREEZES at how long that step ran =
                // the gap from this event until the NEXT (newer) event arrived.
                // Computed from the raw Instants of the two events (newer minus
                // older) so the value is independent of `now` and never jitters
                // by ±1s as the clock advances (two separately-floored ages would
                // flicker between adjacent integers). Index 0 is the newest event
                // and has no successor yet, so its duration is unknown → None.
                let duration_secs = if i == 0 {
                    None
                } else {
                    Some(stored[i - 1].ts.duration_since(se.ts).as_secs())
                };
                Event {
                    ts_secs,
                    summary: se.summary.clone(),
                    duration_secs,
                }
            })
            .collect();

        let agent_display = format!("{} {}", s.family.icon(), s.family.display_label());

        Some(SessionDetail {
            id: s.id.clone(),
            family: s.family.clone(),
            name: s.name.clone(),
            agent_display,
            cwd: s.cwd.clone(),
            state: s.state.as_str().to_string(),
            action: s.activity.clone(),
            waiting_reason: if matches!(
                s.state,
                crate::session::AgentState::Approval | crate::session::AgentState::Question
            ) {
                s.activity.clone()
            } else {
                None
            },
            since_secs: elapsed,
            recent_events,
        })
    }

    pub fn session_views(&self, now: Instant) -> Vec<SessionView> {
        let mut views: Vec<SessionView> = self
            .sessions
            .values()
            .map(|s| {
                let elapsed = now.duration_since(s.last_activity).as_secs();
                let busy = s.busy_duration(now);
                SessionView {
                    id: s.id.clone(),
                    family: s.family.clone(),
                    name: s.name.clone(),
                    state: s.state.as_str().to_string(),
                    activity: s.activity.clone(),
                    last_activity_secs: elapsed,
                    busy_secs: busy.as_secs(),
                    last_prompt: s.last_prompt.clone(),
                    host_app: s.host_app.clone(),
                    host_bundle_id: s.host_bundle_id.clone(),
                    terminal_session_id: s.terminal_session_id.clone(),
                    terminal_tty: s.terminal_tty.clone(),
                    tmux_target: s.tmux_target.clone(),
                    wezterm_pane: s.wezterm_pane.clone(),
                    zellij_session: s.zellij_session.clone(),
                    workspace_path: s.workspace_path.clone(),
                    codex_thread_id: s.codex_thread_id.clone(),
                    pane_title: s.pane_title.clone(),
                }
            })
            .collect();

        // Primary: attention priority (approval > question > working > done > idle).
        // Secondary: name then id — for STABLE order. HashMap iteration order is
        // nondeterministic, so without a tiebreaker, rows in the same state group
        // would jump around between refreshes.
        let pri = |s: &str| match s {
            "approval" => 0,
            "question" => 1,
            "working" => 2,
            "done" => 3,
            _ => 4,
        };
        views.sort_by(|a, b| {
            pri(&a.state)
                .cmp(&pri(&b.state))
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.id.cmp(&b.id))
        });

        views
    }
}

pub fn run_daemon() -> Result<()> {
    let path = sock::socket_path();
    run_daemon_at(path)
}

pub fn run_daemon_at(path: PathBuf) -> Result<()> {
    // Create parent directories
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // If a live daemon is already listening here, do NOT start a second one —
    // otherwise we'd clobber its socket. Only a *stale* socket file is removed.
    if crate::sock::is_listening(&path) {
        anyhow::bail!("a roost daemon is already running on {}", path.display());
    }
    if path.exists() {
        std::fs::remove_file(&path).context("failed to remove stale socket")?;
    }

    let listener = UnixListener::bind(&path).context("failed to bind Unix socket")?;

    let state: Arc<Mutex<DaemonState>> = Arc::new(Mutex::new(DaemonState::new()));

    // Spawn the liveness (keep-alive) thread.
    {
        let state_clone = Arc::clone(&state);
        std::thread::spawn(move || {
            run_liveness_thread(state_clone);
        });
    }

    eprintln!("[roost daemon] listening on {}", path.display());

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state_clone = Arc::clone(&state);
                std::thread::spawn(move || {
                    if let Err(e) = handle_connection(stream, state_clone) {
                        eprintln!("[roost daemon] connection error: {e}");
                    }
                });
            }
            Err(e) => {
                eprintln!("[roost daemon] accept error: {e}");
            }
        }
    }

    Ok(())
}

fn handle_connection(stream: UnixStream, state: Arc<Mutex<DaemonState>>) -> Result<()> {
    let mut write_stream = stream.try_clone()?;
    let reader = BufReader::new(stream);

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let request: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[roost daemon] parse error: {e} (line: {line})");
                continue;
            }
        };

        match request {
            Request::Report { event } => {
                let session_id = event.session_id.clone();
                let cwd = event.cwd.clone();

                // Check outside the lock whether this is a new session.
                // We must NOT hold the lock while calling resolve_name (forks git).
                let is_new = {
                    let st = state.lock().unwrap_or_else(|p| p.into_inner());
                    !st.sessions.contains_key(&session_id)
                };

                // Resolve name outside the lock to avoid blocking the daemon
                // while git runs as a subprocess.
                let resolved_name = if is_new {
                    Some(resolve_name(&cwd))
                } else {
                    None
                };

                let mut st = state.lock().unwrap_or_else(|p| p.into_inner());

                let session_ended = if let Some(session) = st.sessions.get_mut(&session_id) {
                    session.apply(&event)
                } else {
                    // New session: create then apply
                    let mut session = AgentSession::new(&event);
                    session.name = resolved_name.unwrap_or_else(|| resolve_name(&cwd));
                    let ended = session.apply(&event);
                    st.sessions.insert(session_id.clone(), session);
                    // Initialise liveness entry.
                    st.liveness.entry(session_id.clone()).or_default();
                    ended
                };

                if session_ended {
                    st.sessions.remove(&session_id);
                    st.liveness.remove(&session_id);
                } else {
                    // Reset consecutive dead count on any fresh event.
                    if let Some(meta) = st.liveness.get_mut(&session_id) {
                        meta.consecutive_dead = 0;
                    }
                }
            }
            Request::List => {
                let views = {
                    let st = state.lock().unwrap_or_else(|p| p.into_inner());
                    st.session_views(Instant::now())
                };
                let json = serde_json::to_string(&views).unwrap_or_else(|_| "[]".to_string());
                writeln!(write_stream, "{json}")?;
            }
            Request::Detail { session_id } => {
                let detail = {
                    let st = state.lock().unwrap_or_else(|p| p.into_inner());
                    st.session_detail(&session_id, Instant::now())
                };
                let json = match detail {
                    Some(d) => serde_json::to_string(&d).unwrap_or_else(|_| "null".to_string()),
                    None => "null".to_string(),
                };
                writeln!(write_stream, "{json}")?;
            }
        }
    }

    Ok(())
}

/// The interval between liveness probes.
const LIVENESS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Check if `pid` is alive using `kill(pid, 0)`.
/// Returns true if the process exists (even if we don't have permission to signal it).
pub fn pid_is_alive(pid: u32) -> bool {
    // kill(pid, 0) checks existence without sending a signal.
    // - Returns 0 → process exists and we can signal it.
    // - Returns EPERM → process exists but we lack permission.
    // - Returns ESRCH → process does not exist.
    let ret = rustix::process::test_kill_process(
        // SAFETY: non-zero pid guaranteed by from_raw check
        rustix::process::Pid::from_raw(pid as i32).unwrap_or_else(rustix::process::getpid),
    );
    match ret {
        Ok(()) => true,
        Err(e) => e == rustix::io::Errno::PERM,
    }
}

/// The liveness thread: runs every ~2s, probes each session's pid, calls
/// `liveness::should_remove` and removes dead sessions.
fn run_liveness_thread(state: Arc<Mutex<DaemonState>>) {
    loop {
        std::thread::sleep(LIVENESS_INTERVAL);

        let now = Instant::now();

        // Collect pid probes outside the lock to minimise lock hold time.
        // SessionEnd is handled synchronously in the event path (session removed
        // immediately), so the liveness thread never needs an `ended` flag.
        let probes: Vec<(String, Option<u32>, Instant, u32)> = {
            let st = state.lock().unwrap_or_else(|p| p.into_inner());
            st.sessions
                .iter()
                .map(|(id, sess)| {
                    let consecutive_dead =
                        st.liveness.get(id).map(|m| m.consecutive_dead).unwrap_or(0);
                    (id.clone(), sess.pid, sess.last_activity, consecutive_dead)
                })
                .collect()
        };

        // Probe each pid, then update state under lock.
        for (id, pid_opt, last_activity, consecutive_dead) in probes {
            let new_consecutive = match pid_opt {
                Some(pid) if !pid_is_alive(pid) => consecutive_dead + 1,
                _ => 0,
            };

            // ended_flag=false: SessionEnd sessions are already removed before
            // the liveness thread can observe them.
            let remove = liveness::should_remove(now, last_activity, new_consecutive, false);

            let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
            if remove {
                st.sessions.remove(&id);
                st.liveness.remove(&id);
            } else if let Some(meta) = st.liveness.get_mut(&id) {
                meta.consecutive_dead = new_consecutive;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::AgentState;

    #[test]
    fn daemon_state_views_sorted() {
        let mut state = DaemonState::new();

        // Add sessions manually for testing
        use crate::protocol::{AgentFamily, EventKind, HookEvent};

        let start_ev = HookEvent {
            kind: EventKind::SessionStart,
            family: AgentFamily::Claude,
            session_id: "s1".to_string(),
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

        let mut s1 = AgentSession::new(&start_ev);
        s1.name = "working-session".to_string();
        s1.state = AgentState::Working;
        state.sessions.insert("s1".to_string(), s1);

        let mut s2 = AgentSession::new(&start_ev);
        s2.id = "s2".to_string();
        s2.name = "approval-session".to_string();
        s2.state = AgentState::Approval;
        state.sessions.insert("s2".to_string(), s2);

        let views = state.session_views(Instant::now());
        assert_eq!(views.len(), 2);
        // approval should come first
        assert_eq!(views[0].state, "approval");
        assert_eq!(views[1].state, "working");
    }

    /// Regression: a Done session's peek `since` must FREEZE — once the agent
    /// stops, it had no open active segment, so the value must not keep counting
    /// up as `now` advances.
    #[test]
    fn detail_since_is_frozen_when_done() {
        use crate::protocol::{AgentFamily, EventKind, HookEvent};
        use std::time::Duration;

        let start_ev = HookEvent {
            kind: EventKind::SessionStart,
            family: AgentFamily::Claude,
            session_id: "s1".to_string(),
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

        let mut state = DaemonState::new();
        let mut s = AgentSession::new(&start_ev);
        // Done: a fixed amount of accumulated busy time, no open active segment.
        s.state = AgentState::Done;
        s.active_accum = Duration::from_secs(42);
        s.active_since = None;
        state.sessions.insert("s1".to_string(), s);

        let base = Instant::now();
        let a = state.session_detail("s1", base).unwrap().since_secs;
        let b = state
            .session_detail("s1", base + Duration::from_secs(10))
            .unwrap()
            .since_secs;
        assert_eq!(a, 42, "since should reflect frozen busy time");
        assert_eq!(a, b, "since must not advance after the session is Done");
    }

    /// Regression: completed-step durations in the peek "recent" list must be
    /// FROZEN — identical no matter what `now` we render at. The previous
    /// implementation subtracted two separately-floored ages, which flickered by
    /// ±1s as the clock advanced (the "乱跳" the user observed).
    #[test]
    fn recent_event_durations_are_stable_across_now() {
        use crate::protocol::{AgentFamily, EventKind, HookEvent};
        use crate::session::StoredEvent;
        use std::time::Duration;

        let start_ev = HookEvent {
            kind: EventKind::SessionStart,
            family: AgentFamily::Claude,
            session_id: "s1".to_string(),
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

        let mut state = DaemonState::new();
        let mut s = AgentSession::new(&start_ev);

        // Craft three events with fractional-second gaps that would make two
        // independently-floored ages flip between adjacent integers.
        let base = Instant::now();
        s.last_activity = base - Duration::from_millis(700);
        s.events.clear();
        // chron order (oldest → newest): front = oldest, back = newest.
        for ms_ago in [11_400u64, 4_700, 700] {
            s.events.push_back(StoredEvent {
                kind: EventKind::PostToolUse,
                ts: base - Duration::from_millis(ms_ago),
                summary: format!("step {ms_ago}"),
            });
        }
        state.sessions.insert("s1".to_string(), s);

        // Render at several `now` offsets; completed durations must not change.
        let durations_at = |delta_ms: u64| -> Vec<Option<u64>> {
            let now = base + Duration::from_millis(delta_ms);
            state
                .session_detail("s1", now)
                .unwrap()
                .recent_events
                .iter()
                .map(|e| e.duration_secs)
                .collect()
        };

        let d0 = durations_at(0);
        for delta in [200u64, 500, 600, 900, 1_300, 1_800] {
            assert_eq!(
                durations_at(delta),
                d0,
                "completed-step durations must be frozen, but changed at +{delta}ms"
            );
        }
        // Newest event (index 0) has no successor → unknown duration.
        assert_eq!(d0[0], None, "newest event should have no frozen duration");
        // Older events should carry a concrete frozen duration.
        assert!(
            d0[1].is_some() && d0[2].is_some(),
            "older events should have frozen durations, got {d0:?}"
        );
    }
}
