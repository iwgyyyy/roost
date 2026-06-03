//! SSH tunnel supervisor for remote agent fleet.
//!
//! Each registered [`Remote`] needs a persistent reverse-tunnel:
//!   `ssh -N -R <remote_sock>:<local_sock> <target>`
//!
//! This module provides:
//! - Pure functions: [`build_argv`], [`next_backoff`]
//! - A pure state machine: [`ConnState`] + [`ConnEvent`] + [`step`]
//! - A [`Spawner`] seam trait so tests can mock process creation
//! - [`Supervisor`]: one watcher thread per remote, with exponential back-off reconnect
//!
//! ## Safety constraint
//! No code in this file executes `ssh` or touches real sockets during tests.
//! The production [`StdSpawner`] is the only place that calls
//! `std::process::Command` and it is NEVER used in `#[cfg(test)]`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::remotes::Remote;

// ---------------------------------------------------------------------------
// 1. Pure function: build ssh argv
// ---------------------------------------------------------------------------

/// Build the `ssh` argument vector for a reverse tunnel.
///
/// Produces:
/// ```text
/// ssh -N
///   -o ExitOnForwardFailure=yes
///   -o ServerAliveInterval=15
///   -o ServerAliveCountMax=3
///   -o StreamLocalBindUnlink=yes
///   -R <remote_sock>:<local_sock>
///   <target>
/// ```
///
/// The return value includes `"ssh"` as element 0 so the caller can pass it
/// directly to `Command::new(&argv[0]).args(&argv[1..])`.
pub fn build_argv(remote: &Remote, local_sock: &Path) -> Vec<String> {
    let local_sock_str = local_sock.to_string_lossy().into_owned();
    vec![
        "ssh".to_string(),
        "-N".to_string(),
        "-o".to_string(),
        "ExitOnForwardFailure=yes".to_string(),
        "-o".to_string(),
        "ServerAliveInterval=15".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=3".to_string(),
        "-o".to_string(),
        "StreamLocalBindUnlink=yes".to_string(),
        "-R".to_string(),
        format!("{}:{}", remote.remote_sock, local_sock_str),
        remote.target.clone(),
    ]
}

// ---------------------------------------------------------------------------
// 2. Pure function: exponential back-off schedule
// ---------------------------------------------------------------------------

/// Compute the back-off [`Duration`] for the given attempt number (0-indexed).
///
/// Schedule: 1s × 2^attempt, capped at 30s.
///
/// | attempt | delay |
/// |---------|-------|
/// | 0       | 1 s   |
/// | 1       | 2 s   |
/// | 2       | 4 s   |
/// | 3       | 8 s   |
/// | 4       | 16 s  |
/// | 5+      | 30 s  |
pub fn next_backoff(attempt: u32) -> Duration {
    const MAX_SECS: u64 = 30;
    // 1 << attempt saturates at u64::MAX; then min with MAX_SECS keeps it sane.
    let secs = (1u64 << attempt.min(63)).min(MAX_SECS);
    Duration::from_secs(secs)
}

// ---------------------------------------------------------------------------
// 3. Pure state machine
// ---------------------------------------------------------------------------

/// Connection state of a single tunnel watcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    /// SSH process is running and the tunnel is presumed up.
    Connected,
    /// SSH exited; the watcher is waiting before the next attempt.
    Reconnecting,
    /// Reconnect attempts exhausted; giving up.
    Down,
}

/// Events that drive state transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnEvent {
    /// SSH process started successfully.
    ProcessStarted,
    /// SSH process exited (regardless of exit code).
    ProcessExited,
    /// Reconnect attempts have been exhausted.
    AttemptsExhausted,
}

