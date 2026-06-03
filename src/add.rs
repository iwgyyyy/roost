//! `roost add <ssh-target> [--origin <name>]` — provision a remote dev box.
//!
//! ## Safety contract
//! **No code in this file executes `ssh`, `curl`, or any real network I/O.**
//! All SSH interactions are abstracted behind the [`SshRunner`] trait.
//! The production implementation [`StdSshRunner`] calls `std::process::Command`
//! and is only compiled in non-test builds.  Tests inject a [`MockSshRunner`]
//! that records calls and returns pre-programmed outputs.
//!
//! ## Steps performed by [`run_add`]
//! 1. Probe connectivity + arch: `ssh <target> 'uname -sm'` → parse arch string.
//! 2. Ensure remote binary: `ssh <target> 'command -v roost'`; if absent, install.
//! 3. Install remote hook: `ssh <target> 'roost setup --origin <name> --sock ~/.roost/hub.sock'`.
//! 4. Persist entry: upsert + save `~/.roost/remotes.json`.
//! 5. Notify daemon (if running): send `Request::AddRemote`; if daemon absent, skip.

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::remotes::Remote;

// ── Installer URL ─────────────────────────────────────────────────────────────

/// Public installer URL baked into `roost add` — the cargo-dist installer
/// attached to the latest GitHub release (same script as the README curl
/// command).  Tests never fetch this.
const INSTALLER_URL: &str =
    "https://github.com/iwgyyyy/roost/releases/latest/download/roost-installer.sh";

// ── SshRunner seam ────────────────────────────────────────────────────────────

/// Outcome of a single remote command execution.
#[derive(Debug, Clone)]
pub struct RunOutput {
    /// Combined stdout of the command.
    pub stdout: String,
    /// Whether the command exited with status 0.
    pub success: bool,
}

/// Seam trait: run a command on a remote host via SSH.
///
/// In production: [`StdSshRunner`] shells out via `std::process::Command`.
/// In tests: a mock records calls and returns pre-programmed outputs.
pub trait SshRunner: Send + Sync {
    /// Execute `command` on `target` (e.g. `user@host`) and return its output.
    ///
    /// The implementation must NOT add extra arguments beyond what is required
    /// to run the command non-interactively with minimal overhead.  The exact
    /// argv built by the production runner is `ssh -o BatchMode=yes -o
    /// ConnectTimeout=10 <target> <command>`.
    fn run(&self, target: &str, command: &str) -> Result<RunOutput>;
}

// ── Production SshRunner ───────────────────────────────────────────────────────

/// Production [`SshRunner`]: delegates to `ssh` via `std::process::Command`.
///
/// Intentionally absent from test builds so tests can never accidentally
/// trigger a real SSH connection.
#[cfg(not(test))]
pub struct StdSshRunner;

#[cfg(not(test))]
impl SshRunner for StdSshRunner {
    fn run(&self, target: &str, command: &str) -> Result<RunOutput> {
        let output = std::process::Command::new("ssh")
            .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
            .arg(target)
            .arg(command)
            .output()
            .with_context(|| format!("failed to launch ssh for target {target}"))?;
        Ok(RunOutput {
            stdout: String::from_utf8_lossy(&output.stdout).trim_end().to_string(),
            success: output.status.success(),
        })
    }
}

// ── Pure helpers ───────────────────────────────────────────────────────────────

/// Build the `ssh` command string for probing connectivity and arch.
///
/// ```text
/// uname -sm
/// ```
/// Output looks like "Linux x86_64" or "Darwin arm64".
pub fn cmd_probe_arch() -> &'static str {
    "uname -sm"
}

/// Build the `ssh` command string for checking whether `roost` is on PATH.
pub fn cmd_check_roost() -> &'static str {
    "command -v roost"
}

/// Build the installer command string.
pub fn cmd_install_roost() -> String {
    format!("curl -fsSL {INSTALLER_URL} | sh")
}

/// Build the remote setup command string.
///
/// `origin` is the label for this machine; `hub_sock` is the path of the
/// roost daemon socket on the **local (hub)** machine that the remote should
/// forward events to.
pub fn cmd_setup_remote(origin: &str, hub_sock: &str) -> String {
    format!("roost setup --origin {origin} --sock {hub_sock}")
}

