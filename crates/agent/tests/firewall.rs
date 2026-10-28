#![cfg(target_os = "linux")]

use manager_agent::firewall::{
    FORWARD_CHAIN, INPUT_CHAIN, Isolation, chain_rules, default_gateway, jumps,
};

#[test]
fn containers_reach_only_the_proxy_on_the_host_and_no_private_networks() {
    let isolation = Isolation {
        subnet: "172.30.250.0/24".into(),
        gateway: "172.30.250.1".parse().unwrap(),
        gateway_tcp_ports: vec![3128],
    };
    let rules: Vec<String> = chain_rules(&isolation)
        .iter()
        .map(|r| r.join(" "))
        .collect();
    let input: Vec<&String> = rules
        .iter()
        .filter(|r| r.starts_with(&format!("-A {INPUT_CHAIN}")))
        .collect();
    assert_eq!(
        input,
        [
            "-A GM-ISOLATE-IN -m conntrack --ctstate ESTABLISHED,RELATED -j RETURN",
            "-A GM-ISOLATE-IN -d 172.30.250.1 -p tcp --dport 3128 -j RETURN",
            "-A GM-ISOLATE-IN -j DROP",
        ]
    );
    let forward: Vec<&String> = rules
        .iter()
        .filter(|r| r.starts_with(&format!("-A {FORWARD_CHAIN}")))
        .collect();
    // Replies to players come first, then private ranges are dropped, the rest continues.
    assert!(forward[0].contains("ESTABLISHED,RELATED -j RETURN"));
    for range in [
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "100.64.0.0/10",
    ] {
        assert!(
            forward
                .iter()
                .any(|r| r.contains(&format!("-d {range} -j DROP"))),
            "{range}"
        );
    }
    assert_eq!(
        forward.last().unwrap().as_str(),
        "-A GM-ISOLATE-FWD -j RETURN"
    );
    // Chains are flushed before being refilled, so re-applying never duplicates rules.
    let flush = rules.iter().position(|r| r == "-F GM-ISOLATE-IN").unwrap();
    assert!(
        rules
            .iter()
            .position(|r| r.starts_with("-A GM-ISOLATE-IN"))
            .unwrap()
            > flush
    );
    assert_eq!(
        jumps(&isolation)
            .iter()
            .map(|j| j.join(" "))
            .collect::<Vec<_>>(),
        [
            "INPUT -s 172.30.250.0/24 -j GM-ISOLATE-IN",
            "DOCKER-USER -s 172.30.250.0/24 -j GM-ISOLATE-FWD",
        ]
    );
    // Without a proxy on the gateway, nothing on the host is reachable.
    let closed = Isolation {
        gateway_tcp_ports: vec![],
        ..isolation
    };
    assert!(
        !chain_rules(&closed)
            .iter()
            .any(|r| r.contains(&"--dport".to_string()))
    );
}

#[test]
fn the_gateway_is_derived_when_docker_does_not_report_it() {
    assert_eq!(
        default_gateway("172.30.250.0/24"),
        Some("172.30.250.1".parse().unwrap())
    );
    assert_eq!(
        default_gateway("10.20.0.0/16"),
        Some("10.20.0.1".parse().unwrap())
    );
    assert_eq!(
        default_gateway("10.20.5.7/16"),
        Some("10.20.0.1".parse().unwrap())
    );
    for bad in ["", "172.30.250.0", "172.30.250.0/32", "fd00::/64", "x/24"] {
        assert_eq!(default_gateway(bad), None, "{bad}");
    }
}
