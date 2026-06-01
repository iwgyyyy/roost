//! Platform-specific executor implementation.
//!
//! macOS: `open`, `osascript`, `open -b <bundle>`, URL scheme.
//! Linux: `wmctrl`/`xdotool` (best-effort), `xdg-open`.

use std::process::Stdio;

use super::Executor;

/// Spawn a fire-and-forget command with stdin/stdout/stderr all detached from
/// the parent terminal, returning whether it exited successfully.
///
/// This matters because `roost` runs inside an alternate-screen TUI: any output
/// a launched helper writes (e.g. macOS LaunchServices warnings like
/// `LSCopyApplicationURLsForBundleIdentifier() failed …`) would otherwise land
/// directly on the screen and corrupt the layout.
#[allow(dead_code)]
fn status_detached(mut cmd: std::process::Command) -> bool {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ── RealExecutor ──────────────────────────────────────────────────────────────

/// The real executor: delegates to OS commands.
///
/// Used in production; test code substitutes a mock.
pub struct RealExecutor;

impl Executor for RealExecutor {
    fn run(&self, cmd: &str, args: &[&str]) -> (String, bool) {
        match std::process::Command::new(cmd).args(args).output() {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                (stdout, out.status.success())
            }
            Err(_) => (String::new(), false),
        }
    }

    fn open_url(&self, url: &str) -> bool {
        #[cfg(target_os = "macos")]
        {
            let mut cmd = std::process::Command::new("open");
            cmd.arg(url);
            status_detached(cmd)
        }
        #[cfg(target_os = "linux")]
        {
            let mut cmd = std::process::Command::new("xdg-open");
            cmd.arg(url);
            status_detached(cmd)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = url;
            false
        }
    }

    fn activate_app(&self, bundle_id: &str) -> bool {
        #[cfg(target_os = "macos")]
        {
            let mut cmd = std::process::Command::new("open");
            cmd.args(["-b", bundle_id]);
            status_detached(cmd)
        }
        #[cfg(not(target_os = "macos"))]
        {
            // Linux: try wmctrl or xdotool as best-effort window focuser.
            // We don't have a bundle id concept; just return false.
            let _ = bundle_id;
            false
        }
    }

    fn open_path_in_app(&self, bundle_id: &str, path: &str) -> bool {
        #[cfg(target_os = "macos")]
        {
            let mut cmd = std::process::Command::new("open");
            cmd.args(["-b", bundle_id, path]);
            status_detached(cmd)
        }
        #[cfg(not(target_os = "macos"))]
        {
            // No bundle-id concept off macOS; editor CLI path handles those.
            let _ = (bundle_id, path);
            false
        }
    }

    fn run_applescript(&self, script: &str) -> (String, bool) {
        #[cfg(target_os = "macos")]
        {
            match std::process::Command::new("osascript")
                .arg("-e")
                .arg(script)
                .output()
            {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                    (stdout, out.status.success())
                }
                Err(_) => (String::new(), false),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = script;
            (String::new(), false)
        }
    }

    fn open_dir(&self, path: &str) -> bool {
        #[cfg(target_os = "macos")]
        {
            let mut cmd = std::process::Command::new("open");
            cmd.arg(path);
            status_detached(cmd)
        }
        #[cfg(target_os = "linux")]
        {
            let mut cmd = std::process::Command::new("xdg-open");
            cmd.arg(path);
            status_detached(cmd)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = path;
            false
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify RealExecutor can run a known-good command.
    #[test]
    fn real_executor_can_run_true() {
        let exec = RealExecutor;
        let (_, ok) = exec.run("true", &[]);
        assert!(ok, "running `true` should succeed");
    }

    #[test]
    fn real_executor_false_fails() {
        let exec = RealExecutor;
        let (_, ok) = exec.run("false", &[]);
        assert!(!ok, "running `false` should fail");
    }

    #[test]
    fn real_executor_nonexistent_command_fails() {
        let exec = RealExecutor;
        let (_, ok) = exec.run("__nonexistent_cmd_roost_test__", &[]);
        assert!(!ok, "nonexistent command should fail");
    }
}