/// Parse the output of `uname -sm` into a normalised arch tag.
///
/// Examples:
/// - `"Linux x86_64"` → `"linux-x86_64"`
/// - `"Linux aarch64"` → `"linux-arm64"`
/// - `"Darwin arm64"` → `"darwin-aarch64"`
/// - `"Darwin x86_64"` → `"darwin-x86_64"`
///
/// Returns `Err` if the string is empty or cannot be parsed.
pub fn parse_arch(uname_sm: &str) -> Result<String> {
    let parts: Vec<&str> = uname_sm.split_whitespace().collect();
    if parts.len() < 2 {
        bail!("unexpected `uname -sm` output: {:?}", uname_sm);
    }
    let os = parts[0].to_lowercase();
    let raw_arch = parts[1];

    // Normalise arch aliases to a consistent form.
    let arch = match raw_arch {
        "aarch64" => "arm64",
        "arm64" => "aarch64",
        other => other,
    };

    Ok(format!("{os}-{arch}"))
}

/// Derive a default origin label from an SSH target string.
///
/// Rules:
/// - `user@host` → `host`
/// - `host` (no `@`) → `host`
/// - IP addresses are kept as-is.
///
/// # Examples
/// ```
/// # use roost::add::default_origin;
/// assert_eq!(default_origin("alice@devbox1"), "devbox1");
/// assert_eq!(default_origin("devbox1"),       "devbox1");
/// assert_eq!(default_origin("bob@10.0.0.5"),  "10.0.0.5");
/// assert_eq!(default_origin("10.0.0.5"),      "10.0.0.5");
/// ```
pub fn default_origin(target: &str) -> &str {
    // Everything after the last `@`, or the whole string if no `@`.
    match target.rsplit_once('@') {
        Some((_user, host)) => host,
        None => target,
    }
}

// ── Remote hub socket path ─────────────────────────────────────────────────────

/// The socket path on the **hub** machine that remote agents should forward to.
/// This is the local daemon socket, hardcoded to the standard location because
/// the remote `roost setup` command is baked into the remote agent hooks.
///
/// In practice: `~/.roost/hub.sock` is a stable, user-visible alias.  The
/// actual daemon socket is OS-dependent, but we instruct remote agents to
/// connect to this fixed path that the tunnel maps.
pub const HUB_SOCK: &str = "~/.roost/hub.sock";

// ── Step orchestration ────────────────────────────────────────────────────────

/// Options for [`run_add`].
pub struct AddOptions<'a> {
    /// SSH target, e.g. `user@devbox1` or `10.0.0.5`.
    pub target: &'a str,
    /// Override the origin label.  If `None`, derived from `target`.
    pub origin: Option<&'a str>,
    /// Path to the remotes registry file (for testability; production uses
    /// [`crate::remotes::remotes_path()`]).
    pub registry_path: &'a Path,
    /// Path to the local daemon socket (used when notifying the daemon).
    pub daemon_sock: &'a Path,
}

