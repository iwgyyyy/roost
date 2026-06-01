//! Periodic daemon connection for the TUI.
//!
//! Connects to the daemon, sends `Request::list`, receives `Vec<SessionView>`.
//! On failure (daemon not ready, socket error, parse error) returns an empty
//! list and sets `daemon_ready = false`. The TUI renders "waiting for daemon"
//! in that case — it never crashes.

use std::time::Duration;

use crate::protocol::{Request, SessionDetail, SessionView};
use crate::sock;

/// Attempt to query the daemon for the current session list.
///
/// Returns `(sessions, daemon_ready)`.
/// `daemon_ready` is `false` when the daemon is unreachable or returned an error.
pub fn fetch_sessions() -> (Vec<SessionView>, bool) {
    let path = sock::socket_path();
    if !sock::is_listening(&path) {
        return (vec![], false);
    }

    let mut stream = match sock::connect_with_timeout(&path, Duration::from_millis(300)) {
        Ok(s) => s,
        Err(_) => return (vec![], false),
    };

    let request = Request::List;
    let json = match serde_json::to_string(&request) {
        Ok(j) => j,
        Err(_) => return (vec![], false),
    };

    if sock::send_line(&mut stream, &json).is_err() {
        return (vec![], false);
    }

    let line = match sock::recv_line(&mut stream) {
        Ok(l) => l,
        Err(_) => return (vec![], false),
    };

    if line.is_empty() {
        return (vec![], true);
    }

    match serde_json::from_str::<Vec<SessionView>>(&line) {
        Ok(sessions) => (sessions, true),
        Err(_) => (vec![], false),
    }
}

/// Fetch the `SessionDetail` for a specific session from the daemon.
///
/// Returns `None` if the daemon is unreachable, the session does not exist,
/// or any I/O / parse error occurs.
pub fn fetch_detail(session_id: &str) -> Option<SessionDetail> {
    let path = sock::socket_path();
    if !sock::is_listening(&path) {
        return None;
    }

    let mut stream = sock::connect_with_timeout(&path, Duration::from_millis(300)).ok()?;

    let request = Request::Detail {
        session_id: session_id.to_string(),
    };
    let json = serde_json::to_string(&request).ok()?;
    sock::send_line(&mut stream, &json).ok()?;

    let line = sock::recv_line(&mut stream).ok()?;
    if line.is_empty() || line == "null" {
        return None;
    }

    serde_json::from_str::<SessionDetail>(&line).ok()
}
