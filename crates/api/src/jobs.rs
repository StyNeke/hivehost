use manager_core::model::{JobId, JobKind};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::path::Path;
use uuid::Uuid;

pub struct JobStore(Connection);

#[derive(Debug)]
pub struct ClaimedJob {
    pub id: JobId,
    pub kind: JobKind,
    pub attempt: i64,
}

impl JobStore {
    pub fn renew(
        &mut self,
        job: &ClaimedJob,
        worker: &str,
        now: i64,
        lease_seconds: i64,
    ) -> rusqlite::Result<bool> {
        let until = lease_deadline(worker, now, lease_seconds)?;
        let changed = self.0.execute(
            "UPDATE jobs SET lease_until = ?1 WHERE id = ?2 AND state = 'running'
             AND attempt = ?3 AND worker = ?4 AND lease_until > ?5",
            params![until, job.id.0.to_string(), job.attempt, worker, now],
        )?;
        Ok(changed == 1)
    }
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        Ok(Self(crate::database::open(path)?))
    }

    pub fn enqueue(&mut self, kind: JobKind) -> rusqlite::Result<JobId> {
        let id = JobId(Uuid::new_v4());
        let kind = match kind {
            JobKind::Install => "install",
            JobKind::Backup => "backup",
            JobKind::Restore => "restore",
            JobKind::Update => "update",
            JobKind::Delete => "delete",
        };
        self.0.execute(
            "INSERT INTO jobs (id, kind) VALUES (?1, ?2)",
            params![id.0.to_string(), kind],
        )?;
        Ok(id)
    }

    pub fn claim_job(
        &mut self,
        worker: &str,
        now: i64,
        lease_seconds: i64,
    ) -> rusqlite::Result<Option<ClaimedJob>> {
        let until = lease_deadline(worker, now, lease_seconds)?;
        let transaction = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let job = transaction.query_row(
            "UPDATE jobs SET state = 'running', worker = ?1, lease_until = ?2, attempt = attempt + 1
             WHERE sequence = (SELECT sequence FROM jobs WHERE state = 'queued'
                 OR (state = 'running' AND lease_until <= ?3) ORDER BY sequence LIMIT 1)
             RETURNING id, kind, attempt",
            params![worker, until, now],
            |row| {
                let id: String = row.get(0)?;
                let id = Uuid::parse_str(&id).map_err(|error| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error)))?;
                let kind: String = row.get(1)?;
                let kind = match kind.as_str() {
                    "install" => JobKind::Install,
                    "backup" => JobKind::Backup,
                    "restore" => JobKind::Restore,
                    "update" => JobKind::Update,
                    "delete" => JobKind::Delete,
                    _ => return Err(rusqlite::Error::InvalidQuery),
                };
                Ok(ClaimedJob { id: JobId(id), kind, attempt: row.get(2)? })
            },
        ).optional()?;
        transaction.commit()?;
        Ok(job)
    }

    pub fn complete(&mut self, job: &ClaimedJob, worker: &str, now: i64) -> rusqlite::Result<bool> {
        let changed = self.0.execute(
            "UPDATE jobs SET state = 'completed', worker = NULL, lease_until = NULL
             WHERE id = ?1 AND state = 'running' AND attempt = ?2 AND worker = ?3 AND lease_until > ?4",
            params![job.id.0.to_string(), job.attempt, worker, now],
        )?;
        Ok(changed == 1)
    }
}

fn lease_deadline(worker: &str, now: i64, lease_seconds: i64) -> rusqlite::Result<i64> {
    now.checked_add(lease_seconds)
        .filter(|_| lease_seconds > 0 && now >= 0 && !worker.trim().is_empty())
        .ok_or(rusqlite::Error::InvalidParameterName(
            "invalid worker or lease".into(),
        ))
}