/// Execute the `roost add` workflow against `runner`.
///
/// Returns `Ok(())` on full success.  Any hard failure returns `Err`.
/// Progress is printed to stdout.
///
/// Tests supply a mock runner; production supplies [`StdSshRunner`].
pub fn run_add<R: SshRunner>(runner: &R, opts: AddOptions<'_>) -> Result<()> {
    let origin = opts.origin.unwrap_or_else(|| default_origin(opts.target));

    // ── Step 1: probe connectivity + arch ─────────────────────────────────
    println!("[1/4] Checking connectivity to {} …", opts.target);
    let arch_out = runner.run(opts.target, cmd_probe_arch()).with_context(|| {
        format!(
            "SSH connection to {} failed.\n\
             Make sure passwordless (key-based) SSH access is set up:\n\
             ssh-copy-id {}",
            opts.target, opts.target
        )
    })?;

    if !arch_out.success {
        bail!(
            "SSH connection to {} failed (exit code non-zero).\n\
             Make sure passwordless (key-based) SSH access is set up:\n\
             ssh-copy-id {}",
            opts.target,
            opts.target
        );
    }

    let arch = parse_arch(&arch_out.stdout).with_context(|| {
        format!(
            "could not parse arch from `uname -sm` output: {:?}",
            arch_out.stdout
        )
    })?;
    println!("    ✓ Connected — arch: {arch}");

    // ── Step 2: ensure remote binary ──────────────────────────────────────
    println!("[2/4] Checking for roost binary on {} …", opts.target);
    let check_out = runner.run(opts.target, cmd_check_roost())?;
    if !check_out.success || check_out.stdout.trim().is_empty() {
        println!("    roost not found — installing via curl …");
        let install_cmd = cmd_install_roost();
        let install_out = runner.run(opts.target, &install_cmd)?;
        if !install_out.success {
            bail!(
                "roost install on {} failed.\n\
                 Ensure the remote machine has `curl` and internet access,\n\
                 or install roost manually and re-run `roost add`.",
                opts.target
            );
        }
        println!("    ✓ roost installed");
    } else {
        println!("    ✓ roost already present: {}", check_out.stdout.trim());
    }

    // ── Step 3: install remote hook ───────────────────────────────────────
    println!("[3/4] Installing roost hook on {} (origin={origin}) …", opts.target);
    let setup_cmd = cmd_setup_remote(origin, HUB_SOCK);
    let setup_out = runner.run(opts.target, &setup_cmd)?;
    if !setup_out.success {
        bail!(
            "roost setup failed on {}.\n\
             Ensure roost is installed and the remote shell can run `roost setup`.",
            opts.target
        );
    }
    println!("    ✓ Hook installed");

    // ── Step 4: write remotes.json ────────────────────────────────────────
    println!("[4/4] Registering remote …");
    let now = {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // Minimal RFC-3339-like timestamp (seconds precision, UTC).
        format_timestamp(secs)
    };

    let remote = Remote {
        origin: origin.to_string(),
        target: opts.target.to_string(),
        remote_sock: HUB_SOCK.to_string(),
        added_at: now,
        arch,
    };

    let mut registry = crate::remotes::load_from(opts.registry_path);
    registry.upsert(remote.clone());
    crate::remotes::save_to(opts.registry_path, &registry)?;
    println!("    ✓ Saved to {}", opts.registry_path.display());

    // ── Step 5: notify daemon (best-effort) ───────────────────────────────
    notify_daemon(opts.daemon_sock, &remote);

    println!();
    println!("Done! Remote '{origin}' ({}) is now registered.", opts.target);
    println!("Sessions from that machine will appear in `roost` once the tunnel is active.");

    Ok(())
}

