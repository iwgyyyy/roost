use std::time::{Duration, Instant};

// ── Source-aware liveness decision ────────────────────────────────────────────

/// Tunnel connectivity state used by the liveness decision function.
///
/// This mirrors `remote_supervisor::ConnState` but lives here so the pure
/// function has no dependency on the supervisor (easier to unit-test).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelState {
    /// Tunnel is up (SSH process running).
    Connected,
    /// SSH exited; watcher is retrying.
    Reconnecting,
    /// All retry attempts exhausted; tunnel is dead.
    Down,
}

/// Decision returned by [`decide`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivenessAction {
    /// Keep the session alive; no state change.
    Keep,
    /// Mark the session stale (freeze busy timer); keep it in the table.
    Freeze,
    /// Transition the session to Idle (no longer active); keep it in the table.
    ToIdle,
    /// Remove the session from the state table entirely.
    Remove,
}

/// Maximum idle duration for a remote session before it is moved to Idle.
/// Remote sessions don't have a PID to probe, so we use a generous timeout.
pub const REMOTE_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60); // 10 minutes

/// Source-aware liveness decision — pure function, no I/O.
///
/// # Parameters
/// - `is_local`: true when the session originates from the local machine.
/// - `tunnel`: tunnel state for the origin (only meaningful when `is_local` is false).
/// - `pid_alive`: whether `kill(pid, 0)` succeeded (only meaningful when `is_local` is true).
/// - `consecutive_dead`: number of consecutive probe failures (local path only).
/// - `idle_since`: how long since the last hook event from this session.
/// - `session_ended`: true if a SessionEnd event was received (remote path only;
///   local SessionEnd sessions are already removed before this function is called).
///
/// # Rules — local sessions
/// Existing behaviour is preserved exactly:
/// - consecutive dead checks ≥ threshold → Remove
/// - dead PID + idle ≥ IDLE_TIMEOUT → Remove
/// - otherwise → Keep
///
/// # Rules — remote sessions
/// PID probing is skipped (the PID belongs to a different machine).
/// - `session_ended` → Remove (SessionEnd from the remote agent propagated via tunnel)
/// - tunnel `Down` → Freeze (stale; keep in table)
/// - tunnel `Connected` or `Reconnecting` + long idle → ToIdle
/// - tunnel `Connected` or `Reconnecting` + recent activity → Keep
pub fn decide(
    is_local: bool,
    tunnel: TunnelState,
    consecutive_dead: u32,
    idle_since: Duration,
    session_ended: bool,
) -> LivenessAction {
    if is_local {
        // ── Local path: preserve existing liveness semantics exactly ──────────
        // (session_ended on the local path is handled synchronously in the event
        //  path before the liveness thread runs — so we never see it here, but
        //  keep the guard for correctness.)
        if session_ended {
            return LivenessAction::Remove;
        }
        if consecutive_dead >= DEAD_CHECK_THRESHOLD {
            return LivenessAction::Remove;
        }
        if consecutive_dead > 0 && idle_since >= IDLE_TIMEOUT {
            return LivenessAction::Remove;
        }
        LivenessAction::Keep
    } else {
        // ── Remote path: tunnel-aware decisions; skip PID probing ─────────────
        if session_ended {
            return LivenessAction::Remove;
        }
        match tunnel {
            TunnelState::Down => LivenessAction::Freeze,
            TunnelState::Connected | TunnelState::Reconnecting => {
                if idle_since >= REMOTE_IDLE_TIMEOUT {
                    LivenessAction::ToIdle
                } else {
                    LivenessAction::Keep
                }
            }
        }
    }
}

/// Number of consecutive `kill -0` failures before a session is removed.
pub const DEAD_CHECK_THRESHOLD: u32 = 3;

/// Maximum time without any event after which a session with a dead pid is removed.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60); // 5 minutes

