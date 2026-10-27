use manager_core::{
    capacity::{CapacityError, Resources, admit},
    eggs::{EggRevision, validate_variable},
    model::{EggRevisionId, JobId, ServerId, UserId},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};
use uuid::Uuid;

pub struct ProvisioningStore(Connection);

pub use manager_core::host::{HostAdmission, PortBinding, Transport};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateServer {
    pub name: String,
    pub egg_revision: EggRevisionId,
    pub limits: Resources,
    pub allocation_id: Uuid,
    pub image: String,
    pub variables: BTreeMap<String, String>,
    pub license_accepted: bool,
}

#[derive(Debug, Serialize)]
pub struct CreatedServer {
    pub id: ServerId,
    pub job_id: JobId,
    pub egg_revision: EggRevisionId,
    pub limits: Resources,
    pub binding: PortBinding,
}

#[derive(Debug, thiserror::Error)]
pub enum ProvisioningError {
    #[error("host is not ready: {0}")]
    HostNotReady(&'static str),
    #[error("invalid input: {0}")]
    InvalidInput(&'static str),
    #[error("variable validation failed: {0}")]
    Variable(String),
    #[error("requested resources exceed the safe budget")]
    Capacity(CapacityError),
    #[error("administrator access required")]
    Forbidden,
    #[error("allocation is unavailable")]
    AllocationUnavailable,
    #[error("egg revision not found")]
    NotFound,
    #[error("storage operation failed")]
    Storage(#[from] rusqlite::Error),
}

impl ProvisioningStore {
    pub fn open(path: &Path) -> Result<Self, ProvisioningError> {
        Ok(Self(crate::database::open(path)?))
    }

    pub fn add_allocation(
        &mut self,
        actor: UserId,
        binding: PortBinding,
        host: &HostAdmission,
        now: i64,
    ) -> Result<Uuid, ProvisioningError> {
        fresh_snapshot(host, now)?;
        let admin = self
            .0
            .query_row(
                "SELECT is_admin FROM users WHERE id = ?1",
                [actor.0.to_string()],
                |row| row.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false);
        if !admin {
            return Err(ProvisioningError::Forbidden);
        }
        check_binding(&binding, host)?;
        let id = Uuid::new_v4();
        let inserted = self.0.execute(
            "INSERT INTO allocation_pool VALUES (?1, ?2, ?3, ?4)",
            params![
                id.to_string(),
                binding.ip.to_string(),
                binding.port,
                transport_name(binding.transport)
            ],
        );
        match inserted {
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(ProvisioningError::AllocationUnavailable)
            }
            Err(error) => Err(error.into()),
            Ok(_) => Ok(id),
        }
    }

    pub fn create(
        &mut self,
        actor: UserId,
        request: CreateServer,
        host: &HostAdmission,
        now: i64,
    ) -> Result<CreatedServer, ProvisioningError> {
        fresh_snapshot(host, now)?;
        if !host.disk_quotas_ready {
            return Err(ProvisioningError::HostNotReady(
                "disk quota enforcement has not been qualified",
            ));
        }
        if !host.qualified_revisions.contains(&request.egg_revision) {
            return Err(ProvisioningError::HostNotReady(
                "egg runtime has not been qualified",
            ));
        }
        if request.name.trim().is_empty()
            || request.name.chars().count() > 80
            || request.name.chars().any(char::is_control)
        {
            return Err(ProvisioningError::InvalidInput(
                "server name must contain 1–80 printable characters",
            ));
        }
        if request.limits.memory_bytes > i64::MAX as u64
            || request.limits.disk_bytes > i64::MAX as u64
        {
            return Err(ProvisioningError::InvalidInput(
                "resource value is too large",
            ));
        }
        let transaction = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id = ?1)",
            [actor.0.to_string()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(ProvisioningError::Forbidden);
        }
        let encoded: String = transaction
            .query_row(
                "SELECT normalized FROM egg_revisions WHERE id = ?1",
                [request.egg_revision.0.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(ProvisioningError::NotFound)?;
        let revision: EggRevision = serde_json::from_str(&encoded)
            .map_err(|_| ProvisioningError::InvalidInput("stored egg revision is invalid"))?;
        if !revision
            .images
            .iter()
            .any(|(_, image)| image == &request.image)
        {
            return Err(ProvisioningError::InvalidInput(
                "choose an image from the pinned egg revision",
            ));
        }
        if revision.features.iter().any(|feature| feature == "eula") && !request.license_accepted {
            return Err(ProvisioningError::InvalidInput(
                "explicit game license acceptance is required",
            ));
        }
        let variables = resolve_variables(&revision, &request.variables)?;
        let record: (String, u16, String) = transaction
            .query_row(
                "SELECT host_ip, port, transport FROM allocation_pool WHERE id = ?1
             AND NOT EXISTS (SELECT 1 FROM server_allocations WHERE allocation_id = ?1)",
                [request.allocation_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
            .ok_or(ProvisioningError::AllocationUnavailable)?;
        let binding = PortBinding {
            ip: record
                .0
                .parse()
                .map_err(|_| ProvisioningError::AllocationUnavailable)?,
            port: record.1,
            transport: match record.2.as_str() {
                "tcp" => Transport::Tcp,
                "udp" => Transport::Udp,
                _ => return Err(ProvisioningError::AllocationUnavailable),
            },
        };
        check_binding(&binding, host)?;
        let incomplete: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM servers s WHERE NOT EXISTS (SELECT 1 FROM server_provisioning p WHERE p.server_id = s.id))", [], |row| row.get(0))?;
        if incomplete {
            return Err(ProvisioningError::HostNotReady(
                "existing server reservations need reconciliation",
            ));
        }
        let mut reserved = host.legacy_reserved;
        {
            let mut statement = transaction
                .prepare("SELECT memory_bytes, disk_bytes, cpu_millis FROM server_provisioning")?;
            let rows = statement.query_map([], |row| {
                Ok(Resources {
                    memory_bytes: u64::try_from(row.get::<_, i64>(0)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    disk_bytes: u64::try_from(row.get::<_, i64>(1)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    cpu_millis: row.get(2)?,
                })
            })?;
            for row in rows {
                reserved = add_reserved(reserved, row?)?;
            }
        }
        admit(request.limits, host.budget, reserved).map_err(ProvisioningError::Capacity)?;
        let id = ServerId(Uuid::new_v4());
        let job_id = JobId(Uuid::new_v4());
        transaction.execute(
            "INSERT INTO servers (id, name, owner_id) VALUES (?1, ?2, ?3)",
            params![id.0.to_string(), request.name.trim(), actor.0.to_string()],
        )?;
        transaction.execute(
            "INSERT INTO jobs (id, kind) VALUES (?1, 'install')",
            [job_id.0.to_string()],
        )?;
        transaction.execute("INSERT INTO server_provisioning (server_id, egg_revision_id, job_id, image, variables, memory_bytes, disk_bytes, cpu_millis) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id.0.to_string(), request.egg_revision.0.to_string(), job_id.0.to_string(), request.image,
                serde_json::to_string(&variables).map_err(|_| ProvisioningError::InvalidInput("invalid variables"))?, request.limits.memory_bytes as i64, request.limits.disk_bytes as i64, request.limits.cpu_millis])?;
        transaction.execute(
            "INSERT INTO server_allocations VALUES (?1, ?2, 1)",
            params![id.0.to_string(), request.allocation_id.to_string()],
        )?;
        transaction.commit()?;
        Ok(CreatedServer {
            id,
            job_id,
            egg_revision: request.egg_revision,
            limits: request.limits,
            binding,
        })
    }
}

fn fresh_snapshot(host: &HostAdmission, now: i64) -> Result<(), ProvisioningError> {
    if host.measured_at < 0
        || now
            .checked_sub(host.measured_at)
            .is_none_or(|age| !(0..=30).contains(&age))
    {
        return Err(ProvisioningError::HostNotReady(
            "host snapshot is stale or invalid",
        ));
    }
    Ok(())
}

fn transport_name(transport: Transport) -> &'static str {
    match transport {
        Transport::Tcp => "tcp",
        Transport::Udp => "udp",
    }
}

fn check_binding(binding: &PortBinding, host: &HostAdmission) -> Result<(), ProvisioningError> {
    let private = manager_core::host::private_allocation_ip(binding.ip);
    if binding.port == 0 || !private || !host.private_ips.contains(&binding.ip) {
        return Err(ProvisioningError::InvalidInput(
            "allocation must use an approved private host IP and nonzero port",
        ));
    }
    let occupied = host.unavailable_bindings.iter().any(|other| {
        other.port == binding.port
            && other.transport == binding.transport
            && (other.ip == binding.ip
                || (other.ip.is_unspecified()
                    && (other.ip.is_ipv6() || other.ip.is_ipv4() == binding.ip.is_ipv4())))
    });
    if occupied {
        return Err(ProvisioningError::AllocationUnavailable);
    }
    Ok(())
}

fn resolve_variables(
    revision: &EggRevision,
    supplied: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, ProvisioningError> {
    for name in supplied.keys() {
        if !revision
            .variables
            .iter()
            .any(|variable| variable.env_variable == *name && variable.user_editable)
        {
            return Err(ProvisioningError::InvalidInput(
                "variable is unknown or not editable",
            ));
        }
    }
    revision
        .variables
        .iter()
        .map(|variable| {
            let value = supplied
                .get(&variable.env_variable)
                .unwrap_or(&variable.default_value);
            validate_variable(variable, value).map_err(|error| {
                ProvisioningError::Variable(format!("{}: {error}", variable.env_variable))
            })?;
            Ok((variable.env_variable.clone(), value.clone()))
        })
        .collect()
}

fn add_reserved(left: Resources, right: Resources) -> Result<Resources, ProvisioningError> {
    Ok(Resources {
        memory_bytes: left
            .memory_bytes
            .checked_add(right.memory_bytes)
            .ok_or(ProvisioningError::Capacity(CapacityError::Memory))?,
        disk_bytes: left
            .disk_bytes
            .checked_add(right.disk_bytes)
            .ok_or(ProvisioningError::Capacity(CapacityError::Disk))?,
        cpu_millis: left
            .cpu_millis
            .checked_add(right.cpu_millis)
            .ok_or(ProvisioningError::Capacity(CapacityError::Cpu))?,
    })
}
