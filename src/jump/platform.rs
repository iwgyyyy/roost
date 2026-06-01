//! Platform-specific executor implementation.
//!
//! macOS: `open`, `osascript`, `open -b <bundle>`, URL scheme.
//! Linux: `wmctrl`/`xdotool` (best-effort), `xdg-open`.

use super::Executor;

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
            std::process::Command::new("open")
                .arg(url)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("xdg-open")
                .arg(url)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
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
            std::process::Command::new("open")
                .args(["-b", bundle_id])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        #[cfg(not(target_os = "macos"))]
        {
            // Linux: try wmctrl or xdotool as best-effort window focuser.
            // We don't have a bundle id concept; just return false.
            let _ = bundle_id;
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
            std::process::Command::new("open")
                .arg(path)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("xdg-open")
                .arg(path)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
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
