use crate::commands::run_checked;
use manager_core::{
    capacity::Resources,
    host::{PortBinding, Transport},
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
    io,
    path::Path,
    time::Duration,
};

#[derive(Debug)]
pub struct Inventory {
    pub legacy_reserved: Resources,
    pub unavailable_bindings: Vec<PortBinding>,
    pub container_ids: HashSet<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Container {
    id: String,
    memory_bytes: i64,
    nano_cpus: i64,
    cpu_quota: i64,
    cpu_period: i64,
    configured_ports: Option<BTreeMap<String, Option<Vec<DockerBinding>>>>,
    active_ports: Option<BTreeMap<String, Option<Vec<DockerBinding>>>>,
}

#[derive(Deserialize)]
struct DockerBinding {
    #[serde(rename = "HostIp")]
    ip: String,
    #[serde(rename = "HostPort")]
    port: String,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid Docker inventory")
}

pub fn parse_inventory(
    bytes: &[u8],
    owned_ids: &HashSet<String>,
    physical_memory: u64,
) -> io::Result<Inventory> {
    let mut legacy_reserved = Resources {
        memory_bytes: 0,
        disk_bytes: 0,
        cpu_millis: 0,
    };
    let mut bindings = HashSet::new();
    let mut seen = HashSet::new();
    for container in serde_json::Deserializer::from_slice(bytes).into_iter::<Container>() {
        let container = container.map_err(|_| invalid())?;
        if !valid_container_id(&container.id)
            || !seen.insert(container.id.clone())
            || container.memory_bytes < 0
            || container.nano_cpus < 0
            || container.cpu_quota < -1
            || container.cpu_period < 0
        {
            return Err(invalid());
        }
        if !owned_ids.contains(&container.id) {
            let memory = if container.memory_bytes == 0 {
                physical_memory
            } else {
                container.memory_bytes as u64
            };
            let cpu = if container.nano_cpus > 0 {
                (container.nano_cpus as u64).div_ceil(1_000_000)
            } else if container.cpu_quota > 0 {
                if container.cpu_period == 0 {
                    return Err(invalid());
                }
                (container.cpu_quota as u64)
                    .checked_mul(1000)
                    .ok_or_else(invalid)?
                    .div_ceil(container.cpu_period as u64)
            } else {
                // Uncapped CPU is shared rather than reserved: overload slows games down but,
                // unlike memory, never kills them. Explicit legacy quotas still count.
                0
            };
            legacy_reserved.memory_bytes = legacy_reserved
                .memory_bytes
                .checked_add(memory)
                .ok_or_else(invalid)?;
            legacy_reserved.cpu_millis = legacy_reserved
                .cpu_millis
                .checked_add(u32::try_from(cpu).map_err(|_| invalid())?)
                .ok_or_else(invalid)?;
        }
        for mapping in [container.configured_ports, container.active_ports]
            .into_iter()
            .flatten()
        {
            for (key, published) in mapping {
                let (_, transport) = key.rsplit_once('/').ok_or_else(invalid)?;
                let transport = match transport {
                    "tcp" => Transport::Tcp,
                    "udp" => Transport::Udp,
                    "sctp" => continue,
                    _ => return Err(invalid()),
                };
                for published in published.into_iter().flatten() {
                    if published.port.is_empty() || published.port == "0" {
                        continue;
                    }
                    let port = published.port.parse::<u16>().map_err(|_| invalid())?;
                    let ip = if published.ip.is_empty() {
                        "0.0.0.0"
                    } else {
                        &published.ip
                    };
                    bindings.insert(PortBinding {
                        ip: ip.parse().map_err(|_| invalid())?,
                        port,
                        transport,
                    });
                }
            }
        }
    }
    // Disk admission requires a separately qualified quota pool; Docker limits do not establish it.
    Ok(Inventory {
        legacy_reserved,
        unavailable_bindings: bindings.into_iter().collect(),
        container_ids: seen,
    })
}

pub fn valid_container_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub async fn inspect_inventory(
    config_dir: &Path,
    owned_ids: &HashSet<String>,
    physical_memory: u64,
) -> io::Result<Inventory> {
    let prefix = [
        OsString::from("--host"),
        OsString::from("unix:///var/run/docker.sock"),
        OsString::from("--config"),
        config_dir.as_os_str().to_owned(),
    ];
    let mut arguments = prefix.to_vec();
    arguments.extend(["ps", "--all", "--quiet", "--no-trunc"].map(OsString::from));
    let listed = run_checked(
        Path::new("/usr/bin/docker"),
        &arguments,
        Duration::from_secs(10),
        64 * 1024,
    )
    .await?;
    let listed = std::str::from_utf8(&listed).map_err(|_| invalid())?;
    let ids: Vec<_> = listed.split_whitespace().collect();
    if ids.iter().any(|id| !valid_container_id(id)) {
        return Err(invalid());
    }
    if ids.is_empty() {
        return parse_inventory(b"", owned_ids, physical_memory);
    }
    const FORMAT: &str = r#"{"id":{{json .Id}},"memory_bytes":{{json .HostConfig.Memory}},"nano_cpus":{{json .HostConfig.NanoCpus}},"cpu_quota":{{json .HostConfig.CpuQuota}},"cpu_period":{{json .HostConfig.CpuPeriod}},"configured_ports":{{json .HostConfig.PortBindings}},"active_ports":{{json .NetworkSettings.Ports}}}"#;
    let mut arguments = prefix.to_vec();
    arguments.extend(["inspect", "--type=container", "--format", FORMAT, "--"].map(OsString::from));
    arguments.extend(ids.iter().map(OsString::from));
    let inspected = run_checked(
        Path::new("/usr/bin/docker"),
        &arguments,
        Duration::from_secs(10),
        1024 * 1024,
    )
    .await?;
    let inventory = parse_inventory(&inspected, owned_ids, physical_memory)?;
    if inventory.container_ids != ids.into_iter().map(str::to_owned).collect() {
        return Err(invalid());
    }
    Ok(inventory)
}
