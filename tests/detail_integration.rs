use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use roost::protocol::{AgentFamily, EventKind, HookEvent, Request, SessionDetail};

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn temp_socket_path() -> PathBuf {
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    PathBuf::from(format!("/tmp/roost-detail-test-{pid}-{n}.sock"))
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
        family: AgentFamily::Claude,
        session_id: session_id.to_string(),
        cwd: "/home/user/payments-api".to_string(),
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

/// Ingest events to the daemon via `Request::Report` and return a fresh stream for queries.
fn ingest_events(path: &PathBuf, events: &[HookEvent]) -> UnixStream {
    let mut stream = UnixStream::connect(path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    for ev in events {
        let req = Request::Report {
            event: Box::new(ev.clone()),
        };
        send_request(&mut stream, &req);
    }
    stream
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn detail_returns_full_path_and_state() {
    let path = temp_socket_path();
    start_daemon(path.clone());
    wait_for_socket(&path, Duration::from_secs(3));

    let mut ev_start = make_event(EventKind::SessionStart, "detail-sess-1");
    ev_start.cwd = "/home/user/payments-api".to_string();

    let mut ev_prompt = make_event(EventKind::UserPromptSubmit, "detail-sess-1");
    ev_prompt.prompt = Some("Fix the bug".to_string());

    let mut stream = ingest_events(&path, &[ev_start, ev_prompt]);

    // Query detail
    send_request(
        &mut stream,
        &Request::Detail {
            session_id: "detail-sess-1".to_string(),
        },
    );

    let response = recv_line(&mut stream);
    assert_ne!(response, "null", "detail should exist for session");

    let detail: SessionDetail = serde_json::from_str(&response).expect("valid SessionDetail JSON");

    assert_eq!(detail.id, "detail-sess-1");
    assert_eq!(detail.cwd, "/home/user/payments-api");
    assert_eq!(detail.state, "working");
}

#[test]
fn detail_unknown_session_returns_null() {
    let path = temp_socket_path();
    start_daemon(path.clone());
    wait_for_socket(&path, Duration::from_secs(3));

    let mut stream = UnixStream::connect(&path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    send_request(
        &mut stream,
        &Request::Detail {
            session_id: "nonexistent-session".to_string(),
        },
    );

    let response = recv_line(&mut stream);
    assert_eq!(response, "null", "unknown session should return null");
}

#[test]
fn detail_events_are_in_reverse_chronological_order() {
    let path = temp_socket_path();
    start_daemon(path.clone());
    wait_for_socket(&path, Duration::from_secs(3));

    let sess_id = "detail-events-order";
    let ev_start = make_event(EventKind::SessionStart, sess_id);
    let mut ev_edit = make_event(EventKind::PreToolUse, sess_id);
    ev_edit.tool_name = Some("Edit".to_string());
    ev_edit.tool_input = Some(r#"{"path":"src/lib.rs"}"#.to_string());
    let ev_stop = make_event(EventKind::Stop, sess_id);

    let mut stream = ingest_events(&path, &[ev_start, ev_edit, ev_stop]);

    send_request(
        &mut stream,
        &Request::Detail {
            session_id: sess_id.to_string(),
        },
    );

    let response = recv_line(&mut stream);
    let detail: SessionDetail = serde_json::from_str(&response).unwrap();

    // recent_events should be non-empty
    assert!(
        !detail.recent_events.is_empty(),
        "recent_events should not be empty"
    );

    // Should be in reverse-chronological order: newest (ts_secs smallest) comes first
    let ts: Vec<u64> = detail.recent_events.iter().map(|e| e.ts_secs).collect();
    let mut sorted = ts.clone();
    sorted.sort();
    assert_eq!(
        ts, sorted,
        "recent_events should be in ascending ts_secs order (newest = 0s at top)"
    );

    // The last event added (Stop) should appear first (ts_secs ~= 0)
    let first_summary = &detail.recent_events[0].summary;
    assert_eq!(
        first_summary, "✓ done",
        "first (most recent) event should be Stop summary, got: {first_summary:?}"
    );
}

#[test]
fn detail_events_capped_at_max() {
    use roost::session::MAX_EVENTS;

    let path = temp_socket_path();
    start_daemon(path.clone());
    wait_for_socket(&path, Duration::from_secs(3));

    let sess_id = "detail-cap-test";

    let mut stream = UnixStream::connect(&path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    // Ingest more than MAX_EVENTS
    let start_ev = make_event(EventKind::SessionStart, sess_id);
    send_request(
        &mut stream,
        &Request::Report {
            event: Box::new(start_ev),
        },
    );

    for _ in 0..(MAX_EVENTS + 10) {
        let ev = make_event(EventKind::Stop, sess_id);
        send_request(
            &mut stream,
            &Request::Report {
                event: Box::new(ev),
            },
        );
    }

    send_request(
        &mut stream,
        &Request::Detail {
            session_id: sess_id.to_string(),
        },
    );

    let response = recv_line(&mut stream);
    let detail: SessionDetail = serde_json::from_str(&response).unwrap();

    assert_eq!(
        detail.recent_events.len(),
        MAX_EVENTS,
        "recent_events should be capped at MAX_EVENTS={MAX_EVENTS}, got {}",
        detail.recent_events.len()
    );
}

#[test]
fn detail_contains_action_when_working() {
    let path = temp_socket_path();
    start_daemon(path.clone());
    wait_for_socket(&path, Duration::from_secs(3));

    let sess_id = "detail-action-test";
    let ev_start = make_event(EventKind::SessionStart, sess_id);
    let mut ev_edit = make_event(EventKind::PreToolUse, sess_id);
    ev_edit.tool_name = Some("Edit".to_string());
    ev_edit.tool_input = Some(r#"{"path":"src/main.rs"}"#.to_string());

    let mut stream = ingest_events(&path, &[ev_start, ev_edit]);

    send_request(
        &mut stream,
        &Request::Detail {
            session_id: sess_id.to_string(),
        },
    );

    let response = recv_line(&mut stream);
    let detail: SessionDetail = serde_json::from_str(&response).unwrap();

    assert_eq!(detail.state, "working");
    assert!(
        detail.action.is_some(),
        "action should be Some when working"
    );
    let action = detail.action.unwrap();
    assert!(
        action.contains("src/main.rs"),
        "action should mention the path, got: {action:?}"
    );
}
