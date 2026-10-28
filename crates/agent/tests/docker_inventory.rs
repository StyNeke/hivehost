use manager_agent::docker::parse_inventory;
use manager_core::host::Transport;
use serde_json::json;
use std::collections::HashSet;

fn entry(id: &str) -> serde_json::Value {
    json!({"id":id,"memory_bytes":2048,"nano_cpus":1500000000_i64,"cpu_quota":0,"cpu_period":0,"configured_ports":{"25565/tcp":[{"HostIp":"100.116.183.201","HostPort":"25600"}]},"active_ports":null})
}

#[test]
fn legacy_limits_and_stopped_container_allocations_are_preserved() {
    let id = "a".repeat(64);
    let encoded = serde_json::to_vec(&entry(&id)).unwrap();
    let inventory = parse_inventory(&encoded, &HashSet::new(), 16000).unwrap();
    assert_eq!(inventory.legacy_reserved.memory_bytes, 2048);
    assert_eq!(inventory.legacy_reserved.cpu_millis, 1500);
    assert_eq!(inventory.unavailable_bindings[0].port, 25600);
    assert_eq!(inventory.unavailable_bindings[0].transport, Transport::Tcp);
    let owned = HashSet::from([id]);
    assert_eq!(
        parse_inventory(&encoded, &owned, 16000)
            .unwrap()
            .legacy_reserved
            .memory_bytes,
        0
    );
}

#[test]
fn unlimited_legacy_memory_is_reserved_but_uncapped_cpu_is_shared() {
    let mut value = entry(&"b".repeat(64));
    value["memory_bytes"] = json!(0);
    value["nano_cpus"] = json!(0);
    let inventory =
        parse_inventory(&serde_json::to_vec(&value).unwrap(), &HashSet::new(), 16000).unwrap();
    assert_eq!(inventory.legacy_reserved.memory_bytes, 16000);
    // Uncapped CPU is shared: overload slows games down but never kills them, unlike memory.
    assert_eq!(inventory.legacy_reserved.cpu_millis, 0);
    value["cpu_quota"] = json!(150001);
    value["cpu_period"] = json!(100000);
    assert_eq!(
        parse_inventory(&serde_json::to_vec(&value).unwrap(), &HashSet::new(), 16000)
            .unwrap()
            .legacy_reserved
            .cpu_millis,
        1501
    );
}

#[test]
fn malformed_inventory_and_negative_limits_fail_closed() {
    assert!(parse_inventory(b"not JSON", &HashSet::new(), 16000).is_err());
    let mut value = entry("--other-host");
    assert!(parse_inventory(&serde_json::to_vec(&value).unwrap(), &HashSet::new(), 16000).is_err());
    value = entry(&"c".repeat(64));
    value["memory_bytes"] = json!(-1);
    assert!(parse_inventory(&serde_json::to_vec(&value).unwrap(), &HashSet::new(), 16000).is_err());
}
