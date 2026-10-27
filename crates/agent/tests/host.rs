use manager_agent::host::{configured_local_ips, parse_disk_space, parse_memory_total};
use std::net::IpAddr;

#[test]
fn hardware_parsing_is_explicit_about_units_and_invalid_data() {
    assert_eq!(
        parse_memory_total("MemTotal:       24487628 kB\nMemFree: 100 kB\n").unwrap(),
        24487628 * 1024
    );
    assert!(parse_memory_total("MemFree: 100 kB\n").is_err());
    assert!(parse_memory_total("MemTotal: 100 MB\n").is_err());
    assert!(parse_memory_total("MemTotal: 18446744073709551615 kB\n").is_err());
    assert_eq!(
        parse_disk_space("1B-blocks Avail\n110441844736 76898758656\n").unwrap(),
        (110441844736, 76898758656)
    );
    assert!(parse_disk_space("1B-blocks Avail\n10 20\n").is_err());
    assert!(parse_disk_space("not filesystem data").is_err());
}

#[test]
fn only_configured_private_addresses_actually_on_the_host_are_allocatable() {
    let addresses = br#"[{"ifname":"nordlynx","addr_info":[{"family":"inet","local":"100.116.183.201"}]},{"ifname":"eno1","addr_info":[{"family":"inet6","local":"2001:db8::1"},{"family":"inet","local":"192.168.178.90"}]}]"#;
    let allowed: IpAddr = "100.116.183.201".parse().unwrap();
    let absent: IpAddr = "100.116.183.202".parse().unwrap();
    let public: IpAddr = "2001:db8::1".parse().unwrap();
    assert_eq!(
        configured_local_ips(addresses, &[allowed, absent, public]).unwrap(),
        vec![allowed]
    );
    assert!(configured_local_ips(addresses, &[]).unwrap().is_empty());
    assert!(configured_local_ips(b"not JSON", &[allowed]).is_err());
}
