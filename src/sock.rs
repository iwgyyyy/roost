use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// Get the socket path per spec §3:
/// 1. $XDG_RUNTIME_DIR/roost/roost.sock
/// 2. $TMPDIR/roost-<uid>/roost.sock
/// 3. /tmp/roost-<uid>/roost.sock
pub fn socket_path() -> PathBuf {
    let uid = get_uid();

    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("roost").join("roost.sock");
    }

    let tmp_dir = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(tmp_dir)
        .join(format!("roost-{uid}"))
        .join("roost.sock")
}

fn get_uid() -> u32 {
    // Use rustix to get real uid
    rustix::process::getuid().as_raw()
}

/// Check if the daemon is listening on the given socket path.
/// Returns true if connected, false if stale/not present.
pub fn is_listening(path: &std::path::Path) -> bool {
    if !path.exists() {
        return false;
    }
    // Try to connect
    UnixStream::connect(path).is_ok()
}

/// Ensure daemon is running. If not listening, spawn `roost daemon` and wait for it.
pub fn ensure_daemon() -> Result<()> {
    let path = socket_path();

    if is_listening(&path) {
        return Ok(());
    }

    // Get the current executable path to spawn daemon
    let exe = std::env::current_exe().context("cannot find current executable")?;

    // Create socket parent dir if needed
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // If stale socket file exists, remove it
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }

    // Spawn daemon detached
    Command::new(exe)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn daemon")?;

    // Wait for socket to become ready (up to 3 seconds)
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if is_listening(&path) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    anyhow::bail!(
        "daemon did not become ready within 3 seconds (socket: {})",
        path.display()
    )
}

/// Connect to the daemon for hook clients: very short write timeout (200 ms),
/// no read timeout. Designed so a slow/missing daemon never blocks the agent.
pub fn connect_hook(path: &std::path::Path) -> Result<UnixStream> {
    let stream = UnixStream::connect(path)
        .with_context(|| format!("cannot connect to roost daemon at {}", path.display()))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(200)))
        .ok();
    // No read timeout — hook clients never read responses.
    Ok(stream)
}

/// Connect to daemon with a short timeout. Returns Err if unable to connect.
pub fn connect_with_timeout(path: &std::path::Path, timeout: Duration) -> Result<UnixStream> {
    let deadline = Instant::now() + timeout;
    loop {
        match UnixStream::connect(path) {
            Ok(stream) => {
                stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
                stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
                return Ok(stream);
            }
            Err(e) => {
                if Instant::now() >= deadline {
                    return Err(e).context(format!(
                        "cannot connect to roost daemon at {}",
                        path.display()
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

/// Send a JSON line to a stream and optionally read a response line.
pub fn send_line(stream: &mut UnixStream, line: &str) -> Result<()> {
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

pub fn recv_line(stream: &mut UnixStream) -> Result<String> {
    let mut buf: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                if byte[0] == b'\n' {
                    break;
                }
                buf.push(byte[0]);
            }
            Err(e) => return Err(e.into()),
        }
    }
    String::from_utf8(buf).map_err(|e| anyhow::anyhow!("invalid UTF-8 in recv_line: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A process-wide mutex to serialize tests that mutate environment variables.
    /// Without this, concurrent test threads that call set_var / remove_var race each other.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn socket_path_with_xdg_runtime_dir() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        // Temporarily set XDG_RUNTIME_DIR.
        // SAFETY: ENV_MUTEX serializes all env-var tests within this module.
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        }
        let path = socket_path();
        unsafe {
            std::env::remove_var("XDG_RUNTIME_DIR");
        }

        assert_eq!(path, PathBuf::from("/run/user/1000/roost/roost.sock"));
    }

    #[test]
    fn socket_path_fallback_with_tmpdir() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            std::env::remove_var("XDG_RUNTIME_DIR");
            std::env::set_var("TMPDIR", "/var/folders/test");
        }
        let path = socket_path();
        unsafe {
            std::env::remove_var("TMPDIR");
        }

        let uid = rustix::process::getuid().as_raw();
        let expected_dir = format!("/var/folders/test/roost-{uid}");
        assert!(
            path.starts_with(&expected_dir),
            "path {path:?} should start with {expected_dir}"
        );
        assert!(path.ends_with("roost.sock"));
    }

    #[test]
    fn socket_path_fallback_to_tmp_when_no_tmpdir() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            std::env::remove_var("XDG_RUNTIME_DIR");
            std::env::remove_var("TMPDIR");
        }
        let path = socket_path();

        let uid = rustix::process::getuid().as_raw();
        let expected_dir = format!("/tmp/roost-{uid}");
        assert!(
            path.starts_with(&expected_dir),
            "path {path:?} should start with {expected_dir}"
        );
        assert!(path.ends_with("roost.sock"));
    }

    #[test]
    fn socket_path_contains_user_namespace() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            std::env::remove_var("XDG_RUNTIME_DIR");
        }
        let path = socket_path();
        let uid = rustix::process::getuid().as_raw();
        let path_str = path.to_string_lossy();
        assert!(
            path_str.contains(&uid.to_string()),
            "path {path_str} should contain uid {uid}"
        );
    }

    #[test]
    fn is_listening_returns_false_for_nonexistent() {
        let path = PathBuf::from("/tmp/roost-test-nonexistent-9999.sock");
        assert!(!is_listening(&path));
    }

    /// Test that recv_line correctly handles multi-byte UTF-8 (CJK characters).
    /// Sends a JSON line containing "思考中…" over a real Unix socket pair and
    /// verifies round-trip decoding is lossless.
    #[test]
    fn recv_line_handles_multibyte_utf8() {
        use std::os::unix::net::UnixListener;

        let pid = std::process::id();
        let sock_path = PathBuf::from(format!("/tmp/roost-test-utf8-{pid}.sock"));
        // Clean up any leftover socket from a previous run
        let _ = std::fs::remove_file(&sock_path);

        let listener = UnixListener::bind(&sock_path).expect("bind test socket");

        // Spawn a thread that accepts one connection and writes a CJK JSON line.
        let sock_path_clone = sock_path.clone();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().expect("accept");
            // Write a JSON payload that includes CJK activity text
            let payload = r#"{"activity":"思考中…","state":"working"}"#;
            conn.write_all(payload.as_bytes()).unwrap();
            conn.write_all(b"\n").unwrap();
            conn.flush().unwrap();
            drop(sock_path_clone);
        });

        let mut client = UnixStream::connect(&sock_path).expect("connect to test socket");
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();

        let line = recv_line(&mut client).expect("recv_line should succeed");

        // Verify the CJK text survives the round trip
        assert!(
            line.contains("思考中…"),
            "expected CJK text '思考中…' in received line, got: {line:?}"
        );

        // Verify it's valid JSON with the expected field
        let parsed: serde_json::Value = serde_json::from_str(&line).expect("should be valid JSON");
        assert_eq!(
            parsed.get("activity").and_then(|v| v.as_str()),
            Some("思考中…"),
            "activity field should contain '思考中…'"
        );

        server.join().unwrap();
        let _ = std::fs::remove_file(&sock_path);
    }
}
