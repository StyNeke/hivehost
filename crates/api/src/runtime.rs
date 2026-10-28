// Server lifecycle records kept by the API: install state, deletion, egg qualification, installed
// extensions and the activity log. Live container state always comes from the agent.
use manager_core::{
    capacity::Resources,
    eggs::EggRevision,
    host::{PortBinding, Transport},
    model::{EggRevisionId, JobId, ServerId, UserId},
};
use manager_protocol::ServerSpec;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use std::{collections::BTreeMap, path::Path};
use uuid::Uuid;

pub struct RuntimeStore(Connection);

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("not found")]
    NotFound,
    #[error("administrator access required")]
    Forbidden,
    #[error("{0}")]
    Conflict(&'static str),
    #[error("stored server data is invalid")]
    Invalid,
    #[error("storage operation failed")]
    Storage(#[from] rusqlite::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallState {
    Pending,
    Installing,
    Installed,
    Failed,
}

impl InstallState {
    fn parse(value: &str) -> Result<Self, RuntimeError> {
        Ok(match value {
            "pending" => Self::Pending,
            "installing" => Self::Installing,
            "installed" => Self::Installed,
            "failed" => Self::Failed,
            _ => return Err(RuntimeError::Invalid),
        })
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Installing => "installing",
            Self::Installed => "installed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerDetail {
    pub id: ServerId,
    pub name: String,
    pub owner: UserId,
    pub egg_name: String,
    pub egg_revision: EggRevisionId,
    pub image: String,
    pub limits: Resources,
    pub binding: PortBinding,
    pub install_state: InstallState,
    pub install_message: Option<String>,
    pub deleting: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActivityEntry {
    pub at: i64,
    pub actor: Option<String>,
    pub action: String,
    pub outcome: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtensionRecord {
    pub project_id: String,
    pub version_id: String,
    pub title: String,
    pub version_number: String,
    pub filename: String,
    pub dependency: bool,
}

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn uuid(value: String) -> Result<Uuid, RuntimeError> {
    value.parse().map_err(|_| RuntimeError::Invalid)
}

impl RuntimeStore {
    pub fn open(path: &Path) -> Result<Self, RuntimeError> {
        Ok(Self(crate::database::open(path)?))
    }

    pub fn detail(&self, server: ServerId) -> Result<ServerDetail, RuntimeError> {
        let row = self
            .0
            .query_row(
                "SELECT s.name, s.owner_id, r.name, p.egg_revision_id, p.image, p.memory_bytes,
                        p.disk_bytes, p.cpu_millis, a.host_ip, a.port, a.transport,
                        COALESCE(t.install_state, 'pending'), t.install_message, COALESCE(t.deleting, 0)
                 FROM servers s
                 JOIN server_provisioning p ON p.server_id = s.id
                 JOIN egg_revisions r ON r.id = p.egg_revision_id
                 JOIN server_allocations sa ON sa.server_id = s.id AND sa.is_primary = 1
                 JOIN allocation_pool a ON a.id = sa.allocation_id
                 LEFT JOIN server_runtime t ON t.server_id = s.id
                 WHERE s.id = ?1",
                [server.0.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, u32>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, u16>(9)?,
                        row.get::<_, String>(10)?,
                        row.get::<_, String>(11)?,
                        row.get::<_, Option<String>>(12)?,
                        row.get::<_, bool>(13)?,
                    ))
                },
            )
            .optional()?
            .ok_or(RuntimeError::NotFound)?;
        Ok(ServerDetail {
            id: server,
            name: row.0,
            owner: UserId(uuid(row.1)?),
            egg_name: row.2,
            egg_revision: EggRevisionId(uuid(row.3)?),
            image: row.4,
            limits: Resources {
                memory_bytes: u64::try_from(row.5).map_err(|_| RuntimeError::Invalid)?,
                disk_bytes: u64::try_from(row.6).map_err(|_| RuntimeError::Invalid)?,
                cpu_millis: row.7,
            },
            binding: PortBinding {
                ip: row.8.parse().map_err(|_| RuntimeError::Invalid)?,
                port: row.9,
                transport: match row.10.as_str() {
                    "tcp" => Transport::Tcp,
                    "udp" => Transport::Udp,
                    _ => return Err(RuntimeError::Invalid),
                },
            },
            install_state: InstallState::parse(&row.11)?,
            install_message: row.12,
            deleting: row.13,
        })
    }

    // Everything the agent needs, assembled from the pinned egg revision and stored settings.
    pub fn spec(&self, server: ServerId) -> Result<ServerSpec, RuntimeError> {
        let detail = self.detail(server)?;
        let (normalized, variables, license): (String, String, bool) = self
            .0
            .query_row(
                "SELECT r.normalized, p.variables, COALESCE(t.license_accepted, 0)
                 FROM server_provisioning p JOIN egg_revisions r ON r.id = p.egg_revision_id
                 LEFT JOIN server_runtime t ON t.server_id = p.server_id
                 WHERE p.server_id = ?1",
                [server.0.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
            .ok_or(RuntimeError::NotFound)?;
        let revision: EggRevision =
            serde_json::from_str(&normalized).map_err(|_| RuntimeError::Invalid)?;
        let variables: BTreeMap<String, String> =
            serde_json::from_str(&variables).map_err(|_| RuntimeError::Invalid)?;
        Ok(ServerSpec {
            server_id: server.0,
            image: detail.image,
            startup: revision.startup,
            stop: revision.stop,
            ready_when: revision.ready_when,
            accept_eula: license && revision.features.iter().any(|f| f == "eula"),
            installer: revision.installer,
            variables,
            limits: detail.limits,
            binding: detail.binding,
            files: revision.files,
        })
    }

    pub fn server_for_install_job(&self, job: JobId) -> Result<Option<ServerId>, RuntimeError> {
        self.0
            .query_row(
                "SELECT server_id FROM server_provisioning WHERE job_id = ?1",
                [job.0.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|id| uuid(id).map(ServerId))
            .transpose()
    }

    pub fn set_install_state(
        &mut self,
        server: ServerId,
        state: InstallState,
        message: Option<&str>,
    ) -> Result<(), RuntimeError> {
        self.0.execute(
            "INSERT INTO server_runtime (server_id, license_accepted, install_state, install_message)
             VALUES (?1, 0, ?2, ?3)
             ON CONFLICT(server_id) DO UPDATE SET install_state = ?2, install_message = ?3",
            params![server.0.to_string(), state.as_str(), message],
        )?;
        Ok(())
    }

    // Queues a fresh install for a server whose previous install failed.
    pub fn requeue_install(&mut self, server: ServerId) -> Result<JobId, RuntimeError> {
        let transaction = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state: String = transaction
            .query_row(
                "SELECT COALESCE(t.install_state, 'pending') FROM server_provisioning p
                 LEFT JOIN server_runtime t ON t.server_id = p.server_id WHERE p.server_id = ?1",
                [server.0.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(RuntimeError::NotFound)?;
        if state != "failed" {
            return Err(RuntimeError::Conflict(
                "only a failed installation can be retried",
            ));
        }
        let job = JobId(Uuid::new_v4());
        transaction.execute(
            "INSERT INTO jobs (id, kind) VALUES (?1, 'install')",
            [job.0.to_string()],
        )?;
        transaction.execute(
            "UPDATE server_provisioning SET job_id = ?1 WHERE server_id = ?2",
            params![job.0.to_string(), server.0.to_string()],
        )?;
        transaction.execute(
            "UPDATE server_runtime SET install_state = 'pending', install_message = NULL WHERE server_id = ?1",
            [server.0.to_string()],
        )?;
        transaction.commit()?;
        Ok(job)
    }

    // Returns false when deletion was already requested.
    pub fn mark_deleting(
        &mut self,
        server: ServerId,
        keep_backups: bool,
    ) -> Result<bool, RuntimeError> {
        self.0.execute(
            "INSERT INTO deletion_requests VALUES (?1, ?2)
             ON CONFLICT(server_id) DO UPDATE SET keep_backups = keep_backups",
            params![server.0.to_string(), keep_backups],
        )?;
        let changed = self.0.execute(
            "UPDATE server_runtime SET deleting = 1 WHERE server_id = ?1 AND deleting = 0",
            [server.0.to_string()],
        )?;
        if changed == 0 {
            let exists: bool = self.0.query_row(
                "SELECT EXISTS(SELECT 1 FROM server_runtime WHERE server_id = ?1)",
                [server.0.to_string()],
                |row| row.get(0),
            )?;
            if !exists {
                self.0.execute(
                    "INSERT INTO server_runtime (server_id, license_accepted, deleting) VALUES (?1, 0, 1)",
                    [server.0.to_string()],
                )?;
                return Ok(true);
            }
        }
        Ok(changed == 1)
    }

    // Servers marked for deletion, with whether their backups are kept.
    pub fn deleting_servers(&self) -> Result<Vec<(ServerId, bool)>, RuntimeError> {
        let mut statement = self.0.prepare(
            "SELECT t.server_id, COALESCE(d.keep_backups, 0) FROM server_runtime t
             LEFT JOIN deletion_requests d ON d.server_id = t.server_id WHERE t.deleting = 1",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
        })?;
        rows.map(|row| {
            let (id, keep) = row?;
            Ok((ServerId(uuid(id)?), keep))
        })
        .collect()
    }

    pub fn retention(&self, server: ServerId) -> Result<u8, RuntimeError> {
        Ok(self
            .0
            .query_row(
                "SELECT retention FROM backup_settings WHERE server_id = ?1",
                [server.0.to_string()],
                |row| row.get::<_, u8>(0),
            )
            .optional()?
            .unwrap_or(3))
    }

    pub fn set_retention(&mut self, server: ServerId, retention: u8) -> Result<(), RuntimeError> {
        if !(1..=5).contains(&retention) {
            return Err(RuntimeError::Conflict("keep between 1 and 5 backups"));
        }
        self.0.execute(
            "INSERT INTO backup_settings VALUES (?1, ?2) ON CONFLICT(server_id) DO UPDATE SET retention = ?2",
            params![server.0.to_string(), retention],
        )?;
        Ok(())
    }

    // Who may see backups of a server that no longer exists: the deletion-time owner and
    // enabled members, and administrators.
    pub fn retained_access(
        &self,
        actor: UserId,
        server: ServerId,
    ) -> Result<Option<String>, RuntimeError> {
        Ok(self
            .0
            .query_row(
                "SELECT r.server_name FROM retained_backups r
                 WHERE r.server_id = ?1 AND (
                    EXISTS(SELECT 1 FROM retained_backup_access a WHERE a.server_id = r.server_id AND a.user_id = ?2)
                    OR EXISTS(SELECT 1 FROM users u WHERE u.id = ?2 AND u.is_admin = 1))",
                params![server.0.to_string(), actor.0.to_string()],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn retained_for(
        &self,
        actor: UserId,
    ) -> Result<Vec<(ServerId, String, i64)>, RuntimeError> {
        let mut statement = self.0.prepare(
            "SELECT r.server_id, r.server_name, r.deleted_at FROM retained_backups r
             WHERE EXISTS(SELECT 1 FROM retained_backup_access a WHERE a.server_id = r.server_id AND a.user_id = ?1)
                OR EXISTS(SELECT 1 FROM users u WHERE u.id = ?1 AND u.is_admin = 1)
             ORDER BY r.deleted_at DESC",
        )?;
        let rows = statement.query_map([actor.0.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (id, name, at) = row?;
            Ok((ServerId(uuid(id)?), name, at))
        })
        .collect()
    }

    pub fn forget_retained(&mut self, server: ServerId) -> Result<(), RuntimeError> {
        self.0.execute(
            "DELETE FROM retained_backups WHERE server_id = ?1",
            [server.0.to_string()],
        )?;
        Ok(())
    }

    // Removes the server and, through cascades, its reservation, allocation claim, members,
    // extensions and runtime record. The allocation returns to the free pool.
    pub fn finish_delete(
        &mut self,
        server: ServerId,
        keep_backups: bool,
    ) -> Result<(), RuntimeError> {
        let transaction = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if keep_backups {
            transaction.execute(
                "INSERT OR REPLACE INTO retained_backups
                 SELECT id, name, owner_id, strftime('%s','now') FROM servers WHERE id = ?1",
                [server.0.to_string()],
            )?;
            transaction.execute(
                "INSERT OR IGNORE INTO retained_backup_access
                 SELECT ?1, owner_id FROM servers WHERE id = ?1
                 UNION SELECT ?1, user_id FROM server_members WHERE server_id = ?1 AND enabled = 1",
                [server.0.to_string()],
            )?;
        }
        let name: Option<String> = transaction
            .query_row(
                "SELECT name FROM servers WHERE id = ?1",
                [server.0.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        transaction.execute(
            "DELETE FROM server_provisioning WHERE server_id = ?1",
            [server.0.to_string()],
        )?;
        transaction.execute("DELETE FROM servers WHERE id = ?1", [server.0.to_string()])?;
        if let Some(name) = name {
            transaction.execute(
                "INSERT INTO activity (at, actor_id, server_id, server_name, action, outcome)
                 VALUES (strftime('%s','now'), NULL, ?1, ?2, 'delete', 'succeeded')",
                params![server.0.to_string(), name],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn record(
        &mut self,
        actor: Option<UserId>,
        server: ServerId,
        action: &str,
        outcome: &str,
        detail: Option<&str>,
        now: i64,
    ) -> Result<(), RuntimeError> {
        self.0.execute(
            "INSERT INTO activity (at, actor_id, server_id, server_name, action, outcome, detail)
             VALUES (?1, ?2, ?3, (SELECT name FROM servers WHERE id = ?3), ?4, ?5, ?6)",
            params![
                now,
                actor.map(|a| a.0.to_string()),
                server.0.to_string(),
                action,
                outcome,
                detail.map(|d| d.chars().take(500).collect::<String>())
            ],
        )?;
        Ok(())
    }

    pub fn activity(
        &self,
        server: ServerId,
        limit: u32,
    ) -> Result<Vec<ActivityEntry>, RuntimeError> {
        let mut statement = self.0.prepare(
            "SELECT a.at, u.username, a.action, a.outcome, a.detail FROM activity a
             LEFT JOIN users u ON u.id = a.actor_id
             WHERE a.server_id = ?1 ORDER BY a.sequence DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![server.0.to_string(), limit.min(200)], |row| {
            Ok(ActivityEntry {
                at: row.get(0)?,
                actor: row.get(1)?,
                action: row.get(2)?,
                outcome: row.get(3)?,
                detail: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn qualify(
        &mut self,
        actor: UserId,
        revision: EggRevisionId,
        now: i64,
    ) -> Result<(), RuntimeError> {
        let admin: bool = self
            .0
            .query_row(
                "SELECT is_admin FROM users WHERE id = ?1",
                [actor.0.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false);
        if !admin {
            return Err(RuntimeError::Forbidden);
        }
        let exists: bool = self.0.query_row(
            "SELECT EXISTS(SELECT 1 FROM egg_revisions WHERE id = ?1)",
            [revision.0.to_string()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(RuntimeError::NotFound);
        }
        self.0.execute(
            "INSERT OR IGNORE INTO egg_qualifications VALUES (?1, ?2, ?3)",
            params![revision.0.to_string(), actor.0.to_string(), now],
        )?;
        Ok(())
    }

    pub fn extensions(&self, server: ServerId) -> Result<Vec<ExtensionRecord>, RuntimeError> {
        let mut statement = self.0.prepare(
            "SELECT project_id, version_id, title, version_number, filename, dependency
             FROM server_extensions WHERE server_id = ?1 ORDER BY dependency, title",
        )?;
        let rows = statement.query_map([server.0.to_string()], |row| {
            Ok(ExtensionRecord {
                project_id: row.get(0)?,
                version_id: row.get(1)?,
                title: row.get(2)?,
                version_number: row.get(3)?,
                filename: row.get(4)?,
                dependency: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    // Replaces the records for the given projects after the agent confirmed the file change.
    pub fn record_extensions(
        &mut self,
        server: ServerId,
        installed: &[ExtensionRecord],
        removed_projects: &[String],
    ) -> Result<(), RuntimeError> {
        let transaction = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for project in removed_projects
            .iter()
            .chain(installed.iter().map(|e| &e.project_id))
        {
            transaction.execute(
                "DELETE FROM server_extensions WHERE server_id = ?1 AND project_id = ?2",
                params![server.0.to_string(), project],
            )?;
        }
        for extension in installed {
            transaction.execute(
                "DELETE FROM server_extensions WHERE server_id = ?1 AND filename = ?2",
                params![server.0.to_string(), extension.filename],
            )?;
            transaction.execute(
                "INSERT INTO server_extensions VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    server.0.to_string(),
                    extension.project_id,
                    extension.version_id,
                    extension.title,
                    extension.version_number,
                    extension.filename,
                    extension.dependency
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
}