/// Format a Unix timestamp as a minimal UTC timestamp string (`YYYY-MM-DDTHH:MM:SSZ`).
fn format_timestamp(secs: u64) -> String {
    // Pure arithmetic — no external crate.
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400; // days since 1970-01-01

    // Gregorian calendar calculation.
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // 400-year cycle has 97 leap years → 146097 days.
    let z = days + 719468;
    let era = z / 146097;
    let doe = z % 146097; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // year of era [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year [0, 365]
    let mp = (5 * doy + 2) / 153; // month primev [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // day [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // month [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Send `Request::AddRemote` to the daemon socket (best-effort).
///
/// If the daemon is not running, silently skips — the entry was already
/// written to `remotes.json` and will be picked up on next daemon start.
fn notify_daemon(sock: &Path, remote: &Remote) {
    use crate::protocol::Request;
    use crate::sock;

    // Try to connect; if the daemon isn't there, just skip.
    let mut stream = match sock::connect_hook(sock) {
        Ok(s) => s,
        Err(_) => {
            // Daemon not running — that's fine.
            return;
        }
    };

    let req = Request::AddRemote {
        target: remote.target.clone(),
        origin: remote.origin.clone(),
        remote_sock: remote.remote_sock.clone(),
        arch: remote.arch.clone(),
    };

    if let Ok(json) = serde_json::to_string(&req) {
        let _ = sock::send_line(&mut stream, &json);
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use tempfile::tempdir;

    // ── Mock SSH runner ────────────────────────────────────────────────────

    /// Pre-programmed response for a single SSH call.
    #[derive(Debug, Clone)]
    struct MockResponse {
        stdout: String,
        success: bool,
    }

    impl MockResponse {
        fn ok(stdout: impl Into<String>) -> Self {
            Self { stdout: stdout.into(), success: true }
        }
        fn fail(stdout: impl Into<String>) -> Self {
            Self { stdout: stdout.into(), success: false }
        }
    }

    /// Mock SSH runner: records (target, command) pairs and returns pre-programmed responses.
    struct MockSshRunner {
        /// Responses to return, in order.  Pops from the front on each call.
        responses: Mutex<VecDeque<MockResponse>>,
        /// All (target, command) calls received.
        calls: Mutex<Vec<(String, String)>>,
    }

    impl MockSshRunner {
        fn new(responses: impl IntoIterator<Item = MockResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }

        /// Drain all recorded calls.
        fn recorded_calls(&self) -> Vec<(String, String)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl SshRunner for MockSshRunner {
        fn run(&self, target: &str, command: &str) -> Result<RunOutput> {
            self.calls
                .lock()
                .unwrap()
                .push((target.to_string(), command.to_string()));

            let resp = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("MockSshRunner: no more pre-programmed responses");

            Ok(RunOutput {
                stdout: resp.stdout,
                success: resp.success,
            })
        }
    }

    // ── Helper: build AddOptions pointing at a tempdir ──────────────────────

    fn make_opts<'a>(
        target: &'a str,
        origin: Option<&'a str>,
        registry_path: &'a std::path::Path,
        daemon_sock: &'a std::path::Path,
    ) -> AddOptions<'a> {
        AddOptions { target, origin, registry_path, daemon_sock }
    }

    // ════════════════════════════════════════════════════════════════════════
    // 1. Pure helper tests (no network, no disk)
    // ════════════════════════════════════════════════════════════════════════

    // ── 1a. arch parsing ───────────────────────────────────────────────────

    #[test]
    fn parse_arch_linux_x86_64() {
        assert_eq!(parse_arch("Linux x86_64").unwrap(), "linux-x86_64");
    }

    #[test]
    fn parse_arch_linux_aarch64_normalised_to_arm64() {
        // aarch64 → arm64 for consistency with common usage.
        assert_eq!(parse_arch("Linux aarch64").unwrap(), "linux-arm64");
    }

    #[test]
    fn parse_arch_darwin_arm64_normalised_to_aarch64() {
        // arm64 (macOS) → aarch64.
        assert_eq!(parse_arch("Darwin arm64").unwrap(), "darwin-aarch64");
    }

    #[test]
    fn parse_arch_darwin_x86_64() {
        assert_eq!(parse_arch("Darwin x86_64").unwrap(), "darwin-x86_64");
    }

    #[test]
    fn parse_arch_rejects_empty_string() {
        assert!(parse_arch("").is_err());
    }

    #[test]
    fn parse_arch_rejects_single_word() {
        assert!(parse_arch("Linux").is_err());
    }

    #[test]
    fn parse_arch_lowercases_os() {
        // The OS part must be lowercase in the output.
        let result = parse_arch("LINUX x86_64").unwrap();
        assert!(result.starts_with("linux-"), "OS should be lowercase: {result}");
    }

    // ── 1b. origin derivation ──────────────────────────────────────────────

    #[test]
    fn default_origin_strips_user_at() {
        assert_eq!(default_origin("alice@devbox1"), "devbox1");
    }

    #[test]
    fn default_origin_no_at_returns_whole_string() {
        assert_eq!(default_origin("devbox1"), "devbox1");
    }

    #[test]
    fn default_origin_ip_with_user() {
        assert_eq!(default_origin("bob@10.0.0.5"), "10.0.0.5");
    }

    #[test]
    fn default_origin_ip_without_user() {
        assert_eq!(default_origin("10.0.0.5"), "10.0.0.5");
    }

    #[test]
    fn default_origin_uses_last_at_segment() {
        // Edge case: multiple @ signs — use everything after the LAST @.
        assert_eq!(default_origin("a@b@host.example.com"), "host.example.com");
    }

    // ── 1c. command string construction ───────────────────────────────────

    #[test]
    fn cmd_probe_arch_is_uname_sm() {
        assert_eq!(cmd_probe_arch(), "uname -sm");
    }

    #[test]
    fn cmd_check_roost_uses_command_v() {
        assert_eq!(cmd_check_roost(), "command -v roost");
    }

    #[test]
    fn cmd_install_roost_contains_curl_and_url() {
        let cmd = cmd_install_roost();
        assert!(cmd.contains("curl"), "install cmd must contain curl: {cmd}");
        assert!(cmd.contains("https://"), "install cmd must contain https URL: {cmd}");
        assert!(cmd.contains("| sh"), "install cmd must pipe to sh: {cmd}");
    }

    #[test]
    fn cmd_setup_remote_includes_origin_and_sock() {
        let cmd = cmd_setup_remote("devbox1", "~/.roost/hub.sock");
        assert!(cmd.contains("--origin devbox1"), "missing --origin flag: {cmd}");
        assert!(cmd.contains("--sock ~/.roost/hub.sock"), "missing --sock flag: {cmd}");
        assert!(cmd.starts_with("roost setup"), "must call roost setup: {cmd}");
    }

    // ════════════════════════════════════════════════════════════════════════
    // 2. SSH command ordering / structure tests (mock runner, no real SSH)
    // ════════════════════════════════════════════════════════════════════════

    /// Happy path: remote already has roost.
    /// Verifies the exact target and command strings passed to the SSH runner
    /// in the correct order.
    #[test]
    fn run_add_calls_ssh_in_correct_order_roost_present() {
        let dir = tempdir().unwrap();
        let registry_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        let runner = MockSshRunner::new([
            MockResponse::ok("Linux x86_64"),    // step 1: uname -sm
            MockResponse::ok("/usr/bin/roost"),   // step 2: command -v roost → found
            MockResponse::ok(""),                  // step 3: roost setup
        ]);

        run_add(
            &runner,
            make_opts("alice@devbox1", None, &registry_path, &daemon_sock),
        )
        .unwrap();

        let calls = runner.recorded_calls();
        assert_eq!(calls.len(), 3, "should have made exactly 3 SSH calls");

        // All calls must go to the same target.
        assert!(calls.iter().all(|(t, _)| t == "alice@devbox1"));

        // Call order.
        assert_eq!(calls[0].1, "uname -sm");
        assert_eq!(calls[1].1, "command -v roost");
        assert!(calls[2].1.contains("roost setup"));
    }

    /// When roost is absent on the remote, an install step is inserted between
    /// step 2 (check) and step 3 (setup).
    #[test]
    fn run_add_installs_roost_when_absent() {
        let dir = tempdir().unwrap();
        let registry_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        let runner = MockSshRunner::new([
            MockResponse::ok("Linux x86_64"),   // step 1: uname -sm
            MockResponse::fail(""),              // step 2: command -v roost → not found
            MockResponse::ok(""),               // step 2b: curl installer
            MockResponse::ok(""),               // step 3: roost setup
        ]);

        run_add(
            &runner,
            make_opts("bob@devbox2", None, &registry_path, &daemon_sock),
        )
        .unwrap();

        let calls = runner.recorded_calls();
        assert_eq!(calls.len(), 4, "expected 4 SSH calls (check + install + setup + uname):\n{calls:?}");
        assert!(calls[2].1.contains("curl"), "3rd call should be curl installer");
        assert!(calls[3].1.contains("roost setup"), "4th call should be roost setup");
    }

    /// SSH connection failure on step 1 → bail with a clear message.
    #[test]
    fn run_add_fails_fast_on_connectivity_error() {
        let dir = tempdir().unwrap();
        let registry_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        // Simulate SSH reporting failure (non-zero exit).
        let runner = MockSshRunner::new([
            MockResponse::fail(""),  // step 1: uname -sm → SSH auth failure
        ]);

        let result = run_add(
            &runner,
            make_opts("user@unreachable", None, &registry_path, &daemon_sock),
        );

        assert!(result.is_err(), "should fail on connectivity error");
        let msg = format!("{:#}", result.unwrap_err());
        assert!(
            msg.contains("SSH") || msg.contains("ssh") || msg.contains("failed"),
            "error message should mention SSH failure: {msg}"
        );

        // Registry must NOT have been written.
        assert!(!registry_path.exists(), "registry must not be written on failure");
    }

    /// Installer failure (curl fails) → bail with a helpful message.
    #[test]
    fn run_add_fails_when_installer_fails() {
        let dir = tempdir().unwrap();
        let registry_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        let runner = MockSshRunner::new([
            MockResponse::ok("Linux x86_64"),   // step 1: uname -sm OK
            MockResponse::fail(""),              // step 2: command -v roost → not found
            MockResponse::fail("curl: (6) Could not resolve host"), // step 2b: installer fails
        ]);

        let result = run_add(
            &runner,
            make_opts("user@nonet", None, &registry_path, &daemon_sock),
        );

        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(
            msg.contains("install") || msg.contains("curl"),
            "error should mention install/curl failure: {msg}"
        );
    }

    /// Setup failure → bail with a message, no registry written.
    #[test]
    fn run_add_fails_when_setup_fails() {
        let dir = tempdir().unwrap();
        let registry_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        let runner = MockSshRunner::new([
            MockResponse::ok("Linux x86_64"),
            MockResponse::ok("/usr/bin/roost"),
            MockResponse::fail(""),  // setup fails
        ]);

        let result = run_add(
            &runner,
            make_opts("user@box", None, &registry_path, &daemon_sock),
        );

        assert!(result.is_err());
        assert!(!registry_path.exists(), "registry must not be written on setup failure");
    }

    // ════════════════════════════════════════════════════════════════════════
    // 3. Origin derivation in full workflow
    // ════════════════════════════════════════════════════════════════════════

    /// When no --origin is given, the host part of the target is used.
    #[test]
    fn run_add_derives_origin_from_target() {
        let dir = tempdir().unwrap();
        let registry_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        let runner = MockSshRunner::new([
            MockResponse::ok("Linux x86_64"),
            MockResponse::ok("/usr/bin/roost"),
            MockResponse::ok(""),
        ]);

        run_add(
            &runner,
            make_opts("alice@mydevbox", None, &registry_path, &daemon_sock),
        )
        .unwrap();

        let registry = crate::remotes::load_from(&registry_path);
        assert_eq!(registry.all().len(), 1);
        assert_eq!(
            registry.all()[0].origin, "mydevbox",
            "origin should be derived from host part of target"
        );
    }

    /// When --origin is given explicitly, it overrides the derived value.
    #[test]
    fn run_add_respects_explicit_origin() {
        let dir = tempdir().unwrap();
        let registry_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        let runner = MockSshRunner::new([
            MockResponse::ok("Linux x86_64"),
            MockResponse::ok("/usr/bin/roost"),
            MockResponse::ok(""),
        ]);

        run_add(
            &runner,
            make_opts("alice@mydevbox", Some("mybox"), &registry_path, &daemon_sock),
        )
        .unwrap();

        let registry = crate::remotes::load_from(&registry_path);
        assert_eq!(registry.all()[0].origin, "mybox");
    }

    // ════════════════════════════════════════════════════════════════════════
    // 4. Registry persistence tests
    // ════════════════════════════════════════════════════════════════════════

    /// After a successful add, the entry must appear in the registry on disk.
    #[test]
    fn run_add_writes_registry_entry() {
        let dir = tempdir().unwrap();
        let registry_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        let runner = MockSshRunner::new([
            MockResponse::ok("Linux x86_64"),
            MockResponse::ok("/usr/bin/roost"),
            MockResponse::ok(""),
        ]);

        run_add(
            &runner,
            make_opts("alice@devbox1", None, &registry_path, &daemon_sock),
        )
        .unwrap();

        let registry = crate::remotes::load_from(&registry_path);
        assert_eq!(registry.all().len(), 1);
        let entry = &registry.all()[0];
        assert_eq!(entry.target, "alice@devbox1");
        assert_eq!(entry.origin, "devbox1");
        assert_eq!(entry.arch, "linux-x86_64");
        assert!(!entry.added_at.is_empty());
    }

    /// Running `roost add` twice with the same target must be idempotent:
    /// only one registry entry, with the latest data.
    #[test]
    fn run_add_is_idempotent() {
        let dir = tempdir().unwrap();
        let registry_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        // First add.
        let runner1 = MockSshRunner::new([
            MockResponse::ok("Linux x86_64"),
            MockResponse::ok("/usr/bin/roost"),
            MockResponse::ok(""),
        ]);
        run_add(
            &runner1,
            make_opts("alice@devbox1", None, &registry_path, &daemon_sock),
        )
        .unwrap();

        // Second add (re-add same target).
        let runner2 = MockSshRunner::new([
            MockResponse::ok("Linux aarch64"),   // arch changed (e.g. different box)
            MockResponse::ok("/usr/local/bin/roost"),
            MockResponse::ok(""),
        ]);
        run_add(
            &runner2,
            make_opts("alice@devbox1", None, &registry_path, &daemon_sock),
        )
        .unwrap();

        let registry = crate::remotes::load_from(&registry_path);
        // Must still be exactly one entry (upsert replaced the old one).
        assert_eq!(registry.all().len(), 1, "idempotent add must not create duplicate entries");
        // The latest arch should win.
        assert_eq!(registry.all()[0].arch, "linux-arm64");
    }

    /// Two different targets → two registry entries.
    #[test]
    fn run_add_multiple_targets_creates_separate_entries() {
        let dir = tempdir().unwrap();
        let registry_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        let runner1 = MockSshRunner::new([
            MockResponse::ok("Linux x86_64"),
            MockResponse::ok("/usr/bin/roost"),
            MockResponse::ok(""),
        ]);
        run_add(
            &runner1,
            make_opts("alice@box1", None, &registry_path, &daemon_sock),
        )
        .unwrap();

        let runner2 = MockSshRunner::new([
            MockResponse::ok("Linux aarch64"),
            MockResponse::ok("/usr/bin/roost"),
            MockResponse::ok(""),
        ]);
        run_add(
            &runner2,
            make_opts("alice@box2", None, &registry_path, &daemon_sock),
        )
        .unwrap();

        let registry = crate::remotes::load_from(&registry_path);
        assert_eq!(registry.all().len(), 2);
        let origins: Vec<&str> = registry.all().iter().map(|r| r.origin.as_str()).collect();
        assert!(origins.contains(&"box1"), "box1 missing: {origins:?}");
        assert!(origins.contains(&"box2"), "box2 missing: {origins:?}");
    }

    // ════════════════════════════════════════════════════════════════════════
    // 5. Timestamp helper tests
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn format_timestamp_epoch_zero() {
        // Unix epoch: 1970-01-01T00:00:00Z
        assert_eq!(format_timestamp(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn format_timestamp_known_date() {
        // 2026-06-03T12:00:00Z = 1748952000 seconds since epoch.
        // Verify the format is correct (not the exact value — just the shape).
        let ts = format_timestamp(1_748_952_000);
        assert!(ts.ends_with('Z'), "must end with Z: {ts}");
        assert_eq!(ts.len(), 20, "must be 20 chars: {ts}");
        // Must match YYYY-MM-DDTHH:MM:SSZ pattern.
        assert!(ts.chars().nth(4) == Some('-'), "year-month separator: {ts}");
        assert!(ts.chars().nth(7) == Some('-'), "month-day separator: {ts}");
        assert!(ts.chars().nth(10) == Some('T'), "date-time separator: {ts}");
    }

    // ════════════════════════════════════════════════════════════════════════
    // 6. setup command includes origin in SSH call
    // ════════════════════════════════════════════════════════════════════════

    /// The setup SSH command sent to the remote must contain the correct --origin flag.
    #[test]
    fn run_add_setup_command_contains_origin() {
        let dir = tempdir().unwrap();
        let registry_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock");

        let runner = MockSshRunner::new([
            MockResponse::ok("Linux x86_64"),
            MockResponse::ok("/usr/bin/roost"),
            MockResponse::ok(""),
        ]);

        run_add(
            &runner,
            make_opts("alice@server1", Some("prod-box"), &registry_path, &daemon_sock),
        )
        .unwrap();

        let calls = runner.recorded_calls();
        let setup_call = calls.iter().find(|(_, cmd)| cmd.contains("roost setup"));
        assert!(setup_call.is_some(), "no setup call found in: {calls:?}");
        let setup_cmd = &setup_call.unwrap().1;
        assert!(
            setup_cmd.contains("--origin prod-box"),
            "setup command must include --origin prod-box: {setup_cmd}"
        );
        assert!(
            setup_cmd.contains("--sock"),
            "setup command must include --sock: {setup_cmd}"
        );
    }
}
