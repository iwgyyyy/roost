/// Integration tests for daemon PID liveness probing and session removal.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use roost::protocol::{AgentFamily, EventKind, HookEvent, Request, SessionView};

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn temp_socket_path() -> PathBuf {
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    PathBuf::from(format!("/tmp/roost-liveness-test-{pid}-{n}.sock"))
}

/// Point ROOST_HOME at a throwaway temp dir so the in-process daemon's history
/// DB never touches the real `~/.roost`. Set exactly once: `call_once` serializes
/// every test's `start_daemon`, so the env var is written before any daemon
/// thread reads it (no data race) and reused by all daemons in this test binary.
fn ensure_isolated_home() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let dir = std::env::temp_dir().join(format!("roost-test-home-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // SAFETY: set once, before any daemon thread exists (see fn doc).
        unsafe { std::env::set_var("ROOST_HOME", &dir) };
    });
}

fn start_daemon(path: PathBuf) -> std::thread::JoinHandle<()> {
    ensure_isolated_home();
    let path_clone = path.clone();
    std::thread::spawn(move || {
        roost::daemon::run_daemon_at(path_clone).unwrap();
    })
}

fn wait_for_socket(path: &PathBuf, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if path.exists() && UnixStream::connect(path).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("socket not ready: {}", path.display());
}

fn send_request(stream: &mut UnixStream, req: &Request) {
    let json = serde_json::to_string(req).unwrap();
    stream.write_all(json.as_bytes()).unwrap();
    stream.write_all(b"\n").unwrap();
    stream.flush().unwrap();
}

fn recv_sessions(stream: &mut UnixStream) -> Vec<SessionView> {
    send_request(stream, &Request::List);
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(line.trim()).unwrap_or_default()
}

fn make_event(kind: EventKind, session_id: &str, pid: Option<u32>) -> HookEvent {
    HookEvent {
        kind,
        origin: None,
        family: AgentFamily::Claude,
        session_id: session_id.to_string(),
        cwd: "/tmp/test-project".to_string(),
        pid,
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
    }
}

/// A session with a non-existent PID should be removed after a few liveness cycles.
#[test]
fn dead_pid_session_removed_by_liveness() {
    let path = temp_socket_path();
    start_daemon(path.clone());
    wait_for_socket(&path, Duration::from_secs(3));

    let mut stream = UnixStream::connect(&path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();

    // PID 99999999 is very unlikely to exist.
    let nonexistent_pid: u32 = 99_999_999;
    let ev = make_event(
        EventKind::SessionStart,
        "dead-pid-session",
        Some(nonexistent_pid),
    );
    send_request(
        &mut stream,
        &Request::Report {
            event: Box::new(ev),
        },
    );

    // Verify session was added.
    let views = recv_sessions(&mut stream);
    assert_eq!(views.len(), 1, "session should exist initially");

    // Wait long enough for DEAD_CHECK_THRESHOLD (3) liveness cycles at 2s each.
    // Give an extra buffer.
    std::thread::sleep(Duration::from_secs(9));

    let views = recv_sessions(&mut stream);
    assert!(
        views.is_empty(),
        "session with dead pid should have been removed by liveness; still {} session(s)",
        views.len()
    );
}

/// A session with the current test process's PID should NOT be removed.
#[test]
fn alive_pid_session_not_removed_by_liveness() {
    let path = temp_socket_path();
    start_daemon(path.clone());
    wait_for_socket(&path, Duration::from_secs(3));

    let mut stream = UnixStream::connect(&path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();

    let self_pid = std::process::id();
    let ev = make_event(EventKind::SessionStart, "alive-pid-session", Some(self_pid));
    send_request(
        &mut stream,
        &Request::Report {
            event: Box::new(ev),
        },
    );

    // Wait for ~3 liveness cycles.
    std::thread::sleep(Duration::from_secs(7));

    let views = recv_sessions(&mut stream);
    assert_eq!(
        views.len(),
        1,
        "session with alive pid should NOT be removed; got {} session(s)",
        views.len()
    );
}
