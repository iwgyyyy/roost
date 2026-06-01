use clap::{Parser, Subcommand};

use anyhow::Result;

#[derive(Parser)]
#[command(name = "roost", about = "Watch your AI coding agents", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the roost TUI panel (default)
    Tui,
    /// Report a hook event to the daemon (called by agent hooks)
    Hook {
        /// Agent family: "claude" or "codex"
        family: String,
        /// Hook event name (e.g. SessionStart, PreToolUse)
        event: String,
    },
    /// Run the background daemon
    Daemon,
    /// Install/uninstall agent hooks
    Setup {
        /// Remove roost hooks instead of installing
        #[arg(long)]
        uninstall: bool,
    },
    /// List currently tracked agent sessions
    List,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None | Some(Commands::Tui) => roost::tui::run_tui(),
        Some(Commands::Hook { family, event }) => roost::hook::run_hook(&family, &event),
        Some(Commands::Daemon) => roost::daemon::run_daemon(),
        Some(Commands::Setup { uninstall }) => {
            if uninstall {
                let path = roost::setup::uninstall_claude()?;
                println!("Removed roost hooks from {}", path.display());

                match roost::setup::uninstall_codex() {
                    Ok(Some(dir)) => println!("Removed Codex hooks from {}", dir.display()),
                    Ok(None) => println!("Codex not found — skipping Codex uninstall."),
                    Err(e) => eprintln!("Warning: Codex uninstall failed: {e}"),
                }
            } else {
                let path = roost::setup::install_claude()?;
                println!("Installed roost hooks in {}", path.display());
                println!("Restart Claude Code for hooks to take effect.");

                println!();
                println!("Note: Codex hooks use an experimental feature flag that may change.");
                match roost::setup::install_codex() {
                    Ok(Some(dir)) => {
                        println!("Installed Codex hooks in {}", dir.display());
                        println!("Restart Codex for hooks to take effect.");
                    }
                    Ok(None) => {
                        println!(
                            "Codex not found (~/.codex missing) — install Codex first, then re-run `roost setup`."
                        );
                    }
                    Err(e) => eprintln!("Warning: Codex setup failed: {e}"),
                }
            }
            Ok(())
        }
        Some(Commands::List) => roost::list::run_list(),
    }
}
