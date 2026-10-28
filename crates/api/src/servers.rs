use manager_core::{
    model::{Server, ServerId, User, UserId},
    permissions::{can_change_member, can_manage},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use std::path::Path;
use uuid::Uuid;

pub struct ServerStore(Connection);

#[derive(Debug, Serialize)]
pub struct ServerSummary {
    pub id: ServerId,
    pub name: String,
    pub can_access: bool,
    pub usage: Option<Usage>,
}
// Measured by the agent. CPU percent uses 100% = one logical CPU. Null fields mean unknown,
// never zero: a stopped server has no CPU/memory reading.
#[derive(Debug, Serialize)]
pub struct Usage {
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub disk_bytes: Option<u64>,
}
#[derive(Debug, Serialize)]
pub struct ServerRecord {
    pub id: ServerId,
    pub name: String,
    pub owner: UserId,
}

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("server not found")]
    NotFound,
    #[error("access denied")]
    Forbidden,
    #[error("storage operation failed")]
    Storage(#[from] rusqlite::Error),
    #[error("invalid stored identifier")]
    InvalidIdentifier,
}

impl ServerStore {
    pub fn open(path: &Path) -> Result<Self, ServerError> {
        Ok(Self(crate::database::open(path)?))
    }

    pub fn exists(&self, server: ServerId) -> bool {
        self.0
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM servers WHERE id = ?1)",
                [server.0.to_string()],
                |row| row.get(0),
            )
            .unwrap_or(true)
    }

    pub fn list(&self, actor: UserId) -> Result<Vec<ServerSummary>, ServerError> {
        let actor = load_user(&self.0, actor)?;
        let mut query = self.0.prepare("SELECT s.id, s.name, s.owner_id, COALESCE(m.enabled, 0) FROM servers s
            LEFT JOIN server_members m ON m.server_id = s.id AND m.user_id = ?1 ORDER BY s.name, s.id")?;
        let rows = query.query_map([actor.id.0.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (id, name, owner, enabled) = row?;
            let server = Server {
                id: ServerId(parse_id(&id)?),
                owner: UserId(parse_id(&owner)?),
            };
            // Filled in from the agent by the HTTP layer; unknown stays None.
            Ok(ServerSummary {
                id: server.id,
                name,
                can_access: can_manage(&actor, &server, enabled),
                usage: None,
            })
        })
        .collect()
    }

    pub fn require_access(
        &self,
        actor: UserId,
        server: ServerId,
    ) -> Result<ServerRecord, ServerError> {
        let actor = load_user(&self.0, actor)?;
        let record = load_server(&self.0, server)?;
        let enabled = member_enabled(&self.0, server, actor.id)?;
        if !can_manage(
            &actor,
            &Server {
                id: record.id,
                owner: record.owner,
            },
            enabled,
        ) {
            return Err(ServerError::Forbidden);
        }
        Ok(record)
    }

    pub fn set_member(
        &mut self,
        actor: UserId,
        server: ServerId,
        target: UserId,
        enabled: bool,
    ) -> Result<(), ServerError> {
        let transaction = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let actor = load_user(&transaction, actor)?;
        let target = load_user(&transaction, target)?;
        let record = load_server(&transaction, server)?;
        let actor_enabled = member_enabled(&transaction, server, actor.id)?;
        if !can_change_member(
            &actor,
            &Server {
                id: record.id,
                owner: record.owner,
            },
            actor_enabled,
            &target,
            enabled,
        ) {
            return Err(ServerError::Forbidden);
        }
        transaction.execute(
            "INSERT INTO server_members (server_id, user_id, enabled) VALUES (?1, ?2, ?3)
            ON CONFLICT(server_id, user_id) DO UPDATE SET enabled = excluded.enabled",
            params![server.0.to_string(), target.id.0.to_string(), enabled],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct Member {
    pub user_id: UserId,
    pub username: String,
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct Person {
    pub id: UserId,
    pub username: String,
}

impl ServerStore {
    // Members of a server the actor may manage, plus every other account so access can be granted.
    pub fn members(&self, actor: UserId, server: ServerId) -> Result<Vec<Member>, ServerError> {
        self.require_access(actor, server)?;
        let mut statement = self.0.prepare(
            "SELECT u.id, u.username, m.enabled FROM server_members m JOIN users u ON u.id = m.user_id
             WHERE m.server_id = ?1 ORDER BY u.username",
        )?;
        let rows = statement.query_map([server.0.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (id, username, enabled) = row?;
            Ok(Member {
                user_id: UserId(parse_id(&id)?),
                username,
                enabled,
            })
        })
        .collect()
    }

    // Usernames are visible to every signed-in teammate (small private team) for granting access.
    pub fn people(&self, actor: UserId) -> Result<Vec<Person>, ServerError> {
        load_user(&self.0, actor)?;
        let mut statement = self
            .0
            .prepare("SELECT id, username FROM users ORDER BY username")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (id, username) = row?;
            Ok(Person {
                id: UserId(parse_id(&id)?),
                username,
            })
        })
        .collect()
    }
}

fn parse_id(value: &str) -> Result<Uuid, ServerError> {
    Uuid::parse_str(value).map_err(|_| ServerError::InvalidIdentifier)
}

fn load_user(connection: &Connection, id: UserId) -> Result<User, ServerError> {
    let is_admin = connection
        .query_row(
            "SELECT is_admin FROM users WHERE id = ?1",
            [id.0.to_string()],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(ServerError::Forbidden)?;
    Ok(User { id, is_admin })
}

fn load_server(connection: &Connection, id: ServerId) -> Result<ServerRecord, ServerError> {
    let (name, owner): (String, String) = connection
        .query_row(
            "SELECT name, owner_id FROM servers WHERE id = ?1",
            [id.0.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or(ServerError::NotFound)?;
    Ok(ServerRecord {
        id,
        name,
        owner: UserId(parse_id(&owner)?),
    })
}

fn member_enabled(
    connection: &Connection,
    server: ServerId,
    user: UserId,
) -> Result<bool, ServerError> {
    Ok(connection
        .query_row(
            "SELECT enabled FROM server_members WHERE server_id = ?1 AND user_id = ?2",
            params![server.0.to_string(), user.0.to_string()],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(false))
}
