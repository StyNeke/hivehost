use crate::{capacity::Resources, model::EggRevisionId};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortBinding {
    pub ip: IpAddr,
    pub port: u16,
    pub transport: Transport,
}

// Produced by the host agent, never accepted as browser creation input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostAdmission {
    pub measured_at: i64,
    pub budget: Resources,
    pub legacy_reserved: Resources,
    pub disk_quotas_ready: bool,
    pub qualified_revisions: Vec<EggRevisionId>,
    pub private_ips: Vec<IpAddr>,
    pub unavailable_bindings: Vec<PortBinding>,
}

pub const MAX_SNAPSHOT_AGE_SECS: i64 = 30;

impl HostAdmission {
    // Future timestamps are as untrustworthy as old ones: both mean clocks or data are wrong.
    pub fn is_fresh(&self, now: i64) -> bool {
        self.measured_at >= 0
            && now
                .checked_sub(self.measured_at)
                .is_some_and(|age| (0..=MAX_SNAPSHOT_AGE_SECS).contains(&age))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostHardware {
    pub cpu_millis: u32,
    pub memory_bytes: u64,
    pub disk_total_bytes: u64,
    pub disk_available_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostStatus {
    pub hardware: HostHardware,
    pub admission: HostAdmission,
    pub blockers: Vec<String>,
}

pub fn private_allocation_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || (ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
        }
        IpAddr::V6(ip) => ip.is_unique_local() || ip.is_loopback(),
    }
}
