use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use roost::protocol::{AgentFamily, EventKind, HookEvent, Request};

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn temp_socket_path() -> PathBuf {
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    PathBuf::from(format!("/tmp/roost-list-test-{pid}-{n}.sock"))
}

fn start_daemon(path: PathBuf) {
    std::thread::spawn(move || {
        roost::daemon::run_daemon_at(path).unwrap();
    });
}

fn wait_for_socket(path: &PathBuf, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if path.exists() && UnixStream::connect(path).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("socket not ready: {}", path.display());
}

fn make_event(kind: EventKind, session_id: &str, cwd: &str) -> HookEvent {
    HookEvent {
        kind,
        family: AgentFamily::Claude,
        session_id: session_id.to_string(),
        cwd: cwd.to_string(),
        pid: Some(1234),
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

fn send_report(stream: &mut UnixStream, ev: HookEvent) {
    let req = Request::Report {
        event: Box::new(ev),
    };
    let json = serde_json::to_string(&req).unwrap();
    stream.write_all(json.as_bytes()).unwrap();
    stream.write_all(b"\n").unwrap();
    stream.flush().unwrap();
}

#[test]
fn list_returns_two_sessions() {
    let path = temp_socket_path();
    start_daemon(path.clone());
    wait_for_socket(&path, Duration::from_secs(3));

    // Insert two sessions via a connection
    {
        let mut stream = UnixStream::connect(&path).unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        send_report(
            &mut stream,
            make_event(EventKind::SessionStart, "list-s1", "/tmp/repo-a"),
        );
        send_report(
            &mut stream,
            make_event(EventKind::UserPromptSubmit, "list-s1", "/tmp/repo-a"),
        );
        send_report(
            &mut stream,
            make_event(EventKind::SessionStart, "list-s2", "/tmp/repo-b"),
        );
    }

    // Brief pause to ensure daemon processes messages
    std::thread::sleep(Duration::from_millis(200));

    // Query via list module
    // Temporarily override socket path by using query_daemon_at (we'll call directly)
    let views = query_at(&path);
    assert_eq!(views.len(), 2, "expected 2 sessions, got: {:?}", views);

    let ids: Vec<&str> = views.iter().map(|v| v.id.as_str()).collect();
    assert!(ids.contains(&"list-s1"));
    assert!(ids.contains(&"list-s2"));

    // Check state of s1 (had UserPromptSubmit → working)
    let s1 = views.iter().find(|v| v.id == "list-s1").unwrap();
    assert_eq!(s1.state, "working");
    assert_eq!(s1.activity.as_deref(), Some("thinking…"));
}

#[test]
fn list_shows_correct_fields() {
    let path = temp_socket_path();
    start_daemon(path.clone());
    wait_for_socket(&path, Duration::from_secs(3));

    {
        let mut stream = UnixStream::connect(&path).unwrap();
        let mut ev = make_event(EventKind::SessionStart, "field-test", "/tmp/my-repo");
        ev.family = AgentFamily::Codex;
        send_report(&mut stream, ev);

        // Codex uses PermissionRequest (not Notification) to signal approval state.
        let mut ev2 = make_event(EventKind::PermissionRequest, "field-test", "/tmp/my-repo");
        ev2.family = AgentFamily::Codex;
        ev2.notification = Some("run: cargo build".to_string());
        send_report(&mut stream, ev2);
    }

    std::thread::sleep(Duration::from_millis(200));

    let views = query_at(&path);
    assert_eq!(views.len(), 1);
    let v = &views[0];
    assert_eq!(v.id, "field-test");
    assert_eq!(v.state, "approval");
}

/// Helper: query a specific daemon socket path for session views.
fn query_at(path: &PathBuf) -> Vec<roost::protocol::SessionView> {
    use std::io::BufRead;
    let mut stream = UnixStream::connect(path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    let req = Request::List;
    let json = serde_json::to_string(&req).unwrap();
    stream.write_all(json.as_bytes()).unwrap();
    stream.write_all(b"\n").unwrap();
    stream.flush().unwrap();

    let mut reader = std::io::BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(line.trim()).unwrap()
}
