#![cfg(target_os = "linux")]

use manager_agent::metrics::{parse_stats, rate};

#[test]
fn cpu_is_cores_used_and_memory_excludes_reclaimable_cache() {
    let stats = br#"{
        "read": "2026-10-28T00:41:07.351822353Z", "preread": "2026-10-28T00:41:06.351822353Z",
        "cpu_stats": {"cpu_usage": {"total_usage": 2500000000}},
        "precpu_stats": {"cpu_usage": {"total_usage": 1000000000}},
        "memory_stats": {"usage": 900, "limit": 3000, "stats": {"inactive_file": 100}},
        "networks": {"eth0": {"rx_bytes": 10, "tx_bytes": 20}, "eth1": {"rx_bytes": 5, "tx_bytes": 5}}
    }"#;
    let sample = parse_stats(stats).unwrap();
    assert_eq!(sample.cpu_millis, 1500, "1.5 CPUs over one second");
    assert_eq!(
        (sample.memory_bytes, sample.memory_limit_bytes),
        (800, 3000)
    );
    assert_eq!((sample.rx_bytes, sample.tx_bytes), (15, 25));
}

#[test]
fn first_samples_and_restarts_never_report_nonsense() {
    // A container's first sample has no previous reading.
    let first = br#"{"read": "2026-10-28T00:41:07Z", "preread": "0001-01-01T00:00:00Z",
        "cpu_stats": {"cpu_usage": {"total_usage": 5}}, "precpu_stats": {"cpu_usage": {"total_usage": 0}},
        "memory_stats": {}}"#;
    assert_eq!(parse_stats(first).unwrap().cpu_millis, 0);
    assert_eq!(rate(1000, 3000, 2), 1000);
    assert_eq!(rate(3000, 1000, 2), 0, "counters reset by a restart");
    assert_eq!(rate(0, 1000, 0), 0);
    assert!(parse_stats(b"not json").is_err());
}