/// Determine whether a session should be removed.
///
/// Parameters:
/// - `now`: the current instant.
/// - `last_event`: when the last hook event was received.
/// - `consecutive_dead_checks`: how many consecutive `kill -0` failures occurred.
/// - `ended_flag`: true if a SessionEnd event was received.
///
/// Returns true if the session should be removed from the state table.
pub fn should_remove(
    now: Instant,
    last_event: Instant,
    consecutive_dead_checks: u32,
    ended_flag: bool,
) -> bool {
    // Rule 1: explicit SessionEnd.
    if ended_flag {
        return true;
    }

    // Rule 2: consecutive liveness failures reach threshold.
    if consecutive_dead_checks >= DEAD_CHECK_THRESHOLD {
        return true;
    }

    // Rule 3: no event for IDLE_TIMEOUT and the process is not alive
    // (consecutive_dead_checks > 0 means at least one failure).
    if consecutive_dead_checks > 0 && now.duration_since(last_event) >= IDLE_TIMEOUT {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Legacy should_remove tests (zero regression) ──────────────────────────

    #[test]
    fn ended_flag_removes_immediately() {
        let now = Instant::now();
        assert!(should_remove(now, now, 0, true));
    }

    #[test]
    fn consecutive_failures_at_threshold_removes() {
        let now = Instant::now();
        assert!(should_remove(now, now, DEAD_CHECK_THRESHOLD, false));
    }

    #[test]
    fn consecutive_failures_below_threshold_does_not_remove() {
        let now = Instant::now();
        assert!(!should_remove(now, now, DEAD_CHECK_THRESHOLD - 1, false));
    }

    #[test]
    fn timeout_with_dead_pid_removes() {
        let now = Instant::now();
        // last event was longer ago than IDLE_TIMEOUT
        let last_event = now - IDLE_TIMEOUT - Duration::from_secs(1);
        // at least one dead check
        assert!(should_remove(now, last_event, 1, false));
    }

    #[test]
    fn timeout_but_process_alive_does_not_remove() {
        let now = Instant::now();
        let last_event = now - IDLE_TIMEOUT - Duration::from_secs(1);
        // zero dead checks → process seems alive
        assert!(!should_remove(now, last_event, 0, false));
    }

    #[test]
    fn alive_process_with_recent_event_not_removed() {
        let now = Instant::now();
        let last_event = now - Duration::from_secs(1);
        assert!(!should_remove(now, last_event, 0, false));
    }

    #[test]
    fn zero_consecutive_dead_and_not_ended_not_removed() {
        let now = Instant::now();
        // last event was very long ago but process seems alive (0 dead checks)
        let last_event = now - Duration::from_secs(3600);
        assert!(!should_remove(now, last_event, 0, false));
    }

    // ── decide(): local session tests (zero regression) ───────────────────────

    /// Local session: process alive (consecutive_dead=0), recent activity → Keep.
    #[test]
    fn local_alive_recent_is_keep() {
        let action = decide(true, TunnelState::Connected, 0, Duration::from_secs(1), false);
        assert_eq!(action, LivenessAction::Keep);
    }

    /// Local session: consecutive dead checks reach threshold → Remove.
    #[test]
    fn local_dead_threshold_is_remove() {
        let action = decide(
            true,
            TunnelState::Connected,
            DEAD_CHECK_THRESHOLD,
            Duration::from_secs(1),
            false,
        );
        assert_eq!(action, LivenessAction::Remove);
    }

    /// Local session: below threshold AND below idle timeout → Keep.
    #[test]
    fn local_below_threshold_is_keep() {
        let action = decide(
            true,
            TunnelState::Connected,
            DEAD_CHECK_THRESHOLD - 1,
            // below the IDLE_TIMEOUT so Rule 3 (dead+idle) does not fire
            Duration::from_secs(60),
            false,
        );
        assert_eq!(action, LivenessAction::Keep);
    }

    /// Local session: one dead check + idle beyond timeout → Remove.
    #[test]
    fn local_dead_plus_idle_timeout_is_remove() {
        let action = decide(
            true,
            TunnelState::Connected,
            1,
            IDLE_TIMEOUT + Duration::from_secs(1),
            false,
        );
        assert_eq!(action, LivenessAction::Remove);
    }

    /// Local session: alive process (dead=0) + very long idle → Keep (no PID kill-0 failure).
    #[test]
    fn local_alive_long_idle_is_keep() {
        let action = decide(
            true,
            TunnelState::Connected,
            0,
            Duration::from_secs(3600),
            false,
        );
        assert_eq!(action, LivenessAction::Keep);
    }

    /// Local session: session_ended flag → Remove.
    #[test]
    fn local_session_ended_is_remove() {
        let action = decide(true, TunnelState::Connected, 0, Duration::ZERO, true);
        assert_eq!(action, LivenessAction::Remove);
    }

    // ── decide(): remote session tests ────────────────────────────────────────

    /// Remote + tunnel Down → Freeze (stale; do NOT remove).
    #[test]
    fn remote_tunnel_down_is_freeze() {
        let action = decide(false, TunnelState::Down, 0, Duration::from_secs(1), false);
        assert_eq!(action, LivenessAction::Freeze);
    }

    /// Remote + tunnel Down → Freeze even with long idle (no remove).
    #[test]
    fn remote_tunnel_down_long_idle_is_still_freeze() {
        let action = decide(false, TunnelState::Down, 0, Duration::from_secs(3600), false);
        assert_eq!(action, LivenessAction::Freeze);
    }

    /// Remote + tunnel Connected + SessionEnd → Remove.
    #[test]
    fn remote_connected_session_ended_is_remove() {
        let action = decide(false, TunnelState::Connected, 0, Duration::ZERO, true);
        assert_eq!(action, LivenessAction::Remove);
    }

    /// Remote + tunnel Reconnecting + SessionEnd → Remove.
    #[test]
    fn remote_reconnecting_session_ended_is_remove() {
        let action = decide(false, TunnelState::Reconnecting, 0, Duration::ZERO, true);
        assert_eq!(action, LivenessAction::Remove);
    }

    /// Remote + tunnel Connected + recent activity → Keep.
    #[test]
    fn remote_connected_recent_is_keep() {
        let action = decide(
            false,
            TunnelState::Connected,
            0,
            Duration::from_secs(30),
            false,
        );
        assert_eq!(action, LivenessAction::Keep);
    }

    /// Remote + tunnel Reconnecting + recent activity → Keep (not Freeze; tunnel may recover).
    #[test]
    fn remote_reconnecting_recent_is_keep() {
        let action = decide(
            false,
            TunnelState::Reconnecting,
            0,
            Duration::from_secs(30),
            false,
        );
        assert_eq!(action, LivenessAction::Keep);
    }

    /// Remote + tunnel Connected + long idle → ToIdle (not Remove).
    #[test]
    fn remote_connected_long_idle_is_to_idle() {
        let action = decide(
            false,
            TunnelState::Connected,
            0,
            REMOTE_IDLE_TIMEOUT + Duration::from_secs(1),
            false,
        );
        assert_eq!(action, LivenessAction::ToIdle);
    }

    /// Remote + tunnel Reconnecting + long idle → ToIdle.
    #[test]
    fn remote_reconnecting_long_idle_is_to_idle() {
        let action = decide(
            false,
            TunnelState::Reconnecting,
            0,
            REMOTE_IDLE_TIMEOUT + Duration::from_secs(1),
            false,
        );
        assert_eq!(action, LivenessAction::ToIdle);
    }

    /// Remote: PID probe (consecutive_dead) is irrelevant — Down tunnel wins.
    #[test]
    fn remote_skips_pid_probe_tunnel_down_wins() {
        // Even with many "dead" checks, tunnel=Down → Freeze, not Remove.
        let action = decide(
            false,
            TunnelState::Down,
            DEAD_CHECK_THRESHOLD * 10,
            Duration::from_secs(1),
            false,
        );
        assert_eq!(action, LivenessAction::Freeze, "remote must not use pid-probe threshold");
    }

    /// Remote: PID probe (consecutive_dead) is irrelevant — tunnel Connected wins.
    #[test]
    fn remote_skips_pid_probe_connected_wins() {
        // High consecutive_dead but tunnel=Connected, recent → Keep.
        let action = decide(
            false,
            TunnelState::Connected,
            DEAD_CHECK_THRESHOLD * 10,
            Duration::from_secs(30),
            false,
        );
        assert_eq!(action, LivenessAction::Keep, "remote must not use pid-probe threshold");
    }

    /// Remote + tunnel Down + SessionEnd → Remove (session explicitly ended).
    #[test]
    fn remote_down_session_ended_is_remove() {
        let action = decide(false, TunnelState::Down, 0, Duration::ZERO, true);
        assert_eq!(action, LivenessAction::Remove);
    }
}
