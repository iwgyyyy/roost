use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};

use crate::config;
use crate::history::{ClosedTurn, History};
use crate::liveness;
use crate::naming::resolve_name;
use crate::notify;
use crate::protocol::{AgentFamily, Event, Request, SessionDetail, SessionView};
#[cfg(not(test))]
use crate::remotes;
#[cfg(not(test))]
use crate::remote_supervisor::{StdSpawner, Supervisor};
use crate::session::{AgentSession, AgentState};
use crate::sock;

// ── Supervisor type alias (cfg-gated) ─────────────────────────────────────────

/// In production builds, the supervisor is a live `Arc<Supervisor<StdSpawner>>`.
/// In test builds, the supervisor slot is `Option<()>` (always `None`) so all
/// supervisor arms degrade to log/no-op without any real spawn.
#[cfg(not(test))]
type SharedSupervisor = Option<Arc<Supervisor<StdSpawner>>>;

#[cfg(test)]
type SharedSupervisor = Option<()>;

/// Shared, optional history writer. `None` if the DB could not be opened — in
/// that case statistics are silently disabled and the daemon runs normally
/// (fail-open: history must never block agents or take down the daemon).
type SharedHistory = Option<Arc<Mutex<History>>>;

/// Persist closed turns under the history lock, outside the sessions lock.
/// Errors are swallowed — a failed stats write must never disrupt the daemon.
fn record_turns(history: &SharedHistory, turns: &[ClosedTurn]) {
    if turns.is_empty() {
        return;
    }
    if let Some(h) = history {
        let hist = h.lock().unwrap_or_else(|p| p.into_inner());
        for t in turns {
            let _ = hist.record_turn(t);
        }
    }
}

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
    /// Whether an offline (tunnel Down) notification has already been fired for
    /// this session. Prevents repeated alerts while the tunnel stays down.
    pub offline_notified: bool,
}

/// Composite key used to namespace sessions by (origin, session_id).
/// This ensures that remote agents sharing the same session_id string as a
/// local session are tracked as distinct entries.
pub type SessionKey = (String, String);

pub struct DaemonState {
    pub sessions: HashMap<SessionKey, AgentSession>,
    pub liveness: HashMap<SessionKey, LivenessMeta>,
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

