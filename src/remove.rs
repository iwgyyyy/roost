//! `roost remove <ssh-target-or-origin> [--purge]` — de-provision a remote dev box.
//!
//! ## Safety contract
//! **No code in this file executes `ssh` or touches real sockets during tests.**
//! All SSH interactions are abstracted behind the [`crate::add::SshRunner`] trait
//! (re-used from the `add` module).  Production code uses `StdSshRunner`;
//! tests inject a `MockSshRunner`.
//!
//! ## Steps performed by [`run_remove`]
//! 1. Locate the registry entry matching `key` (by target first, then by origin).
//! 2. Notify the daemon to stop the tunnel watcher (`Request::RemoveRemote`).
//! 3. Run `ssh <target> 'roost setup --uninstall'` (unarmed: best-effort).
//!    If the remote is unreachable, warn and continue (local cleanup still happens).
//! 4. If `--purge`: run `ssh <target> 'rm -f $(command -v roost)'` to delete
//!    the remote binary (also best-effort).
//! 5. Remove the entry from `remotes.json`.

use std::path::Path;

use anyhow::{Context, Result};

use crate::add::SshRunner;
use crate::remotes::Remote;

// ── SSH command helpers ────────────────────────────────────────────────────────

/// Remote command: uninstall roost hooks on the remote.
pub fn cmd_uninstall_remote() -> &'static str {
    "roost setup --uninstall"
}

/// Remote command: delete the roost binary on the remote (purge mode only).
pub fn cmd_purge_remote() -> &'static str {
    "rm -f $(command -v roost 2>/dev/null)"
}

// ── Options ───────────────────────────────────────────────────────────────────

/// Options for [`run_remove`].
pub struct RemoveOptions<'a> {
    /// SSH target (`user@host`) **or** origin label.
    /// We look up by target first, then by origin.
    pub key: &'a str,
    /// Path to the remotes registry file (tempfile in tests).
    pub registry_path: &'a Path,
    /// Path to the local daemon socket used to notify of the removal.
    pub daemon_sock: &'a Path,
    /// If true, also delete the roost binary from the remote machine.
    pub purge: bool,
}

// ── Orchestration ─────────────────────────────────────────────────────────────

/// Execute the `roost remove` workflow.
///
/// Returns `Ok(())` on success (including the "remote unreachable" degraded path).
/// Returns `Err` only on hard local failures (registry corrupt, I/O error).
///
/// Tests supply a mock runner; production supplies `StdSshRunner`.
pub fn run_remove<R: SshRunner>(runner: &R, opts: RemoveOptions<'_>) -> Result<()> {
    // ── Step 1: locate registry entry ────────────────────────────────────────
    let mut registry = crate::remotes::load_from(opts.registry_path);

    // Try target first (exact match), then origin.
    let remote: Remote = registry
        .find_by_target(opts.key)
        .or_else(|| registry.find_by_origin(opts.key))
        .cloned()
        .with_context(|| {
            format!(
                "no registered remote matching {:?} (not a known target or origin).\n\
                 Run `roost remotes` to see registered remotes.",
                opts.key
            )
        })?;

    println!("Removing remote '{}' ({}) …", remote.origin, remote.target);

    // ── Step 2: notify daemon to stop the tunnel watcher ─────────────────────
    notify_daemon_remove(opts.daemon_sock, &remote.origin);

    // ── Step 3: remote cleanup (best-effort) ─────────────────────────────────
    let remote_reachable = try_remote_cleanup(runner, &remote.target, opts.purge);
    if !remote_reachable {
        eprintln!(
            "Warning: remote '{}' ({}) was unreachable — local state cleaned up, \
             but remote hooks/binary may still be present.",
            remote.origin, remote.target
        );
    }

    // ── Step 4: remove from registry ─────────────────────────────────────────
    // Remove by origin (canonical key).  We already confirmed it exists above.
    registry.remove_by_origin(&remote.origin);
    crate::remotes::save_to(opts.registry_path, &registry)?;

    println!("Done. Remote '{}' has been de-registered.", remote.origin);
    Ok(())
}

