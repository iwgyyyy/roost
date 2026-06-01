//! `roost update` — in-place self-update.
//!
//! Uses `axoupdater`, which reads the install receipt written by the curl
//! install script (`roost-installer.sh`), checks GitHub Releases for a newer
//! version, and downloads + installs it over the current binary.
//!
//! This only works for installs done via the install script (or the updater
//! itself) — those leave a receipt. Source installs (`cargo install`) have no
//! receipt and are pointed at the right command instead.

use anyhow::{Result, anyhow};
use axoupdater::AxoUpdater;

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
        Some(result) => println!("Updated roost to {}.", result.new_version),
        None => println!("roost is already the latest version."),
    }
    Ok(())
}
