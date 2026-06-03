//! JumpTarget model, host descriptor registry, and alias normalisation.
//!
//! All logic here is pure / dependency-free — no I/O, no env reads.

use crate::protocol::SessionView;

// ── Host category ─────────────────────────────────────────────────────────────

/// Broad category determines which jump implementation to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCategory {
    /// Pure terminal emulators (Ghostty, iTerm2, Terminal.app, WezTerm, …)
    Terminal,
    /// Editor-hosted terminals (VS Code, Cursor, JetBrains, Zed, …)
    Editor,
    /// Desktop apps with deep-link URL schemes (Codex.app)
    DesktopApp,
    /// tmux (multiplexer — special CLI)
    Tmux,
    /// Zellij (multiplexer — special CLI)
    Zellij,
    /// Warp (terminal with SQLite state)
    Warp,
    /// Unknown / unrecognised
    Unknown,
}

// ── Host descriptor ───────────────────────────────────────────────────────────

/// Static descriptor for a known host application.
#[derive(Debug, Clone)]
pub struct HostDescriptor {
    /// Canonical lowercase name used internally (e.g. "ghostty", "cursor").
    pub name: &'static str,
    /// Human-readable display name shown in the TUI.
    pub display_name: &'static str,
    /// macOS bundle identifier (e.g. "com.mitchellh.ghostty").
    pub bundle_id: Option<&'static str>,
    /// Jump category.
    pub category: HostCategory,
    /// Short TUI suffix (e.g. " Ghostty", " Cursor").
    pub tui_label: &'static str,
}

// ── Registry ──────────────────────────────────────────────────────────────────

/// Return the static descriptor for a canonical host name, or `None`.
///
/// The input must already be normalised (lowercase, canonical) — use
/// `normalize_host_name` first.
pub fn descriptor_for(name: &str) -> Option<&'static HostDescriptor> {
    REGISTRY.iter().find(|d| d.name == name)
}

static REGISTRY: &[HostDescriptor] = &[
    // ── Pure terminals (macOS / cross-platform) ────────────────────────────
    HostDescriptor {
        name: "ghostty",
        display_name: "Ghostty",
        bundle_id: Some("com.mitchellh.ghostty"),
        category: HostCategory::Terminal,
        tui_label: " Ghostty",
    },
    HostDescriptor {
        name: "iterm2",
        display_name: "iTerm2",
        bundle_id: Some("com.googlecode.iterm2"),
        category: HostCategory::Terminal,
        tui_label: " iTerm2",
    },
    HostDescriptor {
        name: "terminal",
        display_name: "Terminal.app",
        bundle_id: Some("com.apple.Terminal"),
        category: HostCategory::Terminal,
        tui_label: " Terminal",
    },
    HostDescriptor {
        name: "wezterm",
        display_name: "WezTerm",
        bundle_id: Some("com.github.wez.wezterm"),
        category: HostCategory::Terminal,
        tui_label: " WezTerm",
    },
    HostDescriptor {
        name: "kaku",
        display_name: "Kaku",
        bundle_id: Some("dev.kdrag0n.kaku"),
        category: HostCategory::Terminal,
        tui_label: " Kaku",
    },
    HostDescriptor {
        name: "alacritty",
        display_name: "Alacritty",
        bundle_id: Some("io.alacritty"),
        category: HostCategory::Terminal,
        tui_label: " Alacritty",
    },
    HostDescriptor {
        name: "hyper",
        display_name: "Hyper",
        bundle_id: Some("co.zeit.hyper"),
        category: HostCategory::Terminal,
        tui_label: " Hyper",
    },
    HostDescriptor {
        name: "cmux",
        display_name: "cmux",
        bundle_id: Some("com.cmuxterm.app"), // socket RPC focuses the surface; bundle id is the activate fallback
        category: HostCategory::Terminal,
        tui_label: " cmux",
    },
    // ── Multiplexers ──────────────────────────────────────────────────────
    HostDescriptor {
        name: "tmux",
        display_name: "tmux",
        bundle_id: None,
        category: HostCategory::Tmux,
        tui_label: " tmux",
    },
    HostDescriptor {
        name: "zellij",
        display_name: "Zellij",
        bundle_id: None,
        category: HostCategory::Zellij,
        tui_label: " Zellij",
    },
    HostDescriptor {
        name: "warp",
        display_name: "Warp",
        bundle_id: Some("dev.warp.Warp-Stable"),
        category: HostCategory::Warp,
        tui_label: " Warp",
    },
    // ── Editors ───────────────────────────────────────────────────────────
    HostDescriptor {
        name: "vscode",
        display_name: "VS Code",
        bundle_id: Some("com.microsoft.VSCode"),
        category: HostCategory::Editor,
        tui_label: " Code",
    },
    HostDescriptor {
        name: "vscode-insiders",
        display_name: "VS Code Insiders",
        bundle_id: Some("com.microsoft.VSCodeInsiders"),
        category: HostCategory::Editor,
        tui_label: " Code↑",
    },
    HostDescriptor {
        name: "cursor",
        display_name: "Cursor",
        bundle_id: Some("com.todesktop.230313mzl4w4u92"),
        category: HostCategory::Editor,
        tui_label: " Cursor",
    },
    HostDescriptor {
        name: "windsurf",
        display_name: "Windsurf",
        bundle_id: Some("com.codeium.windsurf"),
        category: HostCategory::Editor,
        tui_label: " Windsurf",
    },
    HostDescriptor {
        name: "trae",
        display_name: "Trae",
        bundle_id: Some("com.bytedance.trae"),
        category: HostCategory::Editor,
        tui_label: " Trae",
    },
    HostDescriptor {
        name: "jetbrains",
        display_name: "JetBrains IDE",
        bundle_id: None, // family — resolved at jump time
        category: HostCategory::Editor,
        tui_label: " JetBrains",
    },
    HostDescriptor {
        name: "zed",
        display_name: "Zed",
        bundle_id: Some("dev.zed.Zed"),
        category: HostCategory::Editor,
        tui_label: " Zed",
    },
    // ── Desktop apps with deep links ─────────────────────────────────────
    HostDescriptor {
        name: "codex",
        display_name: "Codex.app",
        bundle_id: Some("com.openai.codex"),
        category: HostCategory::DesktopApp,
        tui_label: " Codex",
    },
];

