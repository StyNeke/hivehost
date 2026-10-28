// Network isolation for game and installer containers, which run untrusted egg scripts and
// server software. Traffic from the managed subnet may only reach the internet: the host itself
// (for example web panels or databases listening on the bridge gateway) and private, LAN, VPN
// and other Docker ranges are refused, except replies to connections players opened and the
// configured egress proxy. The rules live in two chains this agent owns, rebuilt on every start.
use std::{net::Ipv4Addr, process::Stdio};
use tokio::process::Command;

pub const INPUT_CHAIN: &str = "GM-ISOLATE-IN";
pub const FORWARD_CHAIN: &str = "GM-ISOLATE-FWD";

// Destinations games must not reach directly.
pub const PRIVATE_RANGES: [&str; 6] = [
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "100.64.0.0/10",
    "169.254.0.0/16",
    "127.0.0.0/8",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Isolation {
    pub subnet: String,
    pub gateway: Ipv4Addr,
    // TCP ports on the gateway containers may use (an egress proxy listening there).
    pub gateway_tcp_ports: Vec<u16>,
}

// Docker's default gateway for a subnet: its first host address. Older Docker releases leave the
// IPAM gateway field empty for networks created with only a subnet, so the agent derives it.
pub fn default_gateway(subnet: &str) -> Option<Ipv4Addr> {
    let (address, prefix) = subnet.split_once('/')?;
    let address: Ipv4Addr = address.parse().ok()?;
    let prefix: u32 = prefix.parse().ok()?;
    if !(1..=30).contains(&prefix) {
        return None;
    }
    let mask = u32::MAX << (32 - prefix);
    Some(Ipv4Addr::from((u32::from(address) & mask) + 1))
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

// Commands that rebuild both chains from scratch, in order. Creating an existing chain fails
// harmlessly; everything after it is deterministic.
pub fn chain_rules(isolation: &Isolation) -> Vec<Vec<String>> {
    let gateway = isolation.gateway.to_string();
    let mut rules = vec![];
    for chain in [INPUT_CHAIN, FORWARD_CHAIN] {
        rules.push(args(&["-N", chain]));
        rules.push(args(&["-F", chain]));
        rules.push(args(&[
            "-A",
            chain,
            "-m",
            "conntrack",
            "--ctstate",
            "ESTABLISHED,RELATED",
            "-j",
            "RETURN",
        ]));
    }
    for port in &isolation.gateway_tcp_ports {
        rules.push(args(&[
            "-A",
            INPUT_CHAIN,
            "-d",
            &gateway,
            "-p",
            "tcp",
            "--dport",
            &port.to_string(),
            "-j",
            "RETURN",
        ]));
    }
    rules.push(args(&["-A", INPUT_CHAIN, "-j", "DROP"]));
    for range in PRIVATE_RANGES {
        rules.push(args(&["-A", FORWARD_CHAIN, "-d", range, "-j", "DROP"]));
    }
    rules.push(args(&["-A", FORWARD_CHAIN, "-j", "RETURN"]));
    rules
}

// The jumps into the chains, limited to traffic from the managed subnet.
pub fn jumps(isolation: &Isolation) -> Vec<Vec<String>> {
    vec![
        args(&["INPUT", "-s", &isolation.subnet, "-j", INPUT_CHAIN]),
        args(&["DOCKER-USER", "-s", &isolation.subnet, "-j", FORWARD_CHAIN]),
    ]
}

async fn iptables(arguments: &[String]) -> std::io::Result<bool> {
    let status = Command::new("iptables")
        .args(["-w", "10"])
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    Ok(status.success())
}

pub async fn apply(isolation: &Isolation) -> std::io::Result<()> {
    for rule in chain_rules(isolation) {
        let created = iptables(&rule).await?;
        if !created && rule[0] != "-N" {
            return Err(std::io::Error::other(format!(
                "could not apply firewall rule: iptables {}",
                rule.join(" ")
            )));
        }
    }
    for jump in jumps(isolation) {
        let mut check = vec!["-C".to_string()];
        check.extend(jump.iter().cloned());
        if !iptables(&check).await? {
            let mut insert = vec!["-I".to_string()];
            insert.extend(jump.iter().cloned());
            if !iptables(&insert).await? {
                return Err(std::io::Error::other(format!(
                    "could not add firewall jump: iptables -I {}",
                    jump.join(" ")
                )));
            }
        }
    }
    Ok(())
}
