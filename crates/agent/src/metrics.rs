// Resource usage from Docker's stats API. CPU is expressed in millicores (1000 = one logical
// CPU, so 100% in the interface is one CPU); memory follows Docker's own convention of usage
// minus inactive page cache.
use serde::Deserialize;
use std::{collections::HashMap, io};

#[derive(Deserialize)]
struct Stats {
    read: String,
    preread: String,
    cpu_stats: Cpu,
    precpu_stats: Cpu,
    memory_stats: Memory,
    #[serde(default)]
    networks: HashMap<String, Network>,
}

#[derive(Deserialize)]
struct Cpu {
    cpu_usage: CpuUsage,
}

#[derive(Deserialize)]
struct CpuUsage {
    total_usage: u64,
}

#[derive(Deserialize, Default)]
struct Memory {
    #[serde(default)]
    usage: u64,
    #[serde(default)]
    limit: u64,
    #[serde(default)]
    stats: HashMap<String, u64>,
}

#[derive(Deserialize)]
struct Network {
    rx_bytes: u64,
    tx_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub cpu_millis: u32,
    pub memory_bytes: u64,
    pub memory_limit_bytes: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

fn nanos(timestamp: &str) -> Option<i128> {
    let seconds = crate::runtime::parse_docker_time(timestamp)? as i128;
    let fraction = timestamp
        .split_once('.')
        .map(|(_, rest)| rest.trim_end_matches('Z'))
        .unwrap_or("");
    let digits: String = fraction.chars().take(9).collect();
    let padded = format!("{digits:0<9}");
    Some(seconds * 1_000_000_000 + padded.parse::<i128>().ok()?)
}

pub fn parse_stats(bytes: &[u8]) -> io::Result<Sample> {
    let stats: Stats = serde_json::from_slice(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "unexpected stats"))?;
    let wall = match (nanos(&stats.read), nanos(&stats.preread)) {
        (Some(read), Some(preread)) if read > preread => read - preread,
        _ => 0,
    };
    let used = stats
        .cpu_stats
        .cpu_usage
        .total_usage
        .saturating_sub(stats.precpu_stats.cpu_usage.total_usage) as i128;
    let cpu_millis = if wall > 0 {
        (used * 1000 / wall).clamp(0, u32::MAX as i128) as u32
    } else {
        0
    };
    let cache = stats
        .memory_stats
        .stats
        .get("inactive_file")
        .or_else(|| stats.memory_stats.stats.get("total_inactive_file"))
        .copied()
        .unwrap_or(0);
    let (rx_bytes, tx_bytes) = stats.networks.values().fold((0u64, 0u64), |(rx, tx), n| {
        (rx.saturating_add(n.rx_bytes), tx.saturating_add(n.tx_bytes))
    });
    Ok(Sample {
        cpu_millis,
        memory_bytes: stats.memory_stats.usage.saturating_sub(cache),
        memory_limit_bytes: stats.memory_stats.limit,
        rx_bytes,
        tx_bytes,
    })
}

// Bytes per second between two counter readings; counters that went backwards (a restart)
// report zero instead of a huge number.
pub fn rate(previous: u64, current: u64, seconds: i64) -> u64 {
    if seconds <= 0 {
        return 0;
    }
    current.saturating_sub(previous) / seconds as u64
}
