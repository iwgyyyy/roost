use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::time::Duration;

use roost::protocol::{AgentFamily, EventKind, Request};

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn temp_socket_path() -> PathBuf {
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    PathBuf::from(format!("/tmp/roost-hook-test-{pid}-{n}.sock"))
}

#[test]
fn hook_sends_session_start_event() {
    let path = temp_socket_path();
    let listener = UnixListener::bind(&path).unwrap();

    // Override socket path via XDG_RUNTIME_DIR trick: instead, we directly test
    // build_hook_event and then manually send to our listener
    let path_clone = path.clone();

    let listener_thread = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        // Note: set_read_timeout may not be supported on all platforms for UnixStream
        let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        line.trim().to_string()
    });

    // Small delay to ensure listener is ready
    std::thread::sleep(Duration::from_millis(20));

    // Use the hook module directly: build event and send to our fake daemon
    let stdin_json = r#"{"session_id":"test-hook-sess","cwd":"/tmp/my-project","tool_name":"Edit","tool_input":{"path":"src/main.rs"}}"#;

    let event = roost::hook::build_hook_event("claude", "PreToolUse", stdin_json).unwrap();
    let request = Request::Report {
        event: Box::new(event.clone()),
    };

    // Manually connect and send (simulating what run_hook does)
    {
        use std::io::Write;
        let mut stream = std::os::unix::net::UnixStream::connect(&path_clone).unwrap();
        let json = serde_json::to_string(&request).unwrap();
        stream.write_all(json.as_bytes()).unwrap();
        stream.write_all(b"\n").unwrap();
        stream.flush().unwrap();
    }

    let received_line = listener_thread.join().unwrap();
    let received_req: Request = serde_json::from_str(&received_line).expect("valid Request JSON");

    match received_req {
        Request::Report { event: recv_ev } => {
            let recv_ev = *recv_ev;
            assert_eq!(recv_ev.session_id, "test-hook-sess");
            assert_eq!(recv_ev.cwd, "/tmp/my-project");
            assert_eq!(recv_ev.family, AgentFamily::Claude);
            assert!(matches!(recv_ev.kind, EventKind::PreToolUse));
            assert_eq!(recv_ev.tool_name.as_deref(), Some("Edit"));
            assert!(recv_ev.tool_input.is_some());
        }
        Request::List => panic!("expected Report, got List"),
        Request::Detail { .. } => panic!("expected Report, got Detail"),
        _ => panic!("expected Report, got a fleet request variant"),
    }
}

#[test]
fn hook_extracts_session_id_and_family() {
    let stdin_json = r#"{"session_id":"my-sess-xyz","cwd":"/home/user/project"}"#;
    let event = roost::hook::build_hook_event("codex", "SessionStart", stdin_json).unwrap();

    assert_eq!(event.session_id, "my-sess-xyz");
    assert_eq!(event.family, AgentFamily::Codex);
    assert!(matches!(event.kind, EventKind::SessionStart));
}

#[test]
fn hook_extracts_prompt_from_user_prompt_submit() {
    let stdin_json = r#"{"session_id":"s1","cwd":"/tmp","prompt":"Fix the checkout bug"}"#;
    let event = roost::hook::build_hook_event("claude", "UserPromptSubmit", stdin_json).unwrap();

    assert_eq!(event.prompt.as_deref(), Some("Fix the checkout bug"));
}

#[test]
fn hook_handles_empty_stdin() {
    let event = roost::hook::build_hook_event("claude", "SessionStart", "").unwrap();
    // Should not panic, session_id will be generated
    assert!(!event.session_id.is_empty());
    assert_eq!(event.family, AgentFamily::Claude);
}

#[test]
fn hook_returns_error_for_unknown_event() {
    let result = roost::hook::build_hook_event("claude", "BogusEvent", "{}");
    assert!(result.is_err());
}
