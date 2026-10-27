use manager_core::lifecycle::{
    CrashDecision, MAX_RESTARTS, RESTART_WINDOW_SECS, on_unexpected_exit,
};

#[test]
fn crashes_restart_with_backoff_until_the_window_limit_then_give_up() {
    let mut history = Vec::new();
    assert_eq!(
        on_unexpected_exit(&mut history, 1000),
        CrashDecision::Restart { delay_secs: 10 }
    );
    assert_eq!(
        on_unexpected_exit(&mut history, 1030),
        CrashDecision::Restart { delay_secs: 20 }
    );
    assert_eq!(
        on_unexpected_exit(&mut history, 1060),
        CrashDecision::Restart { delay_secs: 40 }
    );
    assert_eq!(history.len(), MAX_RESTARTS as usize);
    assert_eq!(
        on_unexpected_exit(&mut history, 1090),
        CrashDecision::GiveUp
    );
}

#[test]
fn old_crashes_leave_the_window_and_clock_skew_cannot_grow_history() {
    let mut history = vec![0, 10, 20];
    assert_eq!(
        on_unexpected_exit(&mut history, 20 + RESTART_WINDOW_SECS + 1),
        CrashDecision::Restart { delay_secs: 10 }
    );
    assert_eq!(history.len(), 1);
    // A clock that moved backwards must not erase recent crashes or allow unbounded growth.
    let mut history = vec![5000, 5010, 5020];
    assert_eq!(on_unexpected_exit(&mut history, 100), CrashDecision::GiveUp);
    assert!(history.len() <= MAX_RESTARTS as usize + 1);
}
