//! Editor-hosted terminal jump implementations.
//!
//! VS Code family: `<cli> -r <workspace>`
//! JetBrains family: `<cli> <project>`
//! Zed: `zed <workspace>`

use super::{Executor, JumpOutcome};

// ── CLI argument construction ──────────────────────────────────────────────────

/// Build the CLI args to focus/reuse a VS Code-family window for the workspace.
///
/// VS Code family uses `-r` (reuse window) flag to focus an existing instance.
pub fn vscode_reuse_args(workspace: &str) -> Vec<String> {
    vec!["-r".to_string(), workspace.to_string()]
}

/// Build the CLI args for JetBrains IDE to open/focus a project.
///
/// JetBrains CLI tools accept the project path directly (no `-r` flag).
pub fn jetbrains_open_args(project: &str) -> Vec<String> {
    vec![project.to_string()]
}

/// Build the CLI args for Zed to open/focus a workspace.
///
/// Zed follows the same pattern as VS Code: `zed <workspace>`.
pub fn zed_open_args(workspace: &str) -> Vec<String> {
    vec![workspace.to_string()]
}

// ── CLI selection ─────────────────────────────────────────────────────────────

/// Determine the CLI binary name and argument builder for an editor host.
///
/// Returns `(cli_binary, args_for_workspace)`.
pub fn build_editor_args(cli: &str, workspace: &str) -> (String, Vec<String>) {
    let args = match cli {
        "code" | "vscode" | "code-insiders" | "cursor" | "windsurf" | "trae" => {
            vscode_reuse_args(workspace)
        }
        "zed" => zed_open_args(workspace),
        // JetBrains: idea / webstorm / pycharm / clion / goland / rustrover / rider …
        _ => jetbrains_open_args(workspace),
    };
    (cli.to_string(), args)
}

// ── jump_editor ────────────────────────────────────────────────────────────────

/// Execute an editor CLI jump to the workspace.
///
/// Precision: **window-level** — we can focus the editor window that contains
/// the project, but cannot navigate to the specific embedded terminal tab.
pub fn jump_editor(cli: &str, workspace: &str, executor: &dyn Executor) -> JumpOutcome {
    let (bin, args) = build_editor_args(cli, workspace);
    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let (_, ok) = executor.run(&bin, &args_refs);
    if ok {
        JumpOutcome::Activated {
            message: format!("Focused {bin} project window — embedded terminal tab is best-effort"),
        }
    } else {
        JumpOutcome::Failed {
            message: format!("Editor CLI '{bin}' failed for workspace '{workspace}'"),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── argument construction ──────────────────────────────────────────────

    #[test]
    fn vscode_reuse_args_correct() {
        let args = vscode_reuse_args("/home/user/project");
        assert_eq!(args, vec!["-r", "/home/user/project"]);
    }

    #[test]
    fn jetbrains_open_args_correct() {
        let args = jetbrains_open_args("/home/user/project");
        assert_eq!(args, vec!["/home/user/project"]);
    }

    #[test]
    fn zed_open_args_correct() {
        let args = zed_open_args("/home/user/my-project");
        assert_eq!(args, vec!["/home/user/my-project"]);
    }

    #[test]
    fn build_editor_args_vscode() {
        let (cli, args) = build_editor_args("code", "/ws");
        assert_eq!(cli, "code");
        assert_eq!(args, vec!["-r", "/ws"]);
    }

    #[test]
    fn build_editor_args_cursor() {
        let (cli, args) = build_editor_args("cursor", "/ws");
        assert_eq!(cli, "cursor");
        assert_eq!(args, vec!["-r", "/ws"]);
    }

    #[test]
    fn build_editor_args_code_insiders() {
        let (cli, args) = build_editor_args("code-insiders", "/ws");
        assert_eq!(cli, "code-insiders");
        assert_eq!(args, vec!["-r", "/ws"]);
    }

    #[test]
    fn build_editor_args_windsurf() {
        let (cli, args) = build_editor_args("windsurf", "/ws");
        assert_eq!(cli, "windsurf");
        assert_eq!(args, vec!["-r", "/ws"]);
    }

    #[test]
    fn build_editor_args_trae() {
        let (cli, args) = build_editor_args("trae", "/ws");
        assert_eq!(cli, "trae");
        assert_eq!(args, vec!["-r", "/ws"]);
    }

    #[test]
    fn build_editor_args_zed() {
        let (cli, args) = build_editor_args("zed", "/home/user/project");
        assert_eq!(cli, "zed");
        assert_eq!(args, vec!["/home/user/project"]);
    }

    #[test]
    fn build_editor_args_jetbrains_idea() {
        let (cli, args) = build_editor_args("idea", "/home/user/project");
        assert_eq!(cli, "idea");
        // JetBrains: no -r flag
        assert_eq!(args, vec!["/home/user/project"]);
        assert!(!args.contains(&"-r".to_string()));
    }

    #[test]
    fn build_editor_args_jetbrains_webstorm() {
        let (_cli, args) = build_editor_args("webstorm", "/home/user/project");
        assert_eq!(args, vec!["/home/user/project"]);
    }

    #[test]
    fn build_editor_args_zed_does_not_use_r_flag() {
        let (_, args) = build_editor_args("zed", "/ws");
        assert!(
            !args.contains(&"-r".to_string()),
            "Zed should not use -r flag"
        );
    }

    #[test]
    fn build_editor_args_jetbrains_does_not_use_r_flag() {
        let (_, args) = build_editor_args("idea", "/ws");
        assert!(
            !args.contains(&"-r".to_string()),
            "JetBrains should not use -r flag"
        );
    }
}
