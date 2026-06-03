use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use roost::protocol::{AgentFamily, EventKind, HookEvent, Request, SessionView};

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn temp_socket_path() -> PathBuf {
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    PathBuf::from(format!("/tmp/roost-test-{pid}-{n}.sock"))
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
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "socket did not become available in time: {}",
        path.display()
    );
}

fn send_request(stream: &mut UnixStream, req: &Request) {
    let json = serde_json::to_string(req).unwrap();
    stream.write_all(json.as_bytes()).unwrap();
    stream.write_all(b"\n").unwrap();
    stream.flush().unwrap();
}

fn recv_line(stream: &mut UnixStream) -> String {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    line.trim().to_string()
}

fn make_event(kind: EventKind, session_id: &str) -> HookEvent {
    HookEvent {
        kind,
        origin: None,
        family: AgentFamily::Claude,
        session_id: session_id.to_string(),
        cwd: "/tmp/test-repo".to_string(),
        pid: Some(9999),
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

#[test]
fn daemon_accepts_report_and_returns_list() {
    let path = temp_socket_path();
    start_daemon(path.clone());
    wait_for_socket(&path, Duration::from_secs(3));

    let mut stream = UnixStream::connect(&path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    // Send SessionStart
    let start_ev = make_event(EventKind::SessionStart, "sess-abc");
    send_request(
        &mut stream,
        &Request::Report {
            event: Box::new(start_ev),
        },
    );

    // Send UserPromptSubmit
    let mut prompt_ev = make_event(EventKind::UserPromptSubmit, "sess-abc");
    prompt_ev.prompt = Some("Fix the bug".to_string());
    send_request(
        &mut stream,
        &Request::Report {
            event: Box::new(prompt_ev),
        },
    );

    // Send List
    send_request(&mut stream, &Request::List);

    let response = recv_line(&mut stream);
    let views: Vec<SessionView> = serde_json::from_str(&response).expect("valid session views");

    assert_eq!(views.len(), 1);
    assert_eq!(views[0].id, "sess-abc");
    assert_eq!(views[0].state, "working");
    assert_eq!(views[0].activity.as_deref(), Some("thinking…"));
}

#[test]
fn daemon_removes_session_on_session_end() {
    let path = temp_socket_path();
    start_daemon(path.clone());
    wait_for_socket(&path, Duration::from_secs(3));

    let mut stream = UnixStream::connect(&path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    // Start session
    let start_ev = make_event(EventKind::SessionStart, "sess-end-test");
    send_request(
        &mut stream,
        &Request::Report {
            event: Box::new(start_ev),
        },
    );

    // End session
    let end_ev = make_event(EventKind::SessionEnd, "sess-end-test");
    send_request(
        &mut stream,
        &Request::Report {
            event: Box::new(end_ev),
        },
    );

    // List should be empty
    send_request(&mut stream, &Request::List);
    let response = recv_line(&mut stream);
    let views: Vec<SessionView> = serde_json::from_str(&response).unwrap();
    assert!(views.is_empty());
}

#[test]
fn daemon_tracks_multiple_sessions() {
    let path = temp_socket_path();
    start_daemon(path.clone());
    wait_for_socket(&path, Duration::from_secs(3));

    let mut stream = UnixStream::connect(&path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    for i in 0..3 {
        let ev = make_event(EventKind::SessionStart, &format!("sess-{i}"));
        send_request(
            &mut stream,
            &Request::Report {
                event: Box::new(ev),
            },
        );
    }

    send_request(&mut stream, &Request::List);
    let response = recv_line(&mut stream);
    let views: Vec<SessionView> = serde_json::from_str(&response).unwrap();
    assert_eq!(views.len(), 3);
}

#[test]
fn daemon_state_changes_correctly() {
    let path = temp_socket_path();
    start_daemon(path.clone());
    wait_for_socket(&path, Duration::from_secs(3));

    let mut stream = UnixStream::connect(&path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    let sess_id = "sess-state-change";

    // SessionStart -> idle
    let ev = make_event(EventKind::SessionStart, sess_id);
    send_request(
        &mut stream,
        &Request::Report {
            event: Box::new(ev),
        },
    );

    // UserPromptSubmit -> working
    let mut ev = make_event(EventKind::UserPromptSubmit, sess_id);
    ev.prompt = Some("test prompt".to_string());
    send_request(
        &mut stream,
        &Request::Report {
            event: Box::new(ev),
        },
    );

    // Notification (approval) -> approval
    let mut ev = make_event(EventKind::Notification, sess_id);
    ev.notification = Some("Permission needed to run: cargo test".to_string());
    send_request(
        &mut stream,
        &Request::Report {
            event: Box::new(ev),
        },
    );

    // Check state is approval
    send_request(&mut stream, &Request::List);
    let response = recv_line(&mut stream);
    let views: Vec<SessionView> = serde_json::from_str(&response).unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].state, "approval");

    // Stop -> done
    let ev = make_event(EventKind::Stop, sess_id);
    send_request(
        &mut stream,
        &Request::Report {
            event: Box::new(ev),
        },
    );

    send_request(&mut stream, &Request::List);
    let response = recv_line(&mut stream);
    let views: Vec<SessionView> = serde_json::from_str(&response).unwrap();
    assert_eq!(views[0].state, "done");
}