// ── Alias normalisation ───────────────────────────────────────────────────────

/// Normalise a raw host string to a canonical registry name.
///
/// Returns the canonical name if known, or the input lowercased.
pub fn normalize_host_name(raw: &str) -> String {
    let lower = raw.to_lowercase();
    // Try exact match first
    if REGISTRY.iter().any(|d| d.name == lower.as_str()) {
        return lower;
    }
    // Aliases
    let canonical = match lower.as_str() {
        // iTerm aliases
        "iterm" | "iterm.app" | "iterm2.app" => "iterm2",
        // Terminal.app aliases
        "terminal.app" | "apple_terminal" | "apple terminal" => "terminal",
        // WezTerm aliases
        "wez" | "wezterm.app" => "wezterm",
        // VS Code aliases
        "code" | "visual studio code" | "visualstudiocode" => "vscode",
        "code-insiders" | "vscode insiders" | "vscode-insiders" => "vscode-insiders",
        // Cursor
        "cursor.app" => "cursor",
        // Warp aliases
        "warpterminal" | "warp terminal" | "warp.app" => "warp",
        // JetBrains family aliases
        "idea" | "intellij" | "intellij-idea" | "webstorm" | "pycharm" | "clion" | "goland"
        | "rustrover" | "rider" | "datagrip" | "appcode" | "phpstorm" | "rubymine" => "jetbrains",
        // Zellij
        "zellij.app" => "zellij",
        // Ghostty
        "ghostty.app" => "ghostty",
        // Zed aliases
        "zed.app" | "zed editor" => "zed",
        // Codex
        "codex.app" | "openai codex" => "codex",
        // Kaku aliases
        "kaku.app" => "kaku",
        _ => return lower,
    };
    canonical.to_string()
}

// ── JumpTarget ────────────────────────────────────────────────────────────────

/// All the information needed to jump to a running agent session.
#[derive(Debug, Clone)]
pub struct JumpTarget {
    pub session_id: String,
    /// Canonical host name (use `normalize_host_name` before storing).
    pub host: String,
    /// Precise macOS bundle id of the host app, when known (e.g. a preview or
    /// fork variant). Preferred over the descriptor's static bundle id.
    pub host_bundle_id: Option<String>,
    pub terminal_session_id: Option<String>,
    pub terminal_tty: Option<String>,
    pub tmux_target: Option<String>,
    pub wezterm_pane: Option<String>,
    pub zellij_session: Option<String>,
    pub workspace_path: Option<String>,
    pub codex_thread_id: Option<String>,
    pub pane_title: Option<String>,
    /// Working directory (used as workspace_path fallback for editors).
    pub cwd: String,
    /// Origin of the session: `"local"` for local sessions, or a remote name
    /// (e.g. `"devbox1"`) for sessions running on a remote host.
    pub origin: String,
}

