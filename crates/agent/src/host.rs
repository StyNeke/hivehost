use serde::Deserialize;
use std::{io, net::IpAddr, path::PathBuf};

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub data_root: PathBuf,
    pub private_ips: Vec<IpAddr>,
    // Each server gets its own UID, starting here; keep the range clear of real accounts.
    pub uid_base: u32,
    // Optional HTTP(S) proxy for installer/game containers on hosts with filtered egress.
    pub container_proxy: Option<(String, u16)>,
    // Optional PEM bundle and Java truststore for proxies that re-sign TLS, mounted read-only.
    pub container_ca_file: Option<PathBuf>,
    pub container_java_truststore: Option<PathBuf>,
    pub disk_reserve_bytes: u64,
    pub network_name: String,
    pub network_subnet: String,
    pub stop_grace_secs: u64,
    pub docker_socket: PathBuf,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            data_root: "/var/lib/game-manager".into(),
            private_ips: vec![],
            uid_base: 200_000,
            container_proxy: None,
            container_ca_file: None,
            container_java_truststore: None,
            disk_reserve_bytes: 10 * 1024 * 1024 * 1024,
            network_name: "gm-games".into(),
            network_subnet: "172.30.250.0/24".into(),
            stop_grace_secs: 120,
            docker_socket: "/var/run/docker.sock".into(),
        }
    }
}

// Accepts http://host:port only; credentials or paths in the proxy URL are refused.
pub fn parse_proxy(value: &str) -> Result<(String, u16), &'static str> {
    let rest = value
        .strip_prefix("http://")
        .ok_or("GM_CONTAINER_PROXY must be http://host:port")?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let (host, port) = rest
        .rsplit_once(':')
        .ok_or("GM_CONTAINER_PROXY must include a port")?;
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return Err("GM_CONTAINER_PROXY host is invalid");
    }
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|p| *p != 0)
        .ok_or("GM_CONTAINER_PROXY port is invalid")?;
    Ok((host.to_owned(), port))
}

// Containers count as this application's own only when their label names a server in the
// agent's registry; a foreign container carrying the label is still treated as legacy.
#[cfg(target_os = "linux")]
async fn owned_container_ids(
    docker_config: &std::path::Path,
    owned: &std::collections::HashSet<uuid::Uuid>,
) -> io::Result<std::collections::HashSet<String>> {
    use std::ffi::OsString;
    let mut ids = std::collections::HashSet::new();
    for label in [crate::runtime::SERVER_LABEL, crate::runtime::INSTALL_LABEL] {
        let output = crate::commands::run_checked(
            std::path::Path::new("/usr/bin/docker"),
            &[
                OsString::from("--host"),
                OsString::from("unix:///var/run/docker.sock"),
                OsString::from("--config"),
                docker_config.as_os_str().to_owned(),
                OsString::from("ps"),
                OsString::from("--all"),
                OsString::from("--no-trunc"),
                OsString::from("--filter"),
                OsString::from(format!("label={label}")),
                OsString::from("--format"),
                OsString::from(format!("{{{{.ID}}}} {{{{.Label \"{label}\"}}}}")),
            ],
            std::time::Duration::from_secs(10),
            256 * 1024,
        )
        .await?;
        for line in String::from_utf8_lossy(&output).lines() {
            if let Some((id, server)) = line.split_once(' ')
                && crate::docker::valid_container_id(id)
                && server
                    .parse::<uuid::Uuid>()
                    .is_ok_and(|server| owned.contains(&server))
            {
                ids.insert(id.to_owned());
            }
        }
    }
    Ok(ids)
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

// `owned` lists this agent's server IDs and `owned_image_bytes` the space their storage images
// already hold; both come from the agent's registry, never from container metadata alone.
pub async fn inspect_host(
    config: &AgentConfig,
    owned: &std::collections::HashSet<uuid::Uuid>,
    owned_image_bytes: u64,
) -> io::Result<manager_core::host::HostStatus> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config, owned, owned_image_bytes);
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
        let owned_ids = owned_container_ids(&docker_config, owned).await?;
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
        // SAFETY: geteuid has no preconditions.
        let disk_quotas_ready = unsafe { libc::geteuid() } == 0
            && crate::storage::quota_tools_available()
            && disk_root == config.data_root.as_path();
        let budget = Resources {
            memory_bytes: memory.saturating_sub(4 * 1024 * 1024 * 1024),
            cpu_millis: cpu.saturating_sub(1000),
            disk_bytes: if disk_quotas_ready {
                crate::storage::disk_budget(
                    disk_available_bytes,
                    config.disk_reserve_bytes,
                    owned_image_bytes,
                )
            } else {
                0
            },
        };
        let mut blockers: Vec<String> = vec![];
        if !disk_quotas_ready {
            blockers.push(
                "Per-server disk quotas are unavailable: the agent must run as root with e2fsprogs and an existing data directory".into(),
            );
        } else if budget.disk_bytes == 0 {
            blockers.push("No disk space is left above the configured reserve".into());
        }
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
                disk_quotas_ready,
                qualified_revisions: vec![],
                private_ips,
                unavailable_bindings: inventory.unavailable_bindings,
            },
            blockers,
        })
    }
}
