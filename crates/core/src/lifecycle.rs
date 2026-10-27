// Crash restarts: at most MAX_RESTARTS within RESTART_WINDOW_SECS, then the server stays stopped.
// A requested stop, failed install or host shutdown never reaches this policy.
pub const MAX_RESTARTS: u32 = 3;
pub const RESTART_WINDOW_SECS: i64 = 600;
const BASE_DELAY_SECS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashDecision {
    Restart { delay_secs: u64 },
    GiveUp,
}

// `history` holds the times of restarts already granted and is persisted by the caller, so the
// limit survives agent restarts. Timestamps after `now` (a clock that went backwards) still count.
pub fn on_unexpected_exit(history: &mut Vec<i64>, now: i64) -> CrashDecision {
    history.retain(|at| now.saturating_sub(*at) < RESTART_WINDOW_SECS);
    if history.len() >= MAX_RESTARTS as usize {
        return CrashDecision::GiveUp;
    }
    history.push(now);
    CrashDecision::Restart {
        delay_secs: BASE_DELAY_SECS << (history.len() - 1),
    }
}