impl JumpTarget {
    /// Build a `JumpTarget` from a `SessionView`.
    pub fn from_session_view(sv: &SessionView) -> Self {
        let raw_host = sv.host_app.as_deref().unwrap_or("unknown");
        JumpTarget {
            session_id: sv.id.clone(),
            host: normalize_host_name(raw_host),
            host_bundle_id: sv.host_bundle_id.clone(),
            terminal_session_id: sv.terminal_session_id.clone(),
            terminal_tty: sv.terminal_tty.clone(),
            tmux_target: sv.tmux_target.clone(),
            wezterm_pane: sv.wezterm_pane.clone(),
            zellij_session: sv.zellij_session.clone(),
            workspace_path: sv.workspace_path.clone(),
            codex_thread_id: sv.codex_thread_id.clone(),
            pane_title: sv.pane_title.clone(),
            cwd: String::new(), // SessionView doesn't expose cwd; fill at jump call site
            origin: sv.origin.clone(),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AgentFamily;

    fn sv_with_host(host: &str) -> SessionView {
        SessionView {
            id: "s1".to_string(),
            family: AgentFamily::Claude,
            name: "repo".to_string(),
            cwd: String::new(),
            state: "working".to_string(),
            activity: None,
            last_activity_secs: 0,
            busy_secs: 0,
            last_prompt: None,
            last_prompt_age_secs: None,
            host_app: Some(host.to_string()),
            host_bundle_id: None,
            terminal_session_id: Some("sess-abc".to_string()),
            terminal_tty: Some("/dev/ttys001".to_string()),
            tmux_target: Some("/tmp/tmux|$3".to_string()),
            wezterm_pane: Some("5".to_string()),
            zellij_session: Some("dev".to_string()),
            workspace_path: Some("/home/user/project".to_string()),
            codex_thread_id: Some("thread-1".to_string()),
            pane_title: Some("claude".to_string()),
            origin: "local".to_string(),
            stale: false,
        }
    }

    // ── JumpTarget construction ─────────────────────────────────────────────

    #[test]
    fn jump_target_from_session_view_populates_all_fields() {
        let sv = sv_with_host("ghostty");
        let t = JumpTarget::from_session_view(&sv);
        assert_eq!(t.host, "ghostty");
        assert_eq!(t.terminal_session_id.as_deref(), Some("sess-abc"));
        assert_eq!(t.terminal_tty.as_deref(), Some("/dev/ttys001"));
        assert_eq!(t.tmux_target.as_deref(), Some("/tmp/tmux|$3"));
        assert_eq!(t.wezterm_pane.as_deref(), Some("5"));
        assert_eq!(t.zellij_session.as_deref(), Some("dev"));
        assert_eq!(t.workspace_path.as_deref(), Some("/home/user/project"));
        assert_eq!(t.codex_thread_id.as_deref(), Some("thread-1"));
        assert_eq!(t.pane_title.as_deref(), Some("claude"));
    }

    #[test]
    fn jump_target_no_host_app_gives_unknown() {
        let mut sv = sv_with_host("ghostty");
        sv.host_app = None;
        let t = JumpTarget::from_session_view(&sv);
        assert_eq!(t.host, "unknown");
    }

    #[test]
    fn jump_target_from_session_view_carries_local_origin() {
        let sv = sv_with_host("ghostty"); // sv_with_host sets origin = "local"
        let t = JumpTarget::from_session_view(&sv);
        assert_eq!(t.origin, "local");
    }

    #[test]
    fn jump_target_from_session_view_carries_remote_origin() {
        let mut sv = sv_with_host("ghostty");
        sv.origin = "devbox1".to_string();
        let t = JumpTarget::from_session_view(&sv);
        assert_eq!(t.origin, "devbox1");
    }

    // ── normalize_host_name ────────────────────────────────────────────────

    #[test]
    fn normalize_ghostty_variants() {
        assert_eq!(normalize_host_name("ghostty"), "ghostty");
        assert_eq!(normalize_host_name("Ghostty"), "ghostty");
        assert_eq!(normalize_host_name("ghostty.app"), "ghostty");
    }

    #[test]
    fn normalize_iterm_variants() {
        assert_eq!(normalize_host_name("iterm"), "iterm2");
        assert_eq!(normalize_host_name("iTerm.app"), "iterm2");
        assert_eq!(normalize_host_name("iterm2"), "iterm2");
        assert_eq!(normalize_host_name("iterm2.app"), "iterm2");
    }

    #[test]
    fn normalize_terminal_variants() {
        assert_eq!(normalize_host_name("terminal"), "terminal");
        assert_eq!(normalize_host_name("Apple_Terminal"), "terminal");
        assert_eq!(normalize_host_name("terminal.app"), "terminal");
    }

    #[test]
    fn normalize_warp_variants() {
        assert_eq!(normalize_host_name("warp"), "warp");
        assert_eq!(normalize_host_name("WarpTerminal"), "warp");
        assert_eq!(normalize_host_name("warp.app"), "warp");
    }

    #[test]
    fn normalize_vscode_variants() {
        assert_eq!(normalize_host_name("vscode"), "vscode");
        assert_eq!(normalize_host_name("code"), "vscode");
        assert_eq!(normalize_host_name("Visual Studio Code"), "vscode");
        assert_eq!(normalize_host_name("code-insiders"), "vscode-insiders");
        assert_eq!(normalize_host_name("vscode-insiders"), "vscode-insiders");
    }

    #[test]
    fn normalize_cursor() {
        assert_eq!(normalize_host_name("cursor"), "cursor");
        assert_eq!(normalize_host_name("Cursor"), "cursor");
        assert_eq!(normalize_host_name("cursor.app"), "cursor");
    }

    #[test]
    fn normalize_jetbrains_family() {
        assert_eq!(normalize_host_name("idea"), "jetbrains");
        assert_eq!(normalize_host_name("webstorm"), "jetbrains");
        assert_eq!(normalize_host_name("pycharm"), "jetbrains");
        assert_eq!(normalize_host_name("clion"), "jetbrains");
        assert_eq!(normalize_host_name("goland"), "jetbrains");
        assert_eq!(normalize_host_name("rustrover"), "jetbrains");
    }

    #[test]
    fn normalize_zed_variants() {
        assert_eq!(normalize_host_name("zed"), "zed");
        assert_eq!(normalize_host_name("zed.app"), "zed");
        assert_eq!(normalize_host_name("Zed Editor"), "zed");
    }

    #[test]
    fn normalize_codex_variants() {
        assert_eq!(normalize_host_name("codex"), "codex");
        assert_eq!(normalize_host_name("Codex.app"), "codex");
    }

    #[test]
    fn normalize_tmux_wezterm_zellij() {
        assert_eq!(normalize_host_name("tmux"), "tmux");
        assert_eq!(normalize_host_name("TMUX"), "tmux");
        assert_eq!(normalize_host_name("wezterm"), "wezterm");
        assert_eq!(normalize_host_name("zellij"), "zellij");
    }

    #[test]
    fn normalize_unknown_returns_lowercased() {
        assert_eq!(normalize_host_name("SomeFutureTerm"), "somefutureterm");
        assert_eq!(normalize_host_name("unknown"), "unknown");
    }

    // ── descriptor_for ────────────────────────────────────────────────────

    #[test]
    fn descriptor_for_all_registered_hosts() {
        for name in &[
            "ghostty",
            "iterm2",
            "terminal",
            "wezterm",
            "kaku",
            "alacritty",
            "hyper",
            "cmux",
            "tmux",
            "zellij",
            "warp",
            "vscode",
            "vscode-insiders",
            "cursor",
            "windsurf",
            "trae",
            "jetbrains",
            "zed",
            "codex",
        ] {
            let d = descriptor_for(name);
            assert!(d.is_some(), "descriptor_for({name:?}) should be Some");
            assert_eq!(d.unwrap().name, *name);
        }
    }

    #[test]
    fn descriptor_for_unknown_returns_none() {
        assert!(descriptor_for("somefutureterm").is_none());
        assert!(descriptor_for("").is_none());
    }

    #[test]
    fn descriptor_categories_correct() {
        assert_eq!(
            descriptor_for("ghostty").unwrap().category,
            HostCategory::Terminal
        );
        assert_eq!(
            descriptor_for("cursor").unwrap().category,
            HostCategory::Editor
        );
        assert_eq!(
            descriptor_for("zed").unwrap().category,
            HostCategory::Editor
        );
        assert_eq!(
            descriptor_for("codex").unwrap().category,
            HostCategory::DesktopApp
        );
        assert_eq!(descriptor_for("tmux").unwrap().category, HostCategory::Tmux);
        assert_eq!(
            descriptor_for("zellij").unwrap().category,
            HostCategory::Zellij
        );
        assert_eq!(descriptor_for("warp").unwrap().category, HostCategory::Warp);
    }

    #[test]
    fn descriptor_bundle_ids_present_where_expected() {
        assert!(descriptor_for("ghostty").unwrap().bundle_id.is_some());
        assert!(descriptor_for("iterm2").unwrap().bundle_id.is_some());
        assert!(descriptor_for("codex").unwrap().bundle_id.is_some());
        // cmux focuses its surface via a Unix-socket RPC, but also carries a
        // bundle id for the activate-app fallback.
        assert!(descriptor_for("cmux").unwrap().bundle_id.is_some());
        // tmux / zellij are multiplexers with no app bundle id
        assert!(descriptor_for("tmux").unwrap().bundle_id.is_none());
        assert!(descriptor_for("zellij").unwrap().bundle_id.is_none());
    }

    #[test]
    fn tui_labels_non_empty() {
        for d in REGISTRY {
            assert!(!d.tui_label.is_empty(), "tui_label empty for {}", d.name);
        }
    }
}
