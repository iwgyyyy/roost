//! `roost update` — in-place self-update.
//!
//! Uses `axoupdater`, which reads the install receipt written by the curl
//! install script (`roost-installer.sh`), checks GitHub Releases for a newer
//! version, and downloads + installs it over the current binary.
//!
//! This only works for installs done via the install script (or the updater
//! itself) — those leave a receipt. Source installs (`cargo install`) have no
//! receipt and are pointed at the right command instead.

use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use axoupdater::AxoUpdater;

use crate::sock;

const INSTALLER_URL: &str =
    "https://github.com/iwgyyyy/roost/releases/latest/download/roost-installer.sh";
const SOURCE_REPO: &str = "https://github.com/iwgyyyy/roost";

/// Run the self-update: upgrade to the latest GitHub release if one exists.
pub fn run_update() -> Result<()> {
    let mut updater = AxoUpdater::new_for("roost");

    // The receipt records the installed version and how it was installed.
    // Without it (e.g. a `cargo install` build) we can't self-update.
    if updater.load_receipt().is_err() {
        return Err(anyhow!(
            "no install receipt found — `roost update` only works for installs \
             from the install script.\n\nUpdate via the install script:\n  \
             curl -fsSL {INSTALLER_URL} | sh\n\nOr, if you installed from source:\n  \
             cargo install --git {SOURCE_REPO} --force"
        ));
    }

    match updater
        .run_sync()
        .map_err(|e| anyhow!("update failed: {e}"))?
    {
        Some(result) => {
            println!("Updated roost to {}.", result.new_version);
            // The new binary is now on disk, but a daemon that is already
            // running still holds the OLD code in memory — restart it so the
            // update (e.g. history recording) takes effect immediately.
            restart_daemon_if_running();
        }
        None => println!("roost is already the latest version."),
    }
    Ok(())
}

/// Stop a running daemon and start the new binary in its place. No-op if no
/// daemon is running — the next `roost` launch starts the new binary itself.
fn restart_daemon_if_running() {
    let path = sock::socket_path();
    if !sock::is_listening(&path) {
        return; // nothing running; next `roost` launch picks up the new binary
    }

    // Stop the old daemon. `pkill` works regardless of the daemon's version (a
    // graceful socket shutdown would not — daemons before this release don't
    // understand it). The running process here is `roost update`, never
    // `roost daemon`, so this does not target ourselves.
    let _ = Command::new("pkill").args(["-f", "roost daemon"]).status();

    // Wait (bounded) for the old daemon to release the socket.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && sock::is_listening(&path) {
        std::thread::sleep(Duration::from_millis(50));
    }

    if sock::is_listening(&path) {
        // Could not stop it (e.g. `pkill` unavailable) — don't claim success.
        eprintln!(
            "Note: the running daemon couldn't be stopped automatically. Run \
             `pkill -f 'roost daemon'` or restart it to pick up the new version."
        );
        return;
    }

    match sock::ensure_daemon() {
        Ok(()) => println!("Restarted the roost daemon on the new version."),
        Err(e) => eprintln!(
            "Note: couldn't start the new daemon ({e}); it will start on your next `roost`."
        ),
    }
}
