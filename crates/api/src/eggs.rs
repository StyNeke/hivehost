use manager_core::{
    eggs::{CompatibilityIssue, EggRevision, normalize_egg},
    model::{EggRevisionId, UserId},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use uuid::Uuid;

pub struct EggStore(Connection);

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("egg is unsupported")]
    Unsupported(Vec<CompatibilityIssue>),
    #[error("access denied")]
    Forbidden,
    #[error("egg not found")]
    NotFound,
    #[error("storage failure")]
    Storage(#[from] rusqlite::Error),
    #[error("invalid stored revision")]
    InvalidRevision,
}

#[derive(Debug, Serialize)]
pub struct EggSummary {
    pub id: Uuid,
    pub name: String,
    pub revision: EggRevisionId,
    pub runtime_qualified: bool,
    // From the experimental panel; shown with an experimental label everywhere.
    pub experimental: bool,
}

pub struct StoredRevision {
    pub id: EggRevisionId,
    pub raw: Vec<u8>,
    pub normalized: EggRevision,
}

impl EggStore {
    pub fn open(path: &Path) -> Result<Self, LibraryError> {
        Ok(Self(crate::database::open(path)?))
    }

    pub fn import(&mut self, actor: UserId, raw: &[u8]) -> Result<EggSummary, LibraryError> {
        let normalized = normalize_egg(raw).map_err(LibraryError::Unsupported)?;
        let encoded =
            serde_json::to_string(&normalized).map_err(|_| LibraryError::InvalidRevision)?;
        let egg = Uuid::new_v4();
        let revision = EggRevisionId(Uuid::new_v4());
        let transaction = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO eggs VALUES (?1, ?2)",
            params![egg.to_string(), actor.0.to_string()],
        )?;
        transaction.execute(
            "INSERT INTO egg_revisions (id, egg_id, name, raw, normalized, sha256, creator_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![revision.0.to_string(), egg.to_string(), normalized.name, raw, encoded, format!("{:x}", Sha256::digest(raw)), actor.0.to_string()],
        )?;
        transaction.commit()?;
        Ok(EggSummary {
            id: egg,
            name: normalized.name,
            revision,
            runtime_qualified: false,
            experimental: false,
        })
    }

    pub fn revise(
        &mut self,
        actor: UserId,
        egg: Uuid,
        raw: &[u8],
    ) -> Result<EggSummary, LibraryError> {
        let normalized = normalize_egg(raw).map_err(LibraryError::Unsupported)?;
        let encoded =
            serde_json::to_string(&normalized).map_err(|_| LibraryError::InvalidRevision)?;
        let revision = EggRevisionId(Uuid::new_v4());
        let transaction = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let allowed = transaction.query_row(
            "SELECT (eggs.importer_id = users.id OR users.is_admin = 1) FROM eggs, users WHERE eggs.id = ?1 AND users.id = ?2",
            params![egg.to_string(), actor.0.to_string()], |row| row.get::<_, bool>(0),
        ).optional()?;
        if !allowed.unwrap_or(false) {
            return Err(LibraryError::Forbidden);
        }
        transaction.execute(
            "INSERT INTO egg_revisions (id, egg_id, name, raw, normalized, sha256, creator_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![revision.0.to_string(), egg.to_string(), normalized.name, raw, encoded, format!("{:x}", Sha256::digest(raw)), actor.0.to_string()],
        )?;
        let experimental: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM experimental_eggs WHERE egg_id = ?1)",
            [egg.to_string()],
            |row| row.get(0),
        )?;
        transaction.commit()?;
        Ok(EggSummary {
            id: egg,
            name: normalized.name,
            revision,
            runtime_qualified: false,
            experimental,
        })
    }

    pub fn list(&self) -> Result<Vec<EggSummary>, LibraryError> {
        let mut statement = self.0.prepare(
            "SELECT egg_id, name, id,
                    EXISTS(SELECT 1 FROM egg_qualifications q WHERE q.revision_id = r.id),
                    EXISTS(SELECT 1 FROM experimental_eggs x WHERE x.egg_id = r.egg_id)
             FROM egg_revisions r WHERE sequence =
             (SELECT max(sequence) FROM egg_revisions WHERE egg_id = r.egg_id) ORDER BY name, egg_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, bool>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (id, name, revision, runtime_qualified, experimental) = row?;
            Ok(EggSummary {
                id: Uuid::parse_str(&id).map_err(|_| LibraryError::InvalidRevision)?,
                name,
                revision: EggRevisionId(
                    Uuid::parse_str(&revision).map_err(|_| LibraryError::InvalidRevision)?,
                ),
                runtime_qualified,
                experimental,
            })
        })
        .collect()
    }

    pub fn revision(&self, id: EggRevisionId) -> Result<StoredRevision, LibraryError> {
        let (raw, normalized): (Vec<u8>, String) = self
            .0
            .query_row(
                "SELECT raw, normalized FROM egg_revisions WHERE id = ?1",
                [id.0.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or(LibraryError::NotFound)?;
        let normalized =
            serde_json::from_str(&normalized).map_err(|_| LibraryError::InvalidRevision)?;
        Ok(StoredRevision {
            id,
            raw,
            normalized,
        })
    }
}