/// Pure state-machine transition function.
///
/// Returns the next state; any unhandled (state, event) pair leaves the state
/// unchanged — this is intentionally lenient so callers don't have to guard.
pub fn step(state: ConnState, event: ConnEvent) -> ConnState {
    match (state, event) {
        (ConnState::Reconnecting, ConnEvent::ProcessStarted) => ConnState::Connected,
        (ConnState::Connected, ConnEvent::ProcessExited) => ConnState::Reconnecting,
        (ConnState::Reconnecting, ConnEvent::ProcessExited) => ConnState::Reconnecting,
        (ConnState::Reconnecting, ConnEvent::AttemptsExhausted) => ConnState::Down,
        // From Down or any other combo: stay put.
        _ => state,
    }
}

// ---------------------------------------------------------------------------
// 4. Spawner seam
// ---------------------------------------------------------------------------

/// Maximum number of reconnect attempts before a watcher gives up.
pub const MAX_ATTEMPTS: u32 = 8;

/// A handle to a spawned "ssh" process (or mock substitute).
///
/// The supervisor calls [`ProcessHandle::wait`] to block until the process exits.
pub trait ProcessHandle: Send + 'static {
    /// Block until the process exits and return its exit status (true = success).
    fn wait(&mut self) -> bool;
}

/// Seam trait: abstracts spawning a child process from an argv.
///
/// In production: `StdSpawner` calls `std::process::Command`.
/// In tests: `MockSpawner` records calls and returns pre-programmed handles.
pub trait Spawner: Send + Sync + 'static {
    /// Spawn the command described by `argv` (argv[0] is the program name).
    /// Returns `Ok(handle)` on success, `Err(message)` on spawn failure.
    fn spawn(&self, argv: &[String]) -> Result<Box<dyn ProcessHandle>, String>;
}

// ---------------------------------------------------------------------------
// Production Spawner (only compiled in non-test builds)
// ---------------------------------------------------------------------------

/// Production [`Spawner`] that delegates to [`std::process::Command`].
///
/// This struct is intentionally absent from test builds so that no test can
/// accidentally import it and trigger a real `ssh` spawn.
#[cfg(not(test))]
pub struct StdSpawner;

#[cfg(not(test))]
struct StdHandle(std::process::Child);

