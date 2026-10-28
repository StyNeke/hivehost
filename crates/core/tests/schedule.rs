use manager_core::schedule::Recurrence;

fn at(hour: u8, minute: u8, weekdays: u8, timezone: &str) -> Recurrence {
    Recurrence {
        hour,
        minute,
        weekdays,
        timezone: timezone.into(),
    }
}

// 2026-03-28 is a Saturday; Europe/Amsterdam moves to summer time on Sunday 2026-03-29 02:00.
const SAT_2026_03_28_UTC_NOON: i64 = 1_774_699_200;

#[test]
fn next_run_follows_weekdays_and_the_zone() {
    let daily = at(4, 30, 0x7f, "Europe/Amsterdam");
    // Saturday 04:30 CET has passed at noon, so Sunday 04:30 CEST (02:30 UTC).
    let next = daily.next_after(SAT_2026_03_28_UTC_NOON).unwrap();
    assert_eq!(next, SAT_2026_03_28_UTC_NOON + 14 * 3600 + 30 * 60);
    // Only Mondays.
    let monday = at(4, 30, 1, "Europe/Amsterdam");
    assert_eq!(
        monday.next_after(SAT_2026_03_28_UTC_NOON).unwrap(),
        next + 24 * 3600
    );
    // The same wall time in UTC differs by the zone offset.
    let utc = at(4, 30, 0x7f, "UTC")
        .next_after(SAT_2026_03_28_UTC_NOON)
        .unwrap();
    assert_eq!(utc - next, 2 * 3600);
    // Runs are strictly after the previous one, so a run is never repeated.
    assert!(daily.next_after(next).unwrap() > next);
}

#[test]
fn skipped_and_repeated_local_times_run_once() {
    // 02:30 does not exist on 2026-03-29 in Amsterdam; it runs at 03:30 CEST (01:30 UTC).
    let gap = at(2, 30, 0x7f, "Europe/Amsterdam");
    let first = gap.next_after(SAT_2026_03_28_UTC_NOON).unwrap();
    assert_eq!(first, SAT_2026_03_28_UTC_NOON + 13 * 3600 + 30 * 60);
    // 02:30 happens twice on 2026-10-25 (end of summer time); only the first one runs.
    let repeated = at(2, 30, 0x7f, "Europe/Amsterdam");
    let before = 1_792_879_200; // 2026-10-24 22:00 UTC (00:00 local)
    let run = repeated.next_after(before).unwrap();
    assert_eq!(run, 1_792_888_200, "02:30 summer time = 00:30 UTC");
    // Not again at 02:30 winter time an hour later, but the next day (25 hours on this day).
    assert_eq!(repeated.next_after(run).unwrap(), run + 25 * 3600);
}

#[test]
fn invalid_recurrences_are_rejected() {
    assert!(at(24, 0, 1, "UTC").validate().is_err());
    assert!(at(1, 60, 1, "UTC").validate().is_err());
    assert!(at(1, 0, 0, "UTC").validate().is_err());
    assert!(at(1, 0, 0x80, "UTC").validate().is_err());
    assert!(at(1, 0, 1, "Mars/Olympus").validate().is_err());
    assert!(at(1, 0, 1, "../../etc/passwd").validate().is_err());
    assert!(at(1, 0, 1, "Europe/Amsterdam").validate().is_ok());
}
