#![cfg(target_os = "linux")]

use manager_agent::host::{AgentConfig, inspect_host};

#[tokio::test]
#[ignore = "explicit read-only qualification on a Linux host with Docker installed"]
async fn docker_and_hardware_probe_matches_the_actual_linux_host() {
    let directory = tempfile::tempdir().unwrap();
    let private_ips = std::env::var("GM_TEST_PRIVATE_IP")
        .ok()
        .map(|ip| vec![ip.parse().unwrap()])
        .unwrap_or_default();
    let config = AgentConfig {
        data_root: directory.path().to_owned(),
        private_ips,
    };
    let status = inspect_host(&config).await.unwrap();
    assert!(status.hardware.memory_bytes > 0);
    assert!(status.hardware.cpu_millis >= 1000);
    assert!(status.hardware.disk_available_bytes <= status.hardware.disk_total_bytes);
    assert!(!status.admission.disk_quotas_ready);
    assert!(!status.blockers.is_empty());
    if !config.private_ips.is_empty() {
        assert_eq!(status.admission.private_ips, config.private_ips);
    }
    println!("{}", serde_json::to_string(&status).unwrap());
}
