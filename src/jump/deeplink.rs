//! Desktop app deep-link jumps (Codex.app).

use super::{Executor, JumpOutcome};

// ── Codex.app ─────────────────────────────────────────────────────────────────

const CODEX_BUNDLE_ID: &str = "com.openai.codex";

/// Build the `codex://threads/<thread_id>` deep-link URL.
pub fn codex_thread_url(thread_id: &str) -> String {
    format!("codex://threads/{thread_id}")
}

/// Jump to a Codex.app conversation.
///
/// - If `thread_id` is available: open `codex://threads/<id>` (conversation-level).
/// - Otherwise: activate Codex.app via bundle id (app-level).
pub fn jump_codex(thread_id: Option<&str>, executor: &dyn Executor) -> JumpOutcome {
    if let Some(id) = thread_id {
        let url = codex_thread_url(id);
        if executor.open_url(&url) {
            return JumpOutcome::Focused {
                message: format!("Opened Codex.app conversation '{id}'"),
            };
        }
        // URL open failed — fall through to app activation
    }
    // Fallback: activate app
    if executor.activate_app(CODEX_BUNDLE_ID) {
        JumpOutcome::Activated {
            message: "Activated Codex.app".to_string(),
        }
    } else {
        JumpOutcome::Failed {
            message: "Failed to open Codex.app".to_string(),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ── URL construction ──────────────────────────────────────────────────

    #[test]
    fn codex_thread_url_format() {
        assert_eq!(
            codex_thread_url("thread-abc-123"),
            "codex://threads/thread-abc-123"
        );
    }

    #[test]
    fn codex_thread_url_with_special_chars() {
        let url = codex_thread_url("abc/def");
        assert!(url.starts_with("codex://threads/"));
        assert!(url.contains("abc/def"));
    }

    // ── Mock executor ─────────────────────────────────────────────────────

    #[derive(Default)]
    struct MockExec {
        calls: Mutex<Vec<String>>,
        url_success: bool,
        app_success: bool,
    }

    impl super::super::Executor for MockExec {
        fn run(&self, cmd: &str, args: &[&str]) -> (String, bool) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("run:{cmd} {}", args.join(" ")));
            (String::new(), false)
        }

        fn open_url(&self, url: &str) -> bool {
            self.calls.lock().unwrap().push(format!("open_url:{url}"));
            self.url_success
        }

        fn activate_app(&self, bundle_id: &str) -> bool {
            self.calls
                .lock()
                .unwrap()
                .push(format!("activate_app:{bundle_id}"));
            self.app_success
        }

        fn run_applescript(&self, script: &str) -> (String, bool) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("applescript:{script}"));
            (String::new(), false)
        }

        fn open_dir(&self, path: &str) -> bool {
            self.calls.lock().unwrap().push(format!("open_dir:{path}"));
            false
        }
    }

    #[test]
    fn jump_codex_with_thread_id_opens_url() {
        let exec = MockExec {
            url_success: true,
            ..Default::default()
        };
        let outcome = jump_codex(Some("thread-1"), &exec);
        assert!(
            matches!(outcome, JumpOutcome::Focused { .. }),
            "expected Focused, got: {outcome:?}"
        );
        let calls = exec.calls.lock().unwrap();
        assert!(
            calls.iter().any(|c| c.contains("codex://threads/thread-1")),
            "should open URL; calls: {calls:?}"
        );
    }

    #[test]
    fn jump_codex_url_failure_falls_back_to_activate() {
        let exec = MockExec {
            url_success: false,
            app_success: true,
            ..Default::default()
        };
        let outcome = jump_codex(Some("thread-1"), &exec);
        assert!(
            matches!(outcome, JumpOutcome::Activated { .. }),
            "expected Activated fallback, got: {outcome:?}"
        );
        let calls = exec.calls.lock().unwrap();
        assert!(
            calls
                .iter()
                .any(|c| c.contains("activate_app") && c.contains(CODEX_BUNDLE_ID)),
            "should call activate_app; calls: {calls:?}"
        );
    }

    #[test]
    fn jump_codex_no_thread_id_activates_app() {
        let exec = MockExec {
            app_success: true,
            ..Default::default()
        };
        let outcome = jump_codex(None, &exec);
        assert!(
            matches!(outcome, JumpOutcome::Activated { .. }),
            "expected Activated, got: {outcome:?}"
        );
        // Should NOT have opened any URL
        let calls = exec.calls.lock().unwrap();
        assert!(
            !calls.iter().any(|c| c.starts_with("open_url")),
            "should not open URL when no thread id; calls: {calls:?}"
        );
    }

    #[test]
    fn jump_codex_all_fail_gives_failed() {
        let exec = MockExec {
            url_success: false,
            app_success: false,
            ..Default::default()
        };
        let outcome = jump_codex(Some("thread-1"), &exec);
        assert!(
            matches!(outcome, JumpOutcome::Failed { .. }),
            "expected Failed, got: {outcome:?}"
        );
    }
}
