//! `roost remotes` — list all registered remote machines with their tunnel state.
//!
//! ## Behaviour
//! 1. Send `Request::ListRemotes` to the running daemon.
//!    The daemon replies with a JSON array of [`RemoteStatus`] entries that
//!    include live tunnel state (`up` / `reconnecting` / `down`).
//! 2. If the daemon is not reachable, fall back to reading `remotes.json`
//!    directly and mark all tunnel states as `unknown`.
//! 3. Print a human-readable table:
//!    ```text
//!    ORIGIN      TARGET                  TUNNEL        ADDED
//!    devbox1     alice@devbox1           up            2026-06-03T00:00:00Z
//!    buildbox    bob@buildbox.local      unknown       2026-05-20T08:00:00Z
//!    ```
//!
//! ## Safety contract
//! This module never executes `ssh` and never writes any file.

use std::path::Path;

use anyhow::Result;

use crate::protocol::{RemoteStatus, TunnelState};
use crate::remotes::Registry;

// ── Column widths ──────────────────────────────────────────────────────────────

const W_ORIGIN: usize = 15;
const W_TARGET: usize = 28;
const W_TUNNEL: usize = 13;

// ── Public entry point ────────────────────────────────────────────────────────

/// Options for [`run_list_remotes`].
pub struct ListRemotesOptions<'a> {
    /// Path to the remotes registry file (tempfile in tests).
    pub registry_path: &'a Path,
    /// Path to the local daemon socket.
    pub daemon_sock: &'a Path,
}

/// Execute the `roost remotes` workflow.
///
/// Returns the rendered table as a `String` (for testability).
/// The production caller prints it to stdout; tests inspect the string.
pub fn render_remotes_table(
    statuses: &[RemoteStatus],
    daemon_online: bool,
) -> String {
    let mut out = String::new();

    if !daemon_online {
        out.push_str("(daemon offline — tunnel states unavailable)\n\n");
    }

    if statuses.is_empty() {
        out.push_str("No remotes registered. Run `roost add <ssh-target>` to add one.\n");
        return out;
    }

    // Header
    out.push_str(&format!(
        "{:<width_o$}  {:<width_t$}  {:<width_s$}  {}\n",
        "ORIGIN",
        "TARGET",
        "TUNNEL",
        "ADDED",
        width_o = W_ORIGIN,
        width_t = W_TARGET,
        width_s = W_TUNNEL,
    ));
    // Separator
    out.push_str(&format!(
        "{:-<width_o$}  {:-<width_t$}  {:-<width_s$}  {:-<20}\n",
        "",
        "",
        "",
        "",
        width_o = W_ORIGIN,
        width_t = W_TARGET,
        width_s = W_TUNNEL,
    ));

    for rs in statuses {
        out.push_str(&format!(
            "{:<width_o$}  {:<width_t$}  {:<width_s$}  {}\n",
            truncate(&rs.origin, W_ORIGIN),
            truncate(&rs.target, W_TARGET),
            rs.tunnel.to_string(),
            rs.added_at,
            width_o = W_ORIGIN,
            width_t = W_TARGET,
            width_s = W_TUNNEL,
        ));
    }

    out
}

/// Truncate a string to `max` chars, appending `…` if it was cut.
fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        let cut: String = chars[..max.saturating_sub(1)].iter().collect();
        format!("{cut}…")
    }
}

/// Build [`RemoteStatus`] list from the registry using `Unknown` for all tunnels.
/// Used as the fallback when the daemon is offline.
pub fn statuses_from_registry(registry: &Registry) -> Vec<RemoteStatus> {
    registry
        .all()
        .iter()
        .map(|r| RemoteStatus {
            origin: r.origin.clone(),
            target: r.target.clone(),
            arch: r.arch.clone(),
            added_at: r.added_at.clone(),
            tunnel: TunnelState::Unknown,
        })
        .collect()
}