#[cfg(not(test))]
impl ProcessHandle for StdHandle {
    fn wait(&mut self) -> bool {
        self.0
            .wait()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

#[cfg(not(test))]
impl Spawner for StdSpawner {
    fn spawn(&self, argv: &[String]) -> Result<Box<dyn ProcessHandle>, String> {
        if argv.is_empty() {
            return Err("empty argv".to_string());
        }
        let child = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .spawn()
            .map_err(|e| format!("spawn failed: {e}"))?;
        Ok(Box::new(StdHandle(child)))
    }
}

// ---------------------------------------------------------------------------
// 5. Per-remote watcher (runs in its own thread)
// ---------------------------------------------------------------------------

/// Watch a single remote: spawn ssh, wait for exit, back-off, repeat.
///
/// Signals [`ConnState`] transitions into the shared `state` slot.
/// Stops when `stop` is set to `true` (checked between retries).
fn run_watcher<S: Spawner>(
    origin: String,
    argv: Vec<String>,
    spawner: Arc<S>,
    state: Arc<Mutex<ConnState>>,
    stop: Arc<Mutex<bool>>,
) {
    let mut attempt: u32 = 0;

    loop {
        // Check stop flag before each attempt.
        if *stop.lock().unwrap_or_else(|p| p.into_inner()) {
            break;
        }

        // Transition: Reconnecting → (attempt to start) → Connected or stay Reconnecting.
        match spawner.spawn(&argv) {
            Ok(mut handle) => {
                // Mark Connected.
                {
                    let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
                    *st = step(*st, ConnEvent::ProcessStarted);
                }
                // Block until ssh exits.
                handle.wait();
                // Mark as needing reconnect.
                {
                    let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
                    *st = step(*st, ConnEvent::ProcessExited);
                }
                attempt = 0; // reset on a successful start (connection was live)
            }
            Err(e) => {
                eprintln!("[roost supervisor] {origin}: spawn error: {e}");
            }
        }

        attempt += 1;
        if attempt > MAX_ATTEMPTS {
            let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
            *st = step(*st, ConnEvent::AttemptsExhausted);
            eprintln!("[roost supervisor] {origin}: giving up after {MAX_ATTEMPTS} attempts");
            break;
        }

        let delay = next_backoff(attempt - 1);
        // Honour stop flag during sleep by checking in small intervals.
        let step_ms = 100u64;
        let total_ms = delay.as_millis() as u64;
        let mut slept = 0u64;
        while slept < total_ms {
            std::thread::sleep(Duration::from_millis(step_ms.min(total_ms - slept)));
            slept += step_ms;
            if *stop.lock().unwrap_or_else(|p| p.into_inner()) {
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 6. Supervisor: manages all per-remote watchers
// ---------------------------------------------------------------------------

/// Per-entry bookkeeping inside the Supervisor.
struct WatcherEntry {
    state: Arc<Mutex<ConnState>>,
    stop: Arc<Mutex<bool>>,
}

/// Manages SSH tunnel watchers for all registered remotes.
///
/// Each call to [`Supervisor::add`] spawns a background thread that maintains
/// a persistent reverse-tunnel for that remote.  [`Supervisor::remove`] signals
/// the watcher to stop.
pub struct Supervisor<S: Spawner> {
    spawner: Arc<S>,
    local_sock: std::path::PathBuf,
    watchers: Mutex<HashMap<String, WatcherEntry>>,
}

impl<S: Spawner> Supervisor<S> {
    /// Create a new Supervisor.
    ///
    /// `local_sock` is the path to the local daemon socket (used as the target
    /// of the `-R` reverse-forward).
    pub fn new(spawner: S, local_sock: std::path::PathBuf) -> Self {
        Self {
            spawner: Arc::new(spawner),
            local_sock,
            watchers: Mutex::new(HashMap::new()),
        }
    }

    /// Start watching `remote`, launching a tunnel watcher thread.
    ///
    /// If a watcher for `remote.origin` already exists, this is a no-op.
    pub fn add(&self, remote: &Remote) {
        let mut watchers = self.watchers.lock().unwrap_or_else(|p| p.into_inner());
        if watchers.contains_key(&remote.origin) {
            return;
        }

        let argv = build_argv(remote, &self.local_sock);
        let state = Arc::new(Mutex::new(ConnState::Reconnecting));
        let stop = Arc::new(Mutex::new(false));

        let entry = WatcherEntry {
            state: Arc::clone(&state),
            stop: Arc::clone(&stop),
        };
        watchers.insert(remote.origin.clone(), entry);

        let origin = remote.origin.clone();
        let spawner = Arc::clone(&self.spawner);

        std::thread::spawn(move || {
            run_watcher(origin, argv, spawner, state, stop);
        });
    }

    /// Stop watching `origin`, signalling the watcher thread to exit.
    ///
    /// Returns `true` if a watcher was found and signalled, `false` if unknown.
    pub fn remove(&self, origin: &str) -> bool {
        let mut watchers = self.watchers.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = watchers.remove(origin) {
            *entry.stop.lock().unwrap_or_else(|p| p.into_inner()) = true;
            true
        } else {
            false
        }
    }

    /// Return the current [`ConnState`] for a remote, or `None` if not tracked.
    pub fn state_of(&self, origin: &str) -> Option<ConnState> {
        let watchers = self.watchers.lock().unwrap_or_else(|p| p.into_inner());
        watchers
            .get(origin)
            .map(|e| *e.state.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    // ── helpers ─────────────────────────────────────────────────────────────

    fn make_remote(origin: &str, target: &str, remote_sock: &str) -> Remote {
        Remote {
            origin: origin.to_string(),
            target: target.to_string(),
            remote_sock: remote_sock.to_string(),
            added_at: "2026-06-03T00:00:00Z".to_string(),
            arch: "linux-x86_64".to_string(),
        }
    }

    // ── 1. argv construction ─────────────────────────────────────────────────

    #[test]
    fn argv_starts_with_ssh_minus_n() {
        let r = make_remote("box1", "user@box1.example.com", "/tmp/roost.sock");
        let argv = build_argv(&r, Path::new("/var/run/local.sock"));
        assert_eq!(argv[0], "ssh");
        assert_eq!(argv[1], "-N");
    }

    #[test]
    fn argv_contains_exit_on_forward_failure() {
        let r = make_remote("box1", "user@box1.example.com", "/tmp/roost.sock");
        let argv = build_argv(&r, Path::new("/var/run/local.sock"));
        let joined = argv.join(" ");
        assert!(
            joined.contains("ExitOnForwardFailure=yes"),
            "argv missing ExitOnForwardFailure=yes: {joined}"
        );
    }

    #[test]
    fn argv_contains_server_alive_options() {
        let r = make_remote("box1", "user@box1.example.com", "/tmp/roost.sock");
        let argv = build_argv(&r, Path::new("/var/run/local.sock"));
        let joined = argv.join(" ");
        assert!(joined.contains("ServerAliveInterval=15"), "{joined}");
        assert!(joined.contains("ServerAliveCountMax=3"), "{joined}");
    }

    #[test]
    fn argv_contains_stream_local_bind_unlink() {
        let r = make_remote("box1", "user@box1.example.com", "/tmp/roost.sock");
        let argv = build_argv(&r, Path::new("/var/run/local.sock"));
        let joined = argv.join(" ");
        assert!(joined.contains("StreamLocalBindUnlink=yes"), "{joined}");
    }

    #[test]
    fn argv_reverse_forward_arg_is_correct() {
        let r = make_remote("box1", "user@box1.example.com", "/remote/roost.sock");
        let argv = build_argv(&r, Path::new("/local/daemon.sock"));
        // -R must be followed by remote_sock:local_sock
        let r_pos = argv.iter().position(|a| a == "-R").expect("-R not found");
        assert_eq!(argv[r_pos + 1], "/remote/roost.sock:/local/daemon.sock");
    }

    #[test]
    fn argv_last_element_is_target() {
        let r = make_remote("box1", "user@box1.example.com", "/tmp/roost.sock");
        let argv = build_argv(&r, Path::new("/var/run/local.sock"));
        assert_eq!(argv.last().unwrap(), "user@box1.example.com");
    }

    // ── 2. back-off schedule ────────────────────────────────────────────────

    #[test]
    fn backoff_attempt0_is_1s() {
        assert_eq!(next_backoff(0), Duration::from_secs(1));
    }

    #[test]
    fn backoff_doubles_each_attempt() {
        assert_eq!(next_backoff(1), Duration::from_secs(2));
        assert_eq!(next_backoff(2), Duration::from_secs(4));
        assert_eq!(next_backoff(3), Duration::from_secs(8));
        assert_eq!(next_backoff(4), Duration::from_secs(16));
    }

    #[test]
    fn backoff_caps_at_30s() {
        assert_eq!(next_backoff(5), Duration::from_secs(30));
        assert_eq!(next_backoff(10), Duration::from_secs(30));
        assert_eq!(next_backoff(100), Duration::from_secs(30));
    }

    // ── 3. state machine ─────────────────────────────────────────────────────

    #[test]
    fn reconnecting_plus_started_gives_connected() {
        assert_eq!(
            step(ConnState::Reconnecting, ConnEvent::ProcessStarted),
            ConnState::Connected
        );
    }

    #[test]
    fn connected_plus_exited_gives_reconnecting() {
        assert_eq!(
            step(ConnState::Connected, ConnEvent::ProcessExited),
            ConnState::Reconnecting
        );
    }

    #[test]
    fn reconnecting_plus_exited_stays_reconnecting() {
        assert_eq!(
            step(ConnState::Reconnecting, ConnEvent::ProcessExited),
            ConnState::Reconnecting
        );
    }

    #[test]
    fn reconnecting_plus_exhausted_gives_down() {
        assert_eq!(
            step(ConnState::Reconnecting, ConnEvent::AttemptsExhausted),
            ConnState::Down
        );
    }

    #[test]
    fn down_ignores_all_events() {
        for ev in [
            ConnEvent::ProcessStarted,
            ConnEvent::ProcessExited,
            ConnEvent::AttemptsExhausted,
        ] {
            assert_eq!(step(ConnState::Down, ev), ConnState::Down, "Down should ignore {ev:?}");
        }
    }

    // ── 4. Mock Spawner ──────────────────────────────────────────────────────

    /// A mock handle that "exits" immediately.
    struct ImmediateExitHandle;

    impl ProcessHandle for ImmediateExitHandle {
        fn wait(&mut self) -> bool {
            false // exits immediately with failure
        }
    }

    /// A mock spawner that counts spawn calls and always fails the process
    /// (process exits immediately), driving the watcher into back-off.
    struct CountingSpawner {
        count: Arc<AtomicU32>,
    }

    impl Spawner for CountingSpawner {
        fn spawn(&self, _argv: &[String]) -> Result<Box<dyn ProcessHandle>, String> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(ImmediateExitHandle))
        }
    }

    /// A mock spawner that records the argv it received.
    struct RecordingSpawner {
        recorded: Arc<Mutex<Vec<Vec<String>>>>,
    }

    impl Spawner for RecordingSpawner {
        fn spawn(&self, argv: &[String]) -> Result<Box<dyn ProcessHandle>, String> {
            self.recorded
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(argv.to_vec());
            Ok(Box::new(ImmediateExitHandle))
        }
    }

    /// A mock spawner that always fails to spawn.
    struct FailSpawner;

    impl Spawner for FailSpawner {
        fn spawn(&self, _argv: &[String]) -> Result<Box<dyn ProcessHandle>, String> {
            Err("mock spawn failure".to_string())
        }
    }

    // ── 5. Supervisor: add/remove and argv plumbing ──────────────────────────

    /// Verify that adding a remote causes the Spawner to be called with the
    /// correct argv (remote_sock:local_sock and target).
    #[test]
    fn supervisor_add_calls_spawner_with_correct_argv() {
        let recorded = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
        let spawner = RecordingSpawner {
            recorded: Arc::clone(&recorded),
        };
        let local_sock = PathBuf::from("/local/daemon.sock");
        let sup = Supervisor::new(spawner, local_sock);

        let remote = make_remote("box1", "user@box1.example.com", "/remote/roost.sock");
        sup.add(&remote);

        // Give the watcher thread a moment to call spawn at least once.
        std::thread::sleep(Duration::from_millis(50));

        let calls = recorded.lock().unwrap_or_else(|p| p.into_inner());
        assert!(!calls.is_empty(), "spawner should have been called");
        let first = &calls[0];
        assert_eq!(first[0], "ssh");
        // Check -R arg
        let r_pos = first.iter().position(|a| a == "-R").expect("-R not found");
        assert_eq!(
            first[r_pos + 1],
            "/remote/roost.sock:/local/daemon.sock",
            "wrong -R argument"
        );
        assert_eq!(first.last().unwrap(), "user@box1.example.com");
    }

    /// Adding the same origin twice must be a no-op (spawner called once, not twice).
    #[test]
    fn supervisor_add_same_origin_is_noop() {
        let count = Arc::new(AtomicU32::new(0));
        // Use a spawner that blocks forever so we can count add() calls cleanly.
        struct BlockingHandle;
        impl ProcessHandle for BlockingHandle {
            fn wait(&mut self) -> bool {
                std::thread::sleep(Duration::from_secs(60));
                false
            }
        }
        struct BlockingSpawner {
            count: Arc<AtomicU32>,
        }
        impl Spawner for BlockingSpawner {
            fn spawn(&self, _argv: &[String]) -> Result<Box<dyn ProcessHandle>, String> {
                self.count.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(BlockingHandle))
            }
        }

        let sup = Supervisor::new(
            BlockingSpawner { count: Arc::clone(&count) },
            PathBuf::from("/local/daemon.sock"),
        );

        let remote = make_remote("box1", "user@box1.example.com", "/remote/roost.sock");
        sup.add(&remote);
        sup.add(&remote); // duplicate — must be ignored

        // One watcher thread has started; give it time to call spawn once.
        std::thread::sleep(Duration::from_millis(50));

        assert_eq!(count.load(Ordering::SeqCst), 1, "spawner should be called exactly once");
    }

    /// remove() on an unknown origin returns false.
    #[test]
    fn supervisor_remove_unknown_origin_returns_false() {
        let sup = Supervisor::new(FailSpawner, PathBuf::from("/local/daemon.sock"));
        assert!(!sup.remove("nonexistent"));
    }

    /// remove() on a known origin returns true and removes it.
    #[test]
    fn supervisor_remove_known_origin_returns_true() {
        let sup = Supervisor::new(FailSpawner, PathBuf::from("/local/daemon.sock"));
        let remote = make_remote("box2", "user@box2.example.com", "/remote/roost.sock");
        sup.add(&remote);
        assert!(sup.remove("box2"));
        // Second remove: watcher is already gone.
        assert!(!sup.remove("box2"));
    }

    /// state_of returns Some(Reconnecting) immediately after add.
    #[test]
    fn supervisor_state_of_after_add_is_reconnecting_or_connected() {
        let sup = Supervisor::new(FailSpawner, PathBuf::from("/local/daemon.sock"));
        let remote = make_remote("box3", "user@box3.example.com", "/remote/roost.sock");
        sup.add(&remote);
        let st = sup.state_of("box3");
        assert!(
            st == Some(ConnState::Reconnecting) || st == Some(ConnState::Connected),
            "expected Reconnecting or Connected immediately after add, got {st:?}"
        );
    }

    /// state_of returns None for unknown origin.
    #[test]
    fn supervisor_state_of_unknown_origin_is_none() {
        let sup = Supervisor::new(FailSpawner, PathBuf::from("/local/daemon.sock"));
        assert_eq!(sup.state_of("ghost"), None);
    }

    // ── 6. Bootstrap from registry ──────────────────────────────────────────

    /// Supervisor::add called for each entry in a Registry boots all watchers.
    #[test]
    fn supervisor_boots_all_registry_entries() {
        use crate::remotes::Registry;

        let count = Arc::new(AtomicU32::new(0));
        let spawner = CountingSpawner {
            count: Arc::clone(&count),
        };
        let local_sock = PathBuf::from("/local/daemon.sock");
        let sup = Supervisor::new(spawner, local_sock);

        let mut reg = Registry::default();
        reg.upsert(make_remote("alpha", "user@alpha", "/tmp/alpha.sock"));
        reg.upsert(make_remote("beta", "user@beta", "/tmp/beta.sock"));
        reg.upsert(make_remote("gamma", "user@gamma", "/tmp/gamma.sock"));

        for r in reg.all() {
            sup.add(r);
        }

        // Give watcher threads a moment to call spawn.
        std::thread::sleep(Duration::from_millis(100));

        // Each watcher calls spawn at least once.
        let n = count.load(Ordering::SeqCst);
        assert!(n >= 3, "expected at least 3 spawn calls (one per remote), got {n}");
    }

    /// Empty registry: no watchers started.
    #[test]
    fn supervisor_empty_registry_no_watchers() {
        use crate::remotes::Registry;

        let count = Arc::new(AtomicU32::new(0));
        let spawner = CountingSpawner {
            count: Arc::clone(&count),
        };
        let sup = Supervisor::new(spawner, PathBuf::from("/local/daemon.sock"));

        let reg = Registry::default();
        for r in reg.all() {
            sup.add(r);
        }

        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(count.load(Ordering::SeqCst), 0, "no spawns for empty registry");
    }
}
