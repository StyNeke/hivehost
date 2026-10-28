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

#[test]
fn known_exit_causes_are_explained_and_deterministic_ones_are_not_retried() {
    use manager_core::lifecycle::explain_exit;
    let lines = |text: &str| text.lines().map(str::to_owned).collect::<Vec<_>>();
    let java = explain_exit(&lines(
        "Starting\nCaused by: java.lang.UnsupportedClassVersionError: net/minecraft/bundler/Main has been compiled by a more recent version of the Java Runtime (class file version 69.0), this version of the Java Runtime only recognizes class file versions up to 65.0\n\tat x",
    ))
    .unwrap();
    assert!(java.permanent);
    assert!(java.message.contains("Java 25"), "{}", java.message);
    assert!(
        explain_exit(&lines("**** FAILED TO BIND TO PORT!"))
            .unwrap()
            .permanent
    );
    let memory = explain_exit(&lines(
        "Exception in thread \"main\" java.lang.OutOfMemoryError: Java heap space",
    ))
    .unwrap();
    assert!(!memory.permanent);
    assert!(
        explain_exit(&lines("Error: Unable to access jarfile server.jar"))
            .unwrap()
            .permanent
    );
    assert_eq!(explain_exit(&lines("[Server] Stopping server\nexit")), None);
    assert_eq!(explain_exit(&[]), None);
}