    /// Build a `SessionDetail` for the given (origin, session_id) pair, or `None`
    /// if not found.  Pass `"local"` as `origin` for sessions on the local machine.
    pub fn session_detail(&self, origin: &str, session_id: &str, now: Instant) -> Option<SessionDetail> {
        let key = (origin.to_string(), session_id.to_string());
        let s = self.sessions.get(&key)?;
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
            origin: s.origin.clone(),
            host_app: s.host_app.clone(),
            stale: s.stale,
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
                    cwd: s.cwd.clone(),
                    state: s.state.as_str().to_string(),
                    activity: s.activity.clone(),
                    last_activity_secs: elapsed,
                    busy_secs: busy.as_secs(),
                    last_prompt: s.last_prompt.clone(),
                    last_prompt_age_secs: s.last_prompt_at.map(|t| now.duration_since(t).as_secs()),
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
                    origin: s.origin.clone(),
                    stale: s.stale,
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
            // Attention priority, then most-recent user prompt first (None last),
            // then id as a stable final tiebreaker. Recency is keyed on the user's
            // prompt time — not last_activity — so a busy agent firing tool hooks
            // doesn't churn the order.
            pri(&a.state)
                .cmp(&pri(&b.state))
                .then_with(|| {
                    a.last_prompt_age_secs
                        .unwrap_or(u64::MAX)
                        .cmp(&b.last_prompt_age_secs.unwrap_or(u64::MAX))
                })
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

    // Open the history DB (fail-open: disable stats but keep running on error).
    let history: SharedHistory = match History::open() {
        Ok(h) => Some(Arc::new(Mutex::new(h))),
        Err(e) => {
            eprintln!("[roost daemon] history disabled: {e:#}");
            None
        }
    };

    // Initialise the SSH tunnel supervisor and start a watcher for each
    // already-registered remote.  An empty registry is a no-op.
    // Only compiled in production builds — tests use MockSpawner directly.
    #[cfg(not(test))]
    let supervisor: SharedSupervisor = {
        let sup = Supervisor::new(StdSpawner, path.clone());
        let registry = remotes::load();
        for remote in registry.all() {
            sup.add(remote);
        }
        if !registry.all().is_empty() {
            eprintln!("[roost daemon] supervisor: started {} tunnel watcher(s)", registry.all().len());
        }
        Some(Arc::new(sup))
    };

    // In test builds the supervisor slot is always None (no real spawning).
    #[cfg(test)]
    let supervisor: SharedSupervisor = None;
    // Suppress "unused variable" warning: in test builds the supervisor is
    // never read directly (all arms use cfg-gated `let sup_clone = None`).
    #[cfg(test)]
    let _ = &supervisor;

    // Spawn the liveness (keep-alive) thread after the supervisor is ready so
    // the thread can query tunnel state via the supervisor Arc.
    {
        let state_clone = Arc::clone(&state);
        let history_clone = history.clone();
        #[cfg(not(test))]
        let sup_clone: SharedSupervisor = supervisor.as_ref().map(Arc::clone);
        #[cfg(test)]
        let sup_clone: SharedSupervisor = None;
        std::thread::spawn(move || {
            run_liveness_thread(state_clone, history_clone, sup_clone);
        });
    }

    eprintln!("[roost daemon] listening on {}", path.display());

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state_clone = Arc::clone(&state);
                let history_clone = history.clone();
                // Clone the supervisor Arc so each connection handler can call add/remove.
                #[cfg(not(test))]
                let sup_clone: SharedSupervisor = supervisor.as_ref().map(Arc::clone);
                #[cfg(test)]
                let sup_clone: SharedSupervisor = None;
                std::thread::spawn(move || {
                    if let Err(e) = handle_connection(stream, state_clone, history_clone, sup_clone) {
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

fn handle_connection(
    stream: UnixStream,
    state: Arc<Mutex<DaemonState>>,
    history: SharedHistory,
    #[allow(unused_variables)] supervisor: SharedSupervisor,
) -> Result<()> {
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
                // Normalise origin: None → "local".
                let origin: String = event
                    .origin
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("local")
                    .to_string();
                let key: SessionKey = (origin.clone(), session_id.clone());

                // Check outside the lock whether this is a new session.
                // We must NOT hold the lock while calling resolve_name (forks git).
                let is_new = {
                    let st = state.lock().unwrap_or_else(|p| p.into_inner());
                    !st.sessions.contains_key(&key)
                };

                // Resolve name outside the lock to avoid blocking the daemon
                // while git runs as a subprocess.
                let resolved_name = if is_new {
                    Some(resolve_name(&cwd))
                } else {
                    None
                };

                // Collect any turns closed by this event so they can be written
                // to the history DB *after* the sessions lock is released (DB I/O
                // must not be held under the hot sessions mutex).
                //
                // Similarly, collect any notification payload (family, name,
                // activity, target state) for edge-triggered notifications.
                // Both are gathered inside the lock and processed outside.
                let mut closed: Vec<ClosedTurn> = Vec::new();
                // Captured under the lock, fired after it drops — never spawn while holding the sessions mutex.
                let mut notify_payload: Option<(AgentFamily, String, Option<String>, AgentState, String)> =
                    None;
                {
                    let mut st = state.lock().unwrap_or_else(|p| p.into_inner());

                    let session_ended = if let Some(session) = st.sessions.get_mut(&key) {
                        let old = session.state;
                        let ended = session.apply(&event);
                        let new = session.state;
                        if let Some(t) = session.take_closed_turn() {
                            closed.push(t);
                        }
                        if ended && let Some(t) = session.flush_open_turn() {
                            closed.push(t);
                        }
                        // Edge detection: record payload if this transition should notify.
                        if let Some(target) = notify::trigger_for_transition(old, new) {
                            notify_payload = Some((
                                session.family.clone(),
                                session.name.clone(),
                                session.activity.clone(),
                                target,
                                session.origin.clone(),
                            ));
                        }
                        ended
                    } else {
                        // New session: create then apply
                        let mut session = AgentSession::new(&event);
                        session.name = resolved_name.unwrap_or_else(|| resolve_name(&cwd));
                        let old = session.state;
                        let ended = session.apply(&event);
                        let new = session.state;
                        if let Some(t) = session.take_closed_turn() {
                            closed.push(t);
                        }
                        if ended && let Some(t) = session.flush_open_turn() {
                            closed.push(t);
                        }
                        // Edge detection for new session.
                        if let Some(target) = notify::trigger_for_transition(old, new) {
                            notify_payload = Some((
                                session.family.clone(),
                                session.name.clone(),
                                session.activity.clone(),
                                target,
                                session.origin.clone(),
                            ));
                        }
                        st.sessions.insert(key.clone(), session);
                        // Initialise liveness entry.
                        st.liveness.entry(key.clone()).or_default();
                        ended
                    };

                    if session_ended {
                        st.sessions.remove(&key);
                        st.liveness.remove(&key);
                    } else {
                        // Reset consecutive dead count on any fresh event.
                        if let Some(meta) = st.liveness.get_mut(&key) {
                            meta.consecutive_dead = 0;
                        }
                    }
                }

                record_turns(&history, &closed);

                // Fire notification outside the sessions lock (notify::fire may
                // spawn subprocesses; holding the lock during a spawn is forbidden).
                if let Some((family, name, activity, target, origin)) = notify_payload {
                    let cfg = config::load();
                    if let Some(stage) = cfg.stage_for(target) {
                        notify::fire(
                            &family, &name, activity.as_deref(),
                            target, &origin, stage.banner, stage.sound,
                        );
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
                    // Local TUI requests detail for local sessions only.
                    st.session_detail("local", &session_id, Instant::now())
                };
                let json = match detail {
                    Some(d) => serde_json::to_string(&d).unwrap_or_else(|_| "null".to_string()),
                    None => "null".to_string(),
                };
                writeln!(write_stream, "{json}")?;
            }
            Request::AddRemote { target, origin, remote_sock, arch } => {
                // Immediately start a tunnel watcher for the new remote.
                // supervisor is only available in non-test production builds.
                #[cfg(not(test))]
                if let Some(ref sup) = supervisor {
                    use crate::remotes::Remote;
                    let remote = Remote {
                        origin: origin.clone(),
                        target: target.clone(),
                        remote_sock,
                        added_at: String::new(), // already persisted in remotes.json
                        arch,
                    };
                    sup.add(&remote);
                    eprintln!("[roost daemon] AddRemote: started tunnel watcher for {origin} ({target})");
                } else {
                    eprintln!("[roost daemon] AddRemote: registered {origin} ({target}); tunnel starts on next daemon restart.");
                }
                #[cfg(test)]
                {
                    let _ = (target, origin, remote_sock, arch);
                }
            }
            Request::RemoveRemote { origin } => {
                // Immediately stop the tunnel watcher for the remote.
                #[cfg(not(test))]
                if let Some(ref sup) = supervisor {
                    let removed = sup.remove(&origin);
                    if removed {
                        eprintln!("[roost daemon] RemoveRemote: stopped tunnel watcher for {origin}");
                    } else {
                        eprintln!("[roost daemon] RemoveRemote: no active watcher found for {origin}");
                    }
                } else {
                    eprintln!("[roost daemon] RemoveRemote: {origin} — no supervisor (tunnel was not running)");
                }
                #[cfg(test)]
                {
                    let _ = origin;
                }
            }
            Request::ListRemotes => {
                // Return the registered remotes with their current tunnel states.
                #[cfg(not(test))]
                use crate::protocol::{RemoteStatus, TunnelState};
                #[cfg(not(test))]
                {
                    let registry = remotes::load();
                    let statuses: Vec<RemoteStatus> = registry
                        .all()
                        .iter()
                        .map(|r| {
                            let tunnel = if let Some(ref sup) = supervisor {
                                match sup.state_of(&r.origin) {
                                    Some(crate::remote_supervisor::ConnState::Connected) => TunnelState::Up,
                                    Some(crate::remote_supervisor::ConnState::Reconnecting) => TunnelState::Reconnecting,
                                    Some(crate::remote_supervisor::ConnState::Down) => TunnelState::Down,
                                    None => TunnelState::Unknown,
                                }
                            } else {
                                TunnelState::Unknown
                            };
                            RemoteStatus {
                                origin: r.origin.clone(),
                                target: r.target.clone(),
                                arch: r.arch.clone(),
                                added_at: r.added_at.clone(),
                                tunnel,
                            }
                        })
                        .collect();
                    let json = serde_json::to_string(&statuses)
                        .unwrap_or_else(|_| "[]".to_string());
                    let _ = writeln!(write_stream, "{json}");
                }
                #[cfg(test)]
                {
                    let _ = writeln!(write_stream, "[]");
                }
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

/// The liveness thread: runs every ~2s, probes each session's liveness, calls
/// `liveness::decide` with source-awareness and executes the decided action.
///
/// Local sessions use the existing PID-probe path (zero regression).
/// Remote sessions skip PID probing and use the tunnel state instead.
fn run_liveness_thread(
    state: Arc<Mutex<DaemonState>>,
    history: SharedHistory,
    supervisor: SharedSupervisor,
) {
    loop {
        std::thread::sleep(LIVENESS_INTERVAL);

        let now = Instant::now();

        // Collect session metadata outside the lock to minimise lock hold time.
        // Each tuple: (key, origin, pid, last_activity, consecutive_dead)
        let probes: Vec<(SessionKey, String, Option<u32>, Instant, u32)> = {
            let st = state.lock().unwrap_or_else(|p| p.into_inner());
            st.sessions
                .iter()
                .map(|(key, sess)| {
                    let consecutive_dead =
                        st.liveness.get(key).map(|m| m.consecutive_dead).unwrap_or(0);
                    (key.clone(), sess.origin.clone(), sess.pid, sess.last_activity, consecutive_dead)
                })
                .collect()
        };

        for (key, origin, pid_opt, last_activity, consecutive_dead) in probes {
            let is_local = origin == "local";
            let idle_since = now.duration_since(last_activity);

            // ── Local path: PID probe (unchanged behaviour) ───────────────────
            let new_consecutive = if is_local {
                match pid_opt {
                    Some(pid) if !pid_is_alive(pid) => consecutive_dead + 1,
                    _ => 0,
                }
            } else {
                // Remote sessions: PID is on a different machine; skip probe.
                0
            };

            // ── Resolve tunnel state for remote sessions ──────────────────────
            // In test builds, supervisor is always None, so we default to
            // Connected (safest: does not trigger spurious Freeze or Remove).
            let tunnel = if is_local {
                // Tunnel state is irrelevant for local sessions (ignored by decide).
                liveness::TunnelState::Connected
            } else {
                #[cfg(not(test))]
                {
                    match supervisor.as_ref().and_then(|sup| sup.state_of(&origin)) {
                        Some(crate::remote_supervisor::ConnState::Connected) => {
                            liveness::TunnelState::Connected
                        }
                        Some(crate::remote_supervisor::ConnState::Reconnecting) => {
                            liveness::TunnelState::Reconnecting
                        }
                        Some(crate::remote_supervisor::ConnState::Down) => {
                            liveness::TunnelState::Down
                        }
                        // Origin not tracked by supervisor (e.g. supervisor not yet
                        // started, or remote removed): treat as Connected so we do
                        // not spuriously freeze sessions.
                        None => liveness::TunnelState::Connected,
                    }
                }
                #[cfg(test)]
                {
                    // Test build: no real supervisor; default to Connected so
                    // remote sessions in test state are not spuriously frozen.
                    let _ = &supervisor;
                    liveness::TunnelState::Connected
                }
            };

            // ── Pure liveness decision ────────────────────────────────────────
            // session_ended=false: SessionEnd sessions are removed synchronously
            // in the event path before the liveness thread can observe them.
            let action = liveness::decide(is_local, tunnel, new_consecutive, idle_since, false);

            // ── Execute the decision ──────────────────────────────────────────
            let mut closed: Option<crate::history::ClosedTurn> = None;
            // Set to the origin string when an offline notification should fire
            // (determined inside the lock, fired outside it).
            let mut fire_offline_for: Option<String> = None;
            {
                let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
                match action {
                    liveness::LivenessAction::Remove => {
                        // Flush the in-flight turn before dropping the session.
                        if let Some(session) = st.sessions.get_mut(&key) {
                            closed = session.flush_open_turn();
                        }
                        st.sessions.remove(&key);
                        st.liveness.remove(&key);
                    }
                    liveness::LivenessAction::Freeze => {
                        // Freeze busy timer and mark stale; keep session in table.
                        if let Some(session) = st.sessions.get_mut(&key) {
                            session.freeze_stale(now);
                        }
                        // Reset consecutive_dead (not applicable for remote) so
                        // it does not accumulate.
                        if let Some(meta) = st.liveness.get_mut(&key) {
                            meta.consecutive_dead = 0;
                            // Fire a remote offline notification at most once per
                            // Freeze event (i.e. the first time this session's
                            // tunnel drops to Down).  The config gate is checked
                            // outside the lock to avoid I/O while holding it.
                            if !meta.offline_notified {
                                meta.offline_notified = true;
                                fire_offline_for = Some(origin.clone());
                            }
                        }
                    }
                    liveness::LivenessAction::ToIdle => {
                        // Transition session to Idle (stop active timer if running).
                        if let Some(session) = st.sessions.get_mut(&key) {
                            use crate::session::{is_active, AgentState};
                            if is_active(&session.state) {
                                // Freeze the active segment (same as Done/Idle transition).
                                if let Some(since) = session.active_since.take() {
                                    session.active_accum += now.duration_since(since);
                                }
                                session.state = AgentState::Idle;
                                session.activity = None;
                            }
                        }
                    }
                    liveness::LivenessAction::Keep => {
                        // Update consecutive dead count for local sessions.
                        if let Some(meta) = st.liveness.get_mut(&key) {
                            meta.consecutive_dead = new_consecutive;
                        }
                    }
                }
            }
            if let Some(t) = closed {
                record_turns(&history, std::slice::from_ref(&t));
            }
            // Fire offline notification outside the lock (I/O + subprocess spawn).
            // Only fires when remote_offline is enabled in config (default: false).
            if let Some(ref offline_origin) = fire_offline_for {
                let cfg = config::load();
                if cfg.notify.remote_offline {
                    notify::fire_offline(offline_origin, true, true);
                }
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
        use crate::protocol::{EventKind, HookEvent};

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
            origin: None,
        };

        let mut s1 = AgentSession::new(&start_ev);
        s1.name = "working-session".to_string();
        s1.state = AgentState::Working;
        state.sessions.insert(("local".to_string(), "s1".to_string()), s1);

        let mut s2 = AgentSession::new(&start_ev);
        s2.id = "s2".to_string();
        s2.name = "approval-session".to_string();
        s2.state = AgentState::Approval;
        state.sessions.insert(("local".to_string(), "s2".to_string()), s2);

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
        use crate::protocol::{EventKind, HookEvent};
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
            origin: None,
        };

        let mut state = DaemonState::new();
        let mut s = AgentSession::new(&start_ev);
        // Done: a fixed amount of accumulated busy time, no open active segment.
        s.state = AgentState::Done;
        s.active_accum = Duration::from_secs(42);
        s.active_since = None;
        state.sessions.insert(("local".to_string(), "s1".to_string()), s);

        let base = Instant::now();
        let a = state.session_detail("local", "s1", base).unwrap().since_secs;
        let b = state
            .session_detail("local", "s1", base + Duration::from_secs(10))
            .unwrap()
            .since_secs;
        assert_eq!(a, 42, "since should reflect frozen busy time");
        assert_eq!(a, b, "since must not advance after the session is Done");
    }

    /// 状态边沿测试：Working → Approval 通知边沿（带 permission 关键词的 Notification）
    /// 以及 Working → Working（PostToolUse 保持 Working）不触发通知。
    /// 注意：只验证 trigger_for_transition 的计算结果，不调用 fire。
    #[test]
    fn edge_working_to_approval_triggers_notification() {
        use crate::protocol::{EventKind, HookEvent};

        let start_ev = HookEvent {
            kind: EventKind::SessionStart,
            family: AgentFamily::Claude,
            session_id: "edge-test".to_string(),
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

        // SessionStart → Idle
        let mut session = AgentSession::new(&start_ev);
        session.apply(&start_ev);
        assert_eq!(session.state, AgentState::Idle);

        // UserPromptSubmit → Working
        let mut prompt_ev = start_ev.clone();
        prompt_ev.kind = EventKind::UserPromptSubmit;
        session.apply(&prompt_ev);
        assert_eq!(session.state, AgentState::Working);

        // PostToolUse: Working → Working，边沿计算应为 None
        let mut post_ev = start_ev.clone();
        post_ev.kind = EventKind::PostToolUse;
        post_ev.tool_name = Some("Bash".to_string());
        let old = session.state;
        session.apply(&post_ev);
        let new = session.state;
        assert_eq!(session.state, AgentState::Working);
        assert_eq!(
            notify::trigger_for_transition(old, new),
            None,
            "Working→Working (PostToolUse) 不应触发通知"
        );

        // Notification with "permission" → Working → Approval，边沿计算应为 Some(Approval)
        let mut notif_ev = start_ev.clone();
        notif_ev.kind = EventKind::Notification;
        notif_ev.notification = Some("Claude needs your permission to run: git push".to_string());
        let old = session.state;
        session.apply(&notif_ev);
        let new = session.state;
        assert_eq!(session.state, AgentState::Approval);
        assert_eq!(
            notify::trigger_for_transition(old, new),
            Some(AgentState::Approval),
            "Working→Approval 应触发通知"
        );
    }

    // ── A2: (origin, session_id) namespace tests ──────────────────────────────

    /// Helper: build a minimal HookEvent with the given session_id and optional origin.
    fn make_hook_event(session_id: &str, origin: Option<&str>) -> crate::protocol::HookEvent {
        use crate::protocol::{AgentFamily, EventKind, HookEvent};
        HookEvent {
            kind: EventKind::SessionStart,
            family: AgentFamily::Claude,
            session_id: session_id.to_string(),
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
            origin: origin.map(|s| s.to_string()),
        }
    }

    /// 不同 origin、相同 session_id 必须产生两个独立会话，互不覆盖。
    #[test]
    fn same_session_id_different_origin_does_not_collide() {
        use crate::session::AgentSession;

        let mut state = DaemonState::new();

        let ev_local = make_hook_event("sess-abc", None); // origin=None → "local"
        let ev_remote = make_hook_event("sess-abc", Some("devbox1"));

        let mut s_local = AgentSession::new(&ev_local);
        s_local.name = "local-session".to_string();

        let mut s_remote = AgentSession::new(&ev_remote);
        s_remote.name = "remote-session".to_string();

        // Insert via composite key (origin, session_id).
        state
            .sessions
            .insert(("local".to_string(), "sess-abc".to_string()), s_local);
        state
            .sessions
            .insert(("devbox1".to_string(), "sess-abc".to_string()), s_remote);

        assert_eq!(state.sessions.len(), 2, "two distinct keys must coexist");
    }

    /// None origin は "local" に帰一される：HookEvent.origin=None で作成したセッションの
    /// origin フィールドは "local" でなければならない。
    #[test]
    fn none_origin_normalises_to_local() {
        use crate::session::AgentSession;

        let ev = make_hook_event("s1", None); // origin=None
        let session = AgentSession::new(&ev);
        assert_eq!(
            session.origin, "local",
            "origin=None must be normalised to \"local\""
        );
    }

    /// SessionView.origin は AgentSession.origin の実際の値を反映しなければならない。
    #[test]
    fn session_view_origin_reflects_agent_session_origin() {
        use crate::session::{AgentSession, AgentState};

        let mut state = DaemonState::new();

        let ev_local = make_hook_event("s1", None);
        let ev_remote = make_hook_event("s2", Some("devbox2"));

        let mut s_local = AgentSession::new(&ev_local);
        s_local.name = "local-repo".to_string();
        s_local.state = AgentState::Idle;

        let mut s_remote = AgentSession::new(&ev_remote);
        s_remote.name = "remote-repo".to_string();
        s_remote.state = AgentState::Idle;

        state
            .sessions
            .insert(("local".to_string(), "s1".to_string()), s_local);
        state
            .sessions
            .insert(("devbox2".to_string(), "s2".to_string()), s_remote);

        let views = state.session_views(Instant::now());
        assert_eq!(views.len(), 2);

        let local_view = views.iter().find(|v| v.id == "s1").expect("s1 must be present");
        let remote_view = views.iter().find(|v| v.id == "s2").expect("s2 must be present");

        assert_eq!(local_view.origin, "local", "local session view must have origin=\"local\"");
        assert_eq!(
            remote_view.origin, "devbox2",
            "remote session view must carry the actual origin"
        );
    }

    /// 本机イベント（origin=None）は "local" として扱われ、既存動作を維持する。
    #[test]
    fn local_event_origin_is_local() {
        use crate::session::AgentSession;

        let ev = make_hook_event("local-sess", None);
        let session = AgentSession::new(&ev);
        assert_eq!(session.origin, "local");

        let mut state = DaemonState::new();
        state
            .sessions
            .insert(("local".to_string(), "local-sess".to_string()), session);

        let views = state.session_views(Instant::now());
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].origin, "local");
    }

    /// Regression: completed-step durations in the peek "recent" list must be
    /// FROZEN — identical no matter what `now` we render at. The previous
    /// implementation subtracted two separately-floored ages, which flickered by
    /// ±1s as the clock advanced (the "乱跳" the user observed).
    #[test]
    fn recent_event_durations_are_stable_across_now() {
        use crate::protocol::{EventKind, HookEvent};
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
            origin: None,
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
        state.sessions.insert(("local".to_string(), "s1".to_string()), s);

        // Render at several `now` offsets; completed durations must not change.
        let durations_at = |delta_ms: u64| -> Vec<Option<u64>> {
            let now = base + Duration::from_millis(delta_ms);
            state
                .session_detail("local", "s1", now)
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

    // ── A7: stale flag integration tests ──────────────────────────────────────

    /// session_views reflects the true stale flag from AgentSession.
    #[test]
    fn session_view_stale_reflects_agent_session_stale() {
        use crate::session::{AgentSession, AgentState};

        let mut state = DaemonState::new();

        let ev_remote = make_hook_event("s-stale", Some("devbox1"));
        let mut s_stale = AgentSession::new(&ev_remote);
        s_stale.name = "stale-repo".to_string();
        s_stale.state = AgentState::Working;
        // Manually mark stale (as the liveness thread would after Freeze action).
        s_stale.stale = true;
        s_stale.active_since = None; // busy timer frozen
        s_stale.active_accum = std::time::Duration::from_secs(42);

        let ev_fresh = make_hook_event("s-fresh", Some("devbox2"));
        let mut s_fresh = AgentSession::new(&ev_fresh);
        s_fresh.name = "fresh-repo".to_string();
        s_fresh.state = AgentState::Working;
        s_fresh.stale = false;

        state.sessions.insert(("devbox1".to_string(), "s-stale".to_string()), s_stale);
        state.sessions.insert(("devbox2".to_string(), "s-fresh".to_string()), s_fresh);

        let views = state.session_views(Instant::now());
        let stale_view = views.iter().find(|v| v.id == "s-stale").unwrap();
        let fresh_view = views.iter().find(|v| v.id == "s-fresh").unwrap();

        assert!(stale_view.stale, "stale session view must have stale=true");
        assert!(!fresh_view.stale, "fresh session view must have stale=false");
    }

    /// A stale session's busy_secs is frozen (does not advance with now).
    #[test]
    fn stale_session_busy_secs_does_not_advance() {
        use crate::session::{AgentSession, AgentState};
        use std::time::Duration;

        let mut state = DaemonState::new();

        let ev = make_hook_event("s1", Some("devbox1"));
        let mut s = AgentSession::new(&ev);
        s.state = AgentState::Working;
        s.stale = true;
        s.active_since = None; // frozen
        s.active_accum = Duration::from_secs(77);
        state.sessions.insert(("devbox1".to_string(), "s1".to_string()), s);

        let base = Instant::now();
        let views_now = state.session_views(base);
        let views_later = state.session_views(base + Duration::from_secs(60));

        let busy_now = views_now.iter().find(|v| v.id == "s1").unwrap().busy_secs;
        let busy_later = views_later.iter().find(|v| v.id == "s1").unwrap().busy_secs;

        assert_eq!(busy_now, 77, "frozen busy should be 77s");
        assert_eq!(busy_now, busy_later, "stale busy_secs must not advance");
    }

    /// session_views 必须把 AgentSession.cwd 完整填入 SessionView.cwd，
    /// 同时保持 name 仍是末段 basename，两者独立。
    #[test]
    fn session_view_cwd_reflects_agent_session_cwd() {
        use crate::session::{AgentSession, AgentState};

        let mut ev = make_hook_event("s-cwd", None);
        ev.cwd = "/home/dev/acme/payments-api".to_string();

        let mut s = AgentSession::new(&ev);
        s.name = "payments-api".to_string();
        s.state = AgentState::Idle;

        let mut state = DaemonState::new();
        state.sessions.insert(("local".to_string(), "s-cwd".to_string()), s);

        let views = state.session_views(Instant::now());
        let view = views.iter().find(|v| v.id == "s-cwd").expect("view must exist");

        assert_eq!(
            view.cwd, "/home/dev/acme/payments-api",
            "SessionView.cwd must carry the full working directory"
        );
        assert_eq!(
            view.name, "payments-api",
            "SessionView.name must remain the basename and be unaffected by cwd"
        );
    }
}