/// Send `Request::RemoveRemote` to the daemon socket (best-effort).
/// If the daemon is not running, silently skips.
fn notify_daemon_remove(sock: &Path, origin: &str) {
    use crate::protocol::Request;
    use crate::sock;

    let mut stream = match sock::connect_hook(sock) {
        Ok(s) => s,
        Err(_) => return, // daemon not running — fine
    };

    let req = Request::RemoveRemote { origin: origin.to_string() };
    if let Ok(json) = serde_json::to_string(&req) {
        let _ = sock::send_line(&mut stream, &json);
    }
}

/// Run the remote uninstall (and optionally purge) steps.
///
/// Returns `true` if the remote was reachable, `false` if SSH failed.
/// Either way, errors are non-fatal (caller warns but continues).
fn try_remote_cleanup<R: SshRunner>(runner: &R, target: &str, purge: bool) -> bool {
    // Step 3a: uninstall hooks
    match runner.run(target, cmd_uninstall_remote()) {
        Ok(out) if out.success => {
            println!("    ✓ Removed remote hooks from {target}");
        }
        Ok(_) => {
            // Command ran but returned non-zero — hooks may already be gone, warn.
            eprintln!("    ! Could not remove hooks on {target} (command failed, may already be absent)");
        }
        Err(_) => {
            // SSH itself failed — remote unreachable.
            return false;
        }
    }

    // Step 3b (purge only): delete remote binary
    if purge {
        match runner.run(target, cmd_purge_remote()) {
            Ok(out) if out.success => {
                println!("    ✓ Deleted roost binary on {target}");
            }
            Ok(_) => {
                eprintln!("    ! Could not delete roost binary on {target}");
            }
            Err(_) => {
                eprintln!("    ! SSH failed while purging binary on {target}; binary may remain.");
            }
        }
    }

    true
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use tempfile::tempdir;

    use crate::add::{RunOutput, SshRunner};
    use crate::remotes::{Registry, Remote};

    // ── Mock SSH runner ────────────────────────────────────────────────────────

    #[derive(Debug, Clone)]
    struct MockResponse {
        stdout: String,
        success: bool,
    }

    impl MockResponse {
        fn ok() -> Self { Self { stdout: String::new(), success: true } }
        fn fail() -> Self { Self { stdout: String::new(), success: false } }
    }

    struct MockSshRunner {
        responses: Mutex<VecDeque<MockResponse>>,
        calls: Mutex<Vec<(String, String)>>,
        /// If true, `run()` returns Err (simulates SSH-level failure / host unreachable).
        force_error: bool,
    }

    impl MockSshRunner {
        fn new(responses: impl IntoIterator<Item = MockResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
                force_error: false,
            }
        }

        fn unreachable() -> Self {
            Self {
                responses: Mutex::new(VecDeque::new()),
                calls: Mutex::new(Vec::new()),
                force_error: true,
            }
        }

        fn calls(&self) -> Vec<(String, String)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl SshRunner for MockSshRunner {
        fn run(&self, target: &str, command: &str) -> anyhow::Result<RunOutput> {
            self.calls
                .lock()
                .unwrap()
                .push((target.to_string(), command.to_string()));

            if self.force_error {
                anyhow::bail!("mock: host unreachable");
            }

            let resp = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("MockSshRunner: no more pre-programmed responses");

            Ok(RunOutput { stdout: resp.stdout, success: resp.success })
        }
    }

    // ── Helper: populate a registry file ─────────────────────────────────────

    fn seed_registry(path: &Path, entries: &[(&str, &str)]) {
        let mut reg = Registry::default();
        for (origin, target) in entries {
            reg.upsert(Remote {
                origin: origin.to_string(),
                target: target.to_string(),
                remote_sock: "~/.roost/hub.sock".to_string(),
                added_at: "2026-06-03T00:00:00Z".to_string(),
                arch: "linux-x86_64".to_string(),
            });
        }
        crate::remotes::save_to(path, &reg).unwrap();
    }

    fn make_opts<'a>(
        key: &'a str,
        registry_path: &'a Path,
        daemon_sock: &'a Path,
        purge: bool,
    ) -> RemoveOptions<'a> {
        RemoveOptions { key, registry_path, daemon_sock, purge }
    }

    // ════════════════════════════════════════════════════════════════════════
    // 1. Happy-path: remove by target
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn remove_by_target_removes_entry_from_registry() {
        let dir = tempdir().unwrap();
        let reg_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        seed_registry(&reg_path, &[("devbox1", "alice@devbox1")]);

        let runner = MockSshRunner::new([MockResponse::ok()]); // uninstall cmd

        run_remove(&runner, make_opts("alice@devbox1", &reg_path, &daemon_sock, false)).unwrap();

        let reg = crate::remotes::load_from(&reg_path);
        assert!(reg.all().is_empty(), "entry must be removed from registry");
    }

    // ════════════════════════════════════════════════════════════════════════
    // 2. Happy-path: remove by origin
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn remove_by_origin_removes_entry_from_registry() {
        let dir = tempdir().unwrap();
        let reg_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        seed_registry(&reg_path, &[("devbox1", "alice@devbox1")]);

        let runner = MockSshRunner::new([MockResponse::ok()]);

        run_remove(&runner, make_opts("devbox1", &reg_path, &daemon_sock, false)).unwrap();

        let reg = crate::remotes::load_from(&reg_path);
        assert!(reg.all().is_empty(), "entry must be removed from registry");
    }

    // ════════════════════════════════════════════════════════════════════════
    // 3. Multiple remotes — only the matched one is removed
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn remove_only_removes_matched_entry() {
        let dir = tempdir().unwrap();
        let reg_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        seed_registry(
            &reg_path,
            &[("box1", "alice@box1"), ("box2", "alice@box2")],
        );

        let runner = MockSshRunner::new([MockResponse::ok()]);

        run_remove(&runner, make_opts("box1", &reg_path, &daemon_sock, false)).unwrap();

        let reg = crate::remotes::load_from(&reg_path);
        assert_eq!(reg.all().len(), 1, "only one entry should remain");
        assert_eq!(reg.all()[0].origin, "box2", "box2 must still be present");
    }

    // ════════════════════════════════════════════════════════════════════════
    // 4. SSH commands are issued in the right order (no purge)
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn remove_without_purge_calls_only_uninstall() {
        let dir = tempdir().unwrap();
        let reg_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        seed_registry(&reg_path, &[("devbox1", "alice@devbox1")]);

        let runner = MockSshRunner::new([MockResponse::ok()]);

        run_remove(&runner, make_opts("devbox1", &reg_path, &daemon_sock, false)).unwrap();

        let calls = runner.calls();
        assert_eq!(calls.len(), 1, "exactly 1 SSH call without --purge: {calls:?}");
        assert!(
            calls[0].1.contains("setup --uninstall"),
            "must call roost setup --uninstall: {:?}",
            calls[0].1
        );
    }

    // ════════════════════════════════════════════════════════════════════════
    // 5. --purge adds a second SSH call to delete the binary
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn remove_with_purge_calls_uninstall_then_delete_binary() {
        let dir = tempdir().unwrap();
        let reg_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        seed_registry(&reg_path, &[("devbox1", "alice@devbox1")]);

        let runner = MockSshRunner::new([
            MockResponse::ok(), // uninstall
            MockResponse::ok(), // purge / rm
        ]);

        run_remove(&runner, make_opts("devbox1", &reg_path, &daemon_sock, true)).unwrap();

        let calls = runner.calls();
        assert_eq!(calls.len(), 2, "2 SSH calls with --purge: {calls:?}");
        assert!(calls[0].1.contains("setup --uninstall"), "1st call must be uninstall");
        assert!(calls[1].1.contains("rm"), "2nd call must be rm (purge)");
    }

    // ════════════════════════════════════════════════════════════════════════
    // 6. Remote unreachable → local cleanup still happens (degraded mode)
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn remove_when_remote_unreachable_still_removes_from_registry() {
        let dir = tempdir().unwrap();
        let reg_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        seed_registry(&reg_path, &[("devbox1", "alice@devbox1")]);

        // SSH-level failure for all calls.
        let runner = MockSshRunner::unreachable();

        // Must succeed (degraded path, not an error).
        run_remove(&runner, make_opts("devbox1", &reg_path, &daemon_sock, false)).unwrap();

        // Registry entry is still removed.
        let reg = crate::remotes::load_from(&reg_path);
        assert!(reg.all().is_empty(), "entry must be removed even if remote is unreachable");
    }

    // ════════════════════════════════════════════════════════════════════════
    // 7. Key not found → Err
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn remove_unknown_key_returns_error() {
        let dir = tempdir().unwrap();
        let reg_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        seed_registry(&reg_path, &[("devbox1", "alice@devbox1")]);

        let runner = MockSshRunner::new([]);

        let result = run_remove(
            &runner,
            make_opts("ghost@nowhere", &reg_path, &daemon_sock, false),
        );

        assert!(result.is_err(), "should error on unknown key");
        let msg = format!("{:#}", result.unwrap_err());
        assert!(
            msg.contains("ghost@nowhere"),
            "error should mention the unknown key: {msg}"
        );
    }

    // ════════════════════════════════════════════════════════════════════════
    // 8. Remote SSH fails (non-zero exit, not unreachable) — still succeeds locally
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn remove_when_uninstall_cmd_fails_still_removes_from_registry() {
        let dir = tempdir().unwrap();
        let reg_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        seed_registry(&reg_path, &[("devbox1", "alice@devbox1")]);

        // SSH returns non-zero (hooks may already be gone, or roost not installed).
        let runner = MockSshRunner::new([MockResponse::fail()]);

        run_remove(&runner, make_opts("devbox1", &reg_path, &daemon_sock, false)).unwrap();

        let reg = crate::remotes::load_from(&reg_path);
        assert!(reg.all().is_empty(), "registry must be cleaned up even if uninstall cmd failed");
    }

    // ════════════════════════════════════════════════════════════════════════
    // 9. SSH command strings
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn cmd_uninstall_remote_is_correct() {
        assert_eq!(cmd_uninstall_remote(), "roost setup --uninstall");
    }

    #[test]
    fn cmd_purge_remote_contains_rm() {
        let cmd = cmd_purge_remote();
        assert!(cmd.contains("rm"), "purge cmd must contain rm: {cmd}");
        assert!(cmd.contains("roost"), "purge cmd must reference roost binary: {cmd}");
    }

    // ════════════════════════════════════════════════════════════════════════
    // 10. Protocol: RemoveRemote serialization
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn remove_remote_request_serializes_correctly() {
        use crate::protocol::Request;
        let req = Request::RemoveRemote { origin: "devbox1".to_string() };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""type":"remove_remote""#), "{json}");
        assert!(json.contains("devbox1"), "{json}");
        let decoded: Request = serde_json::from_str(&json).unwrap();
        match decoded {
            Request::RemoveRemote { origin } => assert_eq!(origin, "devbox1"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    // ════════════════════════════════════════════════════════════════════════
    // 11. Purge only calls binary-delete on the correct target
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn purge_ssh_calls_go_to_correct_target() {
        let dir = tempdir().unwrap();
        let reg_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        seed_registry(&reg_path, &[("mybox", "bob@mybox.example.com")]);

        let runner = MockSshRunner::new([
            MockResponse::ok(), // uninstall
            MockResponse::ok(), // purge
        ]);

        run_remove(&runner, make_opts("mybox", &reg_path, &daemon_sock, true)).unwrap();

        let calls = runner.calls();
        for (target, _cmd) in &calls {
            assert_eq!(
                target, "bob@mybox.example.com",
                "all SSH calls must go to the correct target"
            );
        }
    }

    // ════════════════════════════════════════════════════════════════════════
    // 12. Unreachable + purge still removes from registry
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn remove_with_purge_when_unreachable_still_removes_registry() {
        let dir = tempdir().unwrap();
        let reg_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        seed_registry(&reg_path, &[("devbox1", "alice@devbox1")]);

        let runner = MockSshRunner::unreachable();

        run_remove(&runner, make_opts("devbox1", &reg_path, &daemon_sock, true)).unwrap();

        let reg = crate::remotes::load_from(&reg_path);
        assert!(reg.all().is_empty());
    }
}
