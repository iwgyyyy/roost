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
        /// Override the roost daemon socket path (default: auto-detected; env: ROOST_SOCK)
        #[arg(long)]
        sock: Option<String>,
        /// Tag this event with a source-machine label (env: ROOST_ORIGIN)
        #[arg(long)]
        origin: Option<String>,
    },
    /// Run the background daemon
    Daemon,
    /// Install/uninstall agent hooks
    Setup {
        /// Remove roost hooks instead of installing
        #[arg(long)]
        uninstall: bool,
        /// Bake a custom daemon socket path into generated hook commands
        /// (for remote agents that forward events back to a hub machine)
        #[arg(long)]
        sock: Option<String>,
        /// Bake a machine label into generated hook commands (e.g. "devbox1")
        #[arg(long)]
        origin: Option<String>,
    },
    /// List currently tracked agent sessions
    List,
    /// Update roost to the latest release (installs from the install script only)
    Update,
    /// Add a remote dev box to the roost fleet
    Add {
        /// SSH target, e.g. `user@devbox1` or `10.0.0.5`
        target: String,
        /// Human-readable label for this machine (defaults to the host part of <target>)
        #[arg(long)]
        origin: Option<String>,
    },
    /// Remove a remote dev box from the roost fleet
    Remove {
        /// SSH target (`user@host`) or origin label identifying the remote to remove
        key: String,
        /// Also delete the roost binary from the remote machine
        #[arg(long)]
        purge: bool,
    },
    /// List all registered remote machines and their tunnel status
    Remotes,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None | Some(Commands::Tui) => {
            maybe_prompt_setup();
            roost::tui::run_tui()
        }
        Some(Commands::Hook {
            family,
            event,
            sock,
            origin,
        }) => roost::hook::run_hook(&family, &event, sock, origin),
        Some(Commands::Daemon) => roost::daemon::run_daemon(),
        Some(Commands::Setup {
            uninstall,
            sock,
            origin,
        }) => {
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

                match roost::setup::uninstall_cursor() {
                    Ok(Some(path)) => println!("Removed Cursor hooks from {}", path.display()),
                    Ok(None) => println!("Cursor not found — skipping."),
                    Err(e) => eprintln!("Warning: Cursor uninstall failed: {e}"),
                }
                Ok(())
            } else {
                install_hooks_with_opts(sock.as_deref(), origin.as_deref())
            }
        }
        Some(Commands::List) => roost::list::run_list(),
        Some(Commands::Update) => roost::update::run_update(),
        Some(Commands::Add { target, origin }) => {
            #[cfg(not(test))]
            {
                use roost::add::{AddOptions, StdSshRunner, run_add};
                let registry_path = roost::remotes::remotes_path();
                let daemon_sock = roost::sock::socket_path();
                run_add(
                    &StdSshRunner,
                    AddOptions {
                        target: &target,
                        origin: origin.as_deref(),
                        registry_path: &registry_path,
                        daemon_sock: &daemon_sock,
                    },
                )
            }
            #[cfg(test)]
            {
                let _ = (target, origin);
                Ok(())
            }
        }
        Some(Commands::Remove { key, purge }) => {
            #[cfg(not(test))]
            {
                use roost::add::StdSshRunner;
                use roost::remove::{RemoveOptions, run_remove};
                let registry_path = roost::remotes::remotes_path();
                let daemon_sock = roost::sock::socket_path();
                run_remove(
                    &StdSshRunner,
                    RemoveOptions {
                        key: &key,
                        registry_path: &registry_path,
                        daemon_sock: &daemon_sock,
                        purge,
                    },
                )
            }
            #[cfg(test)]
            {
                let _ = (key, purge);
                Ok(())
            }
        }
        Some(Commands::Remotes) => {
            #[cfg(not(test))]
            {
                use roost::list_remotes::{ListRemotesOptions, run_list_remotes};
                let registry_path = roost::remotes::remotes_path();
                let daemon_sock = roost::sock::socket_path();
                run_list_remotes(ListRemotesOptions {
                    registry_path: &registry_path,
                    daemon_sock: &daemon_sock,
                })
            }
            #[cfg(test)]
            Ok(())
        }
    }
}

/// Install roost hooks into Claude Code (and Codex if present).
/// `sock` and `origin` are baked into the generated hook commands when given.
fn install_hooks_with_opts(sock: Option<&str>, origin: Option<&str>) -> Result<()> {
    let path = roost::setup::install_claude_with_opts(sock, origin)?;
    println!("Installed roost hooks in {}", path.display());
    println!("Restart Claude Code for hooks to take effect.");

    println!();
    println!("Note: Codex hooks use an experimental feature flag that may change.");
    match roost::setup::install_codex_with_opts(sock, origin) {
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
    match roost::setup::install_deepseek_with_opts(sock, origin) {
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

    println!();
    match roost::setup::install_cursor_with_opts(sock, origin) {
        Ok(Some(path)) => {
            println!("Installed Cursor hooks in {}", path.display());
            println!("Cursor reloads hooks.json on save; restart Cursor if they don't load.");
        }
        Ok(None) => {
            println!(
                "Cursor not found (~/.cursor missing) — install it first, then re-run `roost setup`."
            );
        }
        Err(e) => eprintln!("Warning: Cursor setup failed: {e}"),
    }

    #[cfg(target_os = "linux")]
    maybe_hint_notify_send();

    Ok(())
}

/// Convenience: install without extra flags (local machine default behavior).
#[allow(dead_code)]
fn install_hooks() -> Result<()> {
    install_hooks_with_opts(None, None)
}

/// On Linux, desktop notifications shell out to `notify-send` (from the
/// `libnotify-bin` / `libnotify` package). If it isn't on `PATH`, point the
/// user at it so they can enable popups; if it's already there, stay silent —
/// notifications work out of the box.
#[cfg(target_os = "linux")]
fn maybe_hint_notify_send() {
    use std::env;

    let found = env::var_os("PATH").is_some_and(|paths| {
        env::split_paths(&paths).any(|dir| dir.join("notify-send").is_file())
    });
    if !found {
        println!();
        println!("Tip: install `notify-send` to enable desktop notifications:");
        println!("  Debian/Ubuntu: sudo apt install libnotify-bin");
        println!("  Fedora:        sudo dnf install libnotify");
        println!("  Arch:          sudo pacman -S libnotify");
    }
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
