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
    /// Update roost to the latest release (installs from the install script only)
    Update,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None | Some(Commands::Tui) => {
            maybe_prompt_setup();
            roost::tui::run_tui()
        }
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

                match roost::setup::uninstall_deepseek() {
                    Ok(Some(dir)) => println!("Removed DeepSeek hooks from {}", dir.display()),
                    Ok(None) => println!("DeepSeek (CodeWhale) not found — skipping."),
                    Err(e) => eprintln!("Warning: DeepSeek uninstall failed: {e}"),
                }
                Ok(())
            } else {
                install_hooks()
            }
        }
        Some(Commands::List) => roost::list::run_list(),
        Some(Commands::Update) => roost::update::run_update(),
    }
}

/// Install roost hooks into Claude Code (and Codex if present).
fn install_hooks() -> Result<()> {
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

    println!();
    match roost::setup::install_deepseek() {
        Ok(Some(dir)) => {
            println!("Installed DeepSeek (CodeWhale) hooks in {}", dir.display());
            println!("Restart CodeWhale for hooks to take effect.");
        }
        Ok(None) => {
            println!(
                "DeepSeek (CodeWhale) not found (~/.codewhale missing) — install it first, then re-run `roost setup`."
            );
        }
        Err(e) => eprintln!("Warning: DeepSeek setup failed: {e}"),
    }
    Ok(())
}

/// On first run (TUI), if no roost hooks are installed yet, offer to install
/// them. Only prompts on an interactive terminal; non-interactive launches just
/// proceed (agents simply won't appear until `roost setup` is run).
fn maybe_prompt_setup() {
    use std::io::{self, IsTerminal, Write};

    if !io::stdin().is_terminal() || roost::setup::hooks_installed() {
        return;
    }

    eprintln!("roost hooks aren't installed yet — agents won't appear until they are.");
    eprint!("Install hooks now? [Y/n] ");
    let _ = io::stderr().flush();

    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return;
    }
    let ans = line.trim().to_ascii_lowercase();
    if !(ans.is_empty() || ans == "y" || ans == "yes") {
        eprintln!("Skipped. Run `roost setup` anytime to install them.");
        return;
    }

    if let Err(e) = install_hooks() {
        eprintln!("Setup failed: {e:#}");
    }
    // Let the user read the setup output before the TUI takes over the screen.
    eprint!("\nRestart any running Claude Code / Codex sessions, then press Enter to open roost… ");
    let _ = io::stderr().flush();
    let mut _ignore = String::new();
    let _ = io::stdin().read_line(&mut _ignore);
}
