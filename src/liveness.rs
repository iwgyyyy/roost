use std::time::{Duration, Instant};

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
}