/// Query the daemon for live remote statuses, falling back to the registry
/// file if the daemon is unreachable.
///
/// Returns `(statuses, daemon_online)`.
pub fn query_or_fallback(opts: &ListRemotesOptions<'_>) -> (Vec<RemoteStatus>, bool) {
    // Try daemon first.
    if let Ok(statuses) = query_daemon(opts.daemon_sock) {
        return (statuses, true);
    }
    // Daemon offline — read registry directly.
    let registry = crate::remotes::load_from(opts.registry_path);
    (statuses_from_registry(&registry), false)
}

/// Send `Request::ListRemotes` to the daemon and parse the reply.
fn query_daemon(sock: &Path) -> Result<Vec<RemoteStatus>> {
    use crate::protocol::Request;
    use crate::sock;

    let mut stream = sock::connect_hook(sock)?;
    let req = Request::ListRemotes;
    let json = serde_json::to_string(&req)?;
    sock::send_line(&mut stream, &json)?;
    let reply = sock::recv_line(&mut stream)?;
    let statuses: Vec<RemoteStatus> = serde_json::from_str(&reply)?;
    Ok(statuses)
}

/// Execute the `roost remotes` workflow and print to stdout.
pub fn run_list_remotes(opts: ListRemotesOptions<'_>) -> Result<()> {
    let (statuses, daemon_online) = query_or_fallback(&opts);
    let table = render_remotes_table(&statuses, daemon_online);
    print!("{table}");
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::TunnelState;
    use crate::remotes::{Registry, Remote};
    use tempfile::tempdir;

    fn make_status(origin: &str, target: &str, tunnel: TunnelState) -> RemoteStatus {
        RemoteStatus {
            origin: origin.to_string(),
            target: target.to_string(),
            arch: "linux-x86_64".to_string(),
            added_at: "2026-06-03T00:00:00Z".to_string(),
            tunnel,
        }
    }

    fn make_remote(origin: &str, target: &str) -> Remote {
        Remote {
            origin: origin.to_string(),
            target: target.to_string(),
            remote_sock: "~/.roost/hub.sock".to_string(),
            added_at: "2026-06-03T00:00:00Z".to_string(),
            arch: "linux-x86_64".to_string(),
        }
    }

    // ════════════════════════════════════════════════════════════════════════
    // 1. Empty registry / no remotes
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn render_empty_list_shows_helpful_message() {
        let table = render_remotes_table(&[], true);
        assert!(
            table.contains("No remotes registered"),
            "empty table should hint at roost add: {table}"
        );
    }

    // ════════════════════════════════════════════════════════════════════════
    // 2. Daemon offline → shows warning
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn render_shows_daemon_offline_message() {
        let table = render_remotes_table(&[], false);
        assert!(
            table.contains("daemon offline"),
            "should mention daemon offline: {table}"
        );
    }

    // ════════════════════════════════════════════════════════════════════════
    // 3. Table contains correct columns
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn render_table_has_header_and_row() {
        let statuses = vec![make_status("devbox1", "alice@devbox1", TunnelState::Up)];
        let table = render_remotes_table(&statuses, true);

        assert!(table.contains("ORIGIN"), "header missing ORIGIN: {table}");
        assert!(table.contains("TARGET"), "header missing TARGET: {table}");
        assert!(table.contains("TUNNEL"), "header missing TUNNEL: {table}");
        assert!(table.contains("ADDED"), "header missing ADDED: {table}");
        assert!(table.contains("devbox1"), "row missing origin: {table}");
        assert!(table.contains("alice@devbox1"), "row missing target: {table}");
        assert!(table.contains("up"), "row missing tunnel state: {table}");
    }

    // ════════════════════════════════════════════════════════════════════════
    // 4. All tunnel states render correctly
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn render_all_tunnel_states() {
        let statuses = vec![
            make_status("a", "user@a", TunnelState::Up),
            make_status("b", "user@b", TunnelState::Reconnecting),
            make_status("c", "user@c", TunnelState::Down),
            make_status("d", "user@d", TunnelState::Unknown),
        ];
        let table = render_remotes_table(&statuses, true);

        for expected in ["up", "reconnecting", "down", "unknown"] {
            assert!(
                table.contains(expected),
                "table should contain tunnel state '{expected}': {table}"
            );
        }
    }

    // ════════════════════════════════════════════════════════════════════════
    // 5. Multiple remotes all appear
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn render_multiple_remotes_all_present() {
        let statuses = vec![
            make_status("box1", "alice@box1", TunnelState::Up),
            make_status("box2", "bob@box2", TunnelState::Reconnecting),
            make_status("box3", "carol@box3", TunnelState::Down),
        ];
        let table = render_remotes_table(&statuses, true);

        for origin in ["box1", "box2", "box3"] {
            assert!(table.contains(origin), "table missing {origin}: {table}");
        }
    }

    // ════════════════════════════════════════════════════════════════════════
    // 6. Fallback from registry when daemon offline
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn statuses_from_registry_marks_all_as_unknown() {
        let mut reg = Registry::default();
        reg.upsert(make_remote("box1", "alice@box1"));
        reg.upsert(make_remote("box2", "bob@box2"));

        let statuses = statuses_from_registry(&reg);
        assert_eq!(statuses.len(), 2);
        for s in &statuses {
            assert_eq!(
                s.tunnel,
                TunnelState::Unknown,
                "offline fallback must use Unknown: {:?}",
                s
            );
        }
    }

    // ════════════════════════════════════════════════════════════════════════
    // 7. query_or_fallback returns registry data when daemon socket absent
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn query_or_fallback_uses_registry_when_daemon_absent() {
        let dir = tempdir().unwrap();
        let reg_path = dir.path().join("remotes.json");
        let daemon_sock = dir.path().join("nonexistent.sock"); // no daemon running

        let mut reg = Registry::default();
        reg.upsert(make_remote("devbox1", "alice@devbox1"));
        crate::remotes::save_to(&reg_path, &reg).unwrap();

        let opts = ListRemotesOptions {
            registry_path: &reg_path,
            daemon_sock: &daemon_sock,
        };

        let (statuses, online) = query_or_fallback(&opts);
        assert!(!online, "daemon should be reported offline");
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].origin, "devbox1");
        assert_eq!(statuses[0].tunnel, TunnelState::Unknown);
    }

    // ════════════════════════════════════════════════════════════════════════
    // 8. Truncation helper
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_length_unchanged() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn truncate_long_string_gets_ellipsis() {
        let result = truncate("very-long-origin-name-that-exceeds-limit", 10);
        assert!(result.len() <= 10 || result.chars().count() <= 10);
        assert!(result.contains('…'), "truncated string should contain ellipsis: {result}");
    }

    // ════════════════════════════════════════════════════════════════════════
    // 9. Protocol: ListRemotes roundtrip
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn list_remotes_request_roundtrip() {
        use crate::protocol::Request;
        let req = Request::ListRemotes;
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""type":"list_remotes""#), "{json}");
        let _decoded: Request = serde_json::from_str(&json).unwrap();
    }

    // ════════════════════════════════════════════════════════════════════════
    // 10. RemoteStatus roundtrip
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn remote_status_roundtrip() {
        let rs = make_status("devbox1", "alice@devbox1", TunnelState::Reconnecting);
        let json = serde_json::to_string(&rs).unwrap();
        let decoded: RemoteStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.origin, "devbox1");
        assert_eq!(decoded.tunnel, TunnelState::Reconnecting);
        assert_eq!(decoded.target, "alice@devbox1");
    }

    // ════════════════════════════════════════════════════════════════════════
    // 11. render_remotes_table daemon online, no offline message
    // ════════════════════════════════════════════════════════════════════════

    #[test]
    fn render_online_no_offline_message() {
        let statuses = vec![make_status("box1", "user@box1", TunnelState::Up)];
        let table = render_remotes_table(&statuses, true);
        assert!(
            !table.contains("daemon offline"),
            "online mode must not say daemon offline: {table}"
        );
    }
}
