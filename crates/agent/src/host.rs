use serde::Deserialize;
use std::{io, net::IpAddr, path::PathBuf};

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub data_root: PathBuf,
    pub private_ips: Vec<IpAddr>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            data_root: "/var/lib/game-manager".into(),
            private_ips: vec![],
        }
    }
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid host measurement")
}

pub fn parse_memory_total(input: &str) -> io::Result<u64> {
    let mut totals = input
        .lines()
        .filter_map(|line| line.strip_prefix("MemTotal:"));
    let value = totals.next().ok_or_else(invalid)?;
    if totals.next().is_some() {
        return Err(invalid());
    }
    let fields: Vec<_> = value.split_whitespace().collect();
    if fields.len() != 2 || fields[1] != "kB" {
        return Err(invalid());
    }
    fields[0]
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(1024))
        .filter(|n| *n > 0)
        .ok_or_else(invalid)
}

pub fn parse_disk_space(input: &str) -> io::Result<(u64, u64)> {
    let rows: Vec<_> = input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if rows.len() != 2 {
        return Err(invalid());
    }
    let fields: Vec<_> = rows[1].split_whitespace().collect();
    if fields.len() != 2 {
        return Err(invalid());
    }
    let total = fields[0].parse::<u64>().map_err(|_| invalid())?;
    let available = fields[1].parse::<u64>().map_err(|_| invalid())?;
    if total == 0 || available > total {
        return Err(invalid());
    }
    Ok((total, available))
}

#[derive(Deserialize)]
struct Interface {
    addr_info: Vec<Address>,
}
#[derive(Deserialize)]
struct Address {
    family: String,
    local: String,
}

pub fn configured_local_ips(input: &[u8], configured: &[IpAddr]) -> io::Result<Vec<IpAddr>> {
    let interfaces: Vec<Interface> = serde_json::from_slice(input).map_err(|_| invalid())?;
    let mut actual = std::collections::HashSet::new();
    for interface in interfaces {
        for address in interface.addr_info {
            if address.family == "inet" || address.family == "inet6" {
                actual.insert(address.local.parse::<IpAddr>().map_err(|_| invalid())?);
            }
        }
    }
    Ok(configured
        .iter()
        .copied()
        .filter(|ip| manager_core::host::private_allocation_ip(*ip) && actual.contains(ip))
        .collect())
}

pub async fn inspect_host(config: &AgentConfig) -> io::Result<manager_core::host::HostStatus> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "host inspection requires Linux",
        ))
    }
    #[cfg(target_os = "linux")]
    {
        use crate::{commands::run_checked, docker::inspect_inventory};
        use manager_core::{
            capacity::Resources,
            host::{HostAdmission, HostHardware, HostStatus},
        };
        use std::{
            collections::HashSet,
            ffi::OsString,
            path::Path,
            time::{Duration, SystemTime, UNIX_EPOCH},
        };

        let memory = parse_memory_total(&tokio::fs::read_to_string("/proc/meminfo").await?)?;
        let cpu = u32::try_from(std::thread::available_parallelism()?.get())
            .ok()
            .and_then(|n| n.checked_mul(1000))
            .ok_or_else(invalid)?;
        let disk_root = if tokio::fs::metadata(&config.data_root)
            .await
            .is_ok_and(|m| m.is_dir())
        {
            config.data_root.as_path()
        } else {
            Path::new("/")
        };
        let mut disk_args = vec![
            OsString::from("-B1"),
            OsString::from("--output=size,avail"),
            OsString::from("--"),
        ];
        disk_args.push(disk_root.as_os_str().to_owned());
        let ip_args = [
            OsString::from("-j"),
            OsString::from("address"),
            OsString::from("show"),
        ];
        let docker_config = config.data_root.join("docker-client");
        // shortcut: all containers count as legacy until game operations add the ownership registry.
        let owned_ids = HashSet::new();
        let (disk, addresses, inventory) = tokio::try_join!(
            run_checked(
                Path::new("/usr/bin/df"),
                &disk_args,
                Duration::from_secs(5),
                4096
            ),
            run_checked(Path::new("ip"), &ip_args, Duration::from_secs(5), 64 * 1024),
            inspect_inventory(&docker_config, &owned_ids, memory, cpu),
        )?;
        let (disk_total_bytes, disk_available_bytes) =
            parse_disk_space(std::str::from_utf8(&disk).map_err(|_| invalid())?)?;
        let private_ips = configured_local_ips(&addresses, &config.private_ips)?;
        let measured_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_secs()).ok())
            .ok_or_else(invalid)?;
        let budget = Resources {
            memory_bytes: memory.saturating_sub(4 * 1024 * 1024 * 1024),
            cpu_millis: cpu.saturating_sub(1000),
            disk_bytes: 0,
        };
        let mut blockers = vec![
            "Storage quota setup has not been qualified".into(),
            "Game runtime installation has not been qualified".into(),
        ];
        if private_ips.is_empty() {
            blockers.push("No configured private IP is present on this host".into());
        }
        if inventory.legacy_reserved.memory_bytes > budget.memory_bytes {
            blockers.push("Existing container RAM limits exceed the safe game budget".into());
        }
        if inventory.legacy_reserved.cpu_millis > budget.cpu_millis {
            blockers.push("Existing container CPU limits exceed the safe game budget".into());
        }
        Ok(HostStatus {
            hardware: HostHardware {
                cpu_millis: cpu,
                memory_bytes: memory,
                disk_total_bytes,
                disk_available_bytes,
            },
            admission: HostAdmission {
                measured_at,
                budget,
                legacy_reserved: inventory.legacy_reserved,
                disk_quotas_ready: false,
                qualified_revisions: vec![],
                private_ips,
                unavailable_bindings: inventory.unavailable_bindings,
            },
            blockers,
        })
    }
}
