// Scheduled restarts, backups, console commands and updates. Each occurrence is claimed in one
// transaction (moving next_run_at forward first), so it runs at most once; occurrences missed
// while the panel was down for longer than MISSED_AFTER_SECS are recorded as missed instead of
// running late at a surprising time.
use crate::{
    agent_client::AgentClient,
    database,
    runtime::{RuntimeError, RuntimeStore},
};
use manager_core::{model::ServerId, schedule::Recurrence};
use manager_protocol::{AgentResult, MAX_COMMAND_BYTES, Operation, PowerState};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

pub const MAX_SCHEDULES_PER_SERVER: usize = 20;
const MISSED_AFTER_SECS: i64 = 15 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Restart,
    Backup,
    Command,
    Update,
}

impl Action {
    fn as_str(self) -> &'static str {
        match self {
            Action::Restart => "restart",
            Action::Backup => "backup",
            Action::Command => "command",
            Action::Update => "update",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "restart" => Action::Restart,
            "backup" => Action::Backup,
            "command" => Action::Command,
            "update" => Action::Update,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleInput {
    pub action: Action,
    #[serde(default)]
    pub command: Option<String>,
    pub hour: u8,
    pub minute: u8,
    pub weekdays: u8,
    pub timezone: String,
    pub enabled: bool,
}

impl ScheduleInput {
    fn recurrence(&self) -> Recurrence {
        Recurrence {
            hour: self.hour,
            minute: self.minute,
            weekdays: self.weekdays,
            timezone: self.timezone.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Schedule {
    pub id: Uuid,
    pub action: Action,
    pub command: Option<String>,
    #[serde(flatten)]
    pub recurrence: Recurrence,
    pub enabled: bool,
    pub next_run_at: i64,
    pub last_run_at: Option<i64>,
    pub last_outcome: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Invalid(String),
    #[error("storage operation failed")]
    Storage(#[from] rusqlite::Error),
}

#[derive(Debug, Clone)]
pub struct Due {
    pub id: Uuid,
    pub server: ServerId,
    pub action: Action,
    pub command: Option<String>,
}

pub struct ScheduleStore(Connection);

fn validate(input: &ScheduleInput) -> Result<Option<String>, ScheduleError> {
    input
        .recurrence()
        .validate()
        .map_err(ScheduleError::Invalid)?;
    match input.action {
        Action::Command => {
            let command = input.command.as_deref().unwrap_or("").trim();
            if command.is_empty()
                || command.len() > MAX_COMMAND_BYTES
                || command.chars().any(char::is_control)
            {
                return Err(ScheduleError::Invalid(
                    "enter one console command without line breaks".into(),
                ));
            }
            Ok(Some(command.to_owned()))
        }
        _ => Ok(None),
    }
}

fn row_to_schedule(row: &rusqlite::Row<'_>) -> rusqlite::Result<Schedule> {
    let id: String = row.get(0)?;
    let action: String = row.get(1)?;
    let invalid = |_| rusqlite::Error::InvalidQuery;
    Ok(Schedule {
        id: Uuid::parse_str(&id).map_err(invalid)?,
        action: Action::parse(&action).ok_or(rusqlite::Error::InvalidQuery)?,
        command: row.get(2)?,
        recurrence: Recurrence {
            hour: row.get(3)?,
            minute: row.get(4)?,
            weekdays: row.get(5)?,
            timezone: row.get(6)?,
        },
        enabled: row.get(7)?,
        next_run_at: row.get(8)?,
        last_run_at: row.get(9)?,
        last_outcome: row.get(10)?,
    })
}

const COLUMNS: &str = "id, action, command, hour, minute, weekdays, timezone, enabled, next_run_at, last_run_at, last_outcome";

impl ScheduleStore {
    pub fn open(path: &Path) -> Result<Self, ScheduleError> {
        Ok(Self(database::open(path)?))
    }

    pub fn list(&self, server: ServerId) -> Result<Vec<Schedule>, ScheduleError> {
        let mut statement = self.0.prepare(&format!(
            "SELECT {COLUMNS} FROM schedules WHERE server_id = ?1 ORDER BY hour, minute, created_at"
        ))?;
        let rows = statement.query_map([server.0.to_string()], row_to_schedule)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn create(
        &mut self,
        server: ServerId,
        actor: Uuid,
        input: ScheduleInput,
        now: i64,
    ) -> Result<Schedule, ScheduleError> {
        let command = validate(&input)?;
        let recurrence = input.recurrence();
        let next = recurrence.next_after(now).map_err(ScheduleError::Invalid)?;
        let transaction = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM schedules WHERE server_id = ?1",
            [server.0.to_string()],
            |row| row.get(0),
        )?;
        if count as usize >= MAX_SCHEDULES_PER_SERVER {
            return Err(ScheduleError::Invalid(format!(
                "a server can have at most {MAX_SCHEDULES_PER_SERVER} schedules"
            )));
        }
        let id = Uuid::new_v4();
        transaction.execute(
            "INSERT INTO schedules (id, server_id, action, command, hour, minute, weekdays, timezone, enabled, next_run_at, created_by, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                id.to_string(),
                server.0.to_string(),
                input.action.as_str(),
                command,
                recurrence.hour,
                recurrence.minute,
                recurrence.weekdays,
                recurrence.timezone,
                input.enabled,
                next,
                actor.to_string(),
                now
            ],
        )?;
        transaction.commit()?;
        Ok(Schedule {
            id,
            action: input.action,
            command,
            recurrence,
            enabled: input.enabled,
            next_run_at: next,
            last_run_at: None,
            last_outcome: None,
        })
    }

    pub fn update(
        &mut self,
        server: ServerId,
        id: Uuid,
        input: ScheduleInput,
        now: i64,
    ) -> Result<(), ScheduleError> {
        let command = validate(&input)?;
        let recurrence = input.recurrence();
        let next = recurrence.next_after(now).map_err(ScheduleError::Invalid)?;
        let changed = self.0.execute(
            "UPDATE schedules SET action = ?1, command = ?2, hour = ?3, minute = ?4, weekdays = ?5, timezone = ?6, enabled = ?7, next_run_at = ?8
             WHERE id = ?9 AND server_id = ?10",
            params![
                input.action.as_str(),
                command,
                recurrence.hour,
                recurrence.minute,
                recurrence.weekdays,
                recurrence.timezone,
                input.enabled,
                next,
                id.to_string(),
                server.0.to_string()
            ],
        )?;
        if changed == 0 {
            return Err(ScheduleError::NotFound);
        }
        Ok(())
    }

    pub fn delete(&mut self, server: ServerId, id: Uuid) -> Result<(), ScheduleError> {
        let changed = self.0.execute(
            "DELETE FROM schedules WHERE id = ?1 AND server_id = ?2",
            params![id.to_string(), server.0.to_string()],
        )?;
        if changed == 0 {
            return Err(ScheduleError::NotFound);
        }
        Ok(())
    }

    // Moves every due schedule to its next occurrence and returns the ones to run now.
    pub fn claim_due(&mut self, now: i64) -> Result<Vec<Due>, ScheduleError> {
        let transaction = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let due: Vec<(String, String, String, Option<String>, i64, Recurrence)> = {
            let mut statement = transaction.prepare(
                "SELECT id, server_id, action, command, next_run_at, hour, minute, weekdays, timezone
                 FROM schedules WHERE enabled = 1 AND next_run_at <= ?1 LIMIT 100",
            )?;
            let rows = statement.query_map([now], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    Recurrence {
                        hour: row.get(5)?,
                        minute: row.get(6)?,
                        weekdays: row.get(7)?,
                        timezone: row.get(8)?,
                    },
                ))
            })?;
            rows.collect::<Result<_, _>>()?
        };
        let mut runs = vec![];
        for (id, server, action, command, at, recurrence) in due {
            match recurrence.next_after(now) {
                Ok(next) => {
                    transaction.execute(
                        "UPDATE schedules SET next_run_at = ?1 WHERE id = ?2",
                        params![next, id],
                    )?;
                }
                Err(error) => {
                    // A time zone that disappeared from the host's database: stop, and say why.
                    transaction.execute(
                        "UPDATE schedules SET enabled = 0, last_outcome = ?1 WHERE id = ?2",
                        params![format!("disabled: {error}"), id],
                    )?;
                    continue;
                }
            }
            if now - at > MISSED_AFTER_SECS {
                transaction.execute(
                    "UPDATE schedules SET last_outcome = ?1 WHERE id = ?2",
                    params![
                        "missed: the panel was not running at the scheduled time",
                        id
                    ],
                )?;
                continue;
            }
            if let (Ok(id), Ok(server), Some(action)) = (
                Uuid::parse_str(&id),
                Uuid::parse_str(&server),
                Action::parse(&action),
            ) {
                runs.push(Due {
                    id,
                    server: ServerId(server),
                    action,
                    command,
                });
            }
        }
        transaction.commit()?;
        Ok(runs)
    }

    pub fn finish(&mut self, id: Uuid, outcome: &str, now: i64) -> Result<(), ScheduleError> {
        self.0.execute(
            "UPDATE schedules SET last_run_at = ?1, last_outcome = ?2 WHERE id = ?3",
            params![
                now,
                outcome.chars().take(500).collect::<String>(),
                id.to_string()
            ],
        )?;
        Ok(())
    }

    pub fn server_of(&self, id: Uuid) -> Result<Option<ServerId>, ScheduleError> {
        Ok(self
            .0
            .query_row(
                "SELECT server_id FROM schedules WHERE id = ?1",
                [id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|id| Uuid::parse_str(&id).ok().map(ServerId)))
    }
}

fn current_time() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub async fn run(database: PathBuf, agent: AgentClient) {
    loop {
        if let Err(error) = tick(&database, &agent, current_time()).await {
            eprintln!("scheduler: {error}");
        }
        tokio::time::sleep(Duration::from_secs(20)).await;
    }
}

// Claims due schedules and starts each on its own task, so a slow restart never delays others.
pub async fn tick(database: &Path, agent: &AgentClient, now: i64) -> Result<(), ScheduleError> {
    let path = database.to_path_buf();
    let due = tokio::task::spawn_blocking(move || ScheduleStore::open(&path)?.claim_due(now))
        .await
        .map_err(|_| ScheduleError::Invalid("scheduler task failed".into()))??;
    for run in due {
        let database = database.to_path_buf();
        let agent = agent.clone();
        tokio::spawn(async move {
            let outcome = execute(&database, &agent, &run).await;
            let (result, detail) = match &outcome {
                Ok(detail) => ("succeeded", detail.clone()),
                Err(detail) => ("failed", detail.clone()),
            };
            let action = match run.action {
                Action::Restart => "scheduled_restart",
                Action::Backup => "scheduled_backup",
                Action::Command => "scheduled_command",
                Action::Update => "scheduled_update",
            };
            let _ = tokio::task::spawn_blocking(move || -> Result<(), ScheduleError> {
                let at = current_time();
                ScheduleStore::open(&database)?.finish(run.id, &detail, at)?;
                RuntimeStore::open(&database)
                    .and_then(|mut store| {
                        store.record(None, run.server, action, result, Some(&detail), at)
                    })
                    .map_err(|_| ScheduleError::Invalid("activity".into()))
            })
            .await;
        });
    }
    Ok(())
}

async fn status(agent: &AgentClient, server: ServerId) -> Result<(PowerState, bool), String> {
    match agent
        .call(Operation::ServerStatus {
            server_id: server.0,
        })
        .await
    {
        Ok(AgentResult::ServerStatus(status)) => Ok((status.power, status.desired_running)),
        Ok(AgentResult::Rejected(rejection)) => Err(rejection.message),
        _ => Err("the host agent is not available".into()),
    }
}

fn accepted(result: Result<AgentResult, impl std::fmt::Display>) -> Result<(), String> {
    match result {
        Ok(AgentResult::Accepted) => Ok(()),
        Ok(AgentResult::Rejected(rejection)) => Err(rejection.message),
        Ok(_) => Err("unexpected answer from the host agent".into()),
        Err(error) => Err(format!("the host agent is not available ({error})")),
    }
}

async fn execute(database: &Path, agent: &AgentClient, run: &Due) -> Result<String, String> {
    let path = database.to_path_buf();
    let server = run.server;
    let (detail, spec, retention) = tokio::task::spawn_blocking(move || {
        let store = RuntimeStore::open(&path)?;
        let detail = store.detail(server)?;
        let retention = store.retention(server)?;
        let spec = store.spec(server).ok();
        Ok::<_, RuntimeError>((detail, spec, retention))
    })
    .await
    .map_err(|_| "scheduler task failed".to_string())?
    .map_err(|error| error.to_string())?;
    if detail.deleting {
        return Err("skipped: the server is being deleted".into());
    }
    let spec = spec.ok_or("skipped: the server is not installed")?;
    let (power, desired) = status(agent, server).await?;
    match run.action {
        Action::Command => {
            if power != PowerState::Running {
                return Ok("skipped: the server was not running".into());
            }
            let command = run.command.clone().unwrap_or_default();
            accepted(
                agent
                    .call(Operation::SendCommand {
                        server_id: server.0,
                        command,
                    })
                    .await,
            )?;
            Ok("command sent".into())
        }
        Action::Backup => {
            accepted(
                agent
                    .call(Operation::CreateBackup {
                        server_id: server.0,
                        retention,
                    })
                    .await,
            )?;
            Ok("backup started".into())
        }
        Action::Update => {
            accepted(
                agent
                    .call(Operation::UpdateServer {
                        spec: Box::new(spec),
                        keep_game_version: true,
                    })
                    .await,
            )?;
            Ok("update started (stays on the installed game version)".into())
        }
        Action::Restart => {
            if power == PowerState::Offline && !desired {
                return Ok("skipped: the server was not running".into());
            }
            accepted(
                agent
                    .call(Operation::StopServer {
                        server_id: server.0,
                    })
                    .await,
            )?;
            // The agent force-stops after its grace period, so this wait is bounded.
            for _ in 0..3600 {
                let (power, desired) = status(agent, server).await?;
                if power == PowerState::Offline {
                    accepted(
                        agent
                            .call(Operation::StartServer {
                                spec: Box::new(spec),
                            })
                            .await,
                    )?;
                    return Ok("restarted".into());
                }
                if desired {
                    return Ok("someone started the server during the restart".into());
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err("the server did not stop in time".into())
        }
    }
}
