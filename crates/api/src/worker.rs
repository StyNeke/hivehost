// Background work driven by durable state: install jobs (leased, so an API restart resumes them)
// and servers marked for deletion. Both are idempotent against the agent: re-sending an install
// to a server that is already installing or installed only resumes waiting for the outcome.
use crate::{
    agent_client::AgentClient,
    jobs::{ClaimedJob, JobStore},
    runtime::{InstallState, RuntimeError, RuntimeStore},
};
use manager_core::model::{JobKind, ServerId};
use manager_protocol::{AgentResult, Operation, OperationKind, OperationState};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const LEASE_SECONDS: i64 = 60;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

async fn blocking<T: Send + 'static>(
    path: &Path,
    action: impl FnOnce(&Path) -> Result<T, RuntimeError> + Send + 'static,
) -> Result<T, RuntimeError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || action(&path))
        .await
        .map_err(|_| RuntimeError::Conflict("background task failed"))?
}

const CONCURRENT_INSTALLS: usize = 2;

pub async fn run(database: PathBuf, agent: AgentClient) {
    let worker = format!("api-{}", uuid::Uuid::new_v4().simple());
    let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(CONCURRENT_INSTALLS));
    loop {
        if let Err(error) = tick(&database, &agent, &worker, &slots).await {
            eprintln!("background worker: {error}");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

// Claims at most one job per tick while an install slot is free; the job runs on its own task
// so long installs never hold up deletions or other servers.
pub async fn tick(
    database: &Path,
    agent: &AgentClient,
    worker: &str,
    slots: &std::sync::Arc<tokio::sync::Semaphore>,
) -> Result<(), RuntimeError> {
    if let Ok(permit) = slots.clone().try_acquire_owned() {
        let worker_name = worker.to_owned();
        let claimed = blocking(database, move |path| {
            Ok(JobStore::open(path)?.claim_job(&worker_name, now(), LEASE_SECONDS)?)
        })
        .await?;
        if let Some(job) = claimed {
            let (database, agent, worker) =
                (database.to_path_buf(), agent.clone(), worker.to_owned());
            tokio::spawn(async move {
                let _permit = permit;
                if let Err(error) = run_job(&database, &agent, &worker, job).await {
                    eprintln!("install job: {error}");
                }
            });
        }
    }
    for (server, keep_backups) in blocking(database, |path| {
        RuntimeStore::open(path)?.deleting_servers()
    })
    .await?
    {
        advance_delete(database, agent, server, keep_backups).await?;
    }
    Ok(())
}

async fn run_job(
    database: &Path,
    agent: &AgentClient,
    worker: &str,
    job: ClaimedJob,
) -> Result<(), RuntimeError> {
    let job_id = job.id;
    let server = blocking(database, move |path| {
        RuntimeStore::open(path)?.server_for_install_job(job_id)
    })
    .await?;
    let outcome = match (job.kind, server) {
        (JobKind::Install, Some(server)) => Some((
            server,
            install(database, agent, worker, &job, server).await?,
        )),
        // A job for a deleted server, or a kind this release does not run, completes as a no-op.
        _ => None,
    };
    let worker_name = worker.to_owned();
    blocking(database, move |path| {
        let mut jobs = JobStore::open(path)?;
        jobs.complete(&job, &worker_name, now())?;
        if let Some((server, (state, message))) = outcome {
            let mut store = RuntimeStore::open(path)?;
            store.set_install_state(server, state, message.as_deref())?;
            let result = if state == InstallState::Installed {
                "succeeded"
            } else {
                "failed"
            };
            store.record(None, server, "install", result, message.as_deref(), now())?;
        }
        Ok(())
    })
    .await
}

fn install_outcome(result: &AgentResult) -> Option<(InstallState, Option<String>)> {
    match result {
        AgentResult::ServerStatus(status) => match &status.operation {
            Some(op)
                if op.kind == OperationKind::Install && op.state == OperationState::Succeeded =>
            {
                Some((InstallState::Installed, Some(op.message.clone())))
            }
            Some(op) if op.kind == OperationKind::Install && op.state == OperationState::Failed => {
                Some((InstallState::Failed, Some(op.message.clone())))
            }
            _ if status.installed => Some((InstallState::Installed, None)),
            _ => None,
        },
        _ => None,
    }
}

async fn install(
    database: &Path,
    agent: &AgentClient,
    worker: &str,
    job: &ClaimedJob,
    server: ServerId,
) -> Result<(InstallState, Option<String>), RuntimeError> {
    let spec = blocking(database, move |path| {
        let mut store = RuntimeStore::open(path)?;
        store.set_install_state(server, InstallState::Installing, Some("Installing"))?;
        store.spec(server)
    })
    .await?;
    match agent
        .call(Operation::InstallServer {
            spec: Box::new(spec),
        })
        .await
    {
        Ok(AgentResult::Accepted) => {}
        Ok(AgentResult::Rejected(rejection))
            if rejection.code == "busy" || rejection.code == "already_installed" => {}
        Ok(AgentResult::Rejected(rejection)) => {
            return Ok((InstallState::Failed, Some(rejection.message)));
        }
        // The agent may be restarting; leave the job leased so it is retried after the lease.
        _ => {
            return Err(RuntimeError::Conflict(
                "host agent unavailable during install",
            ));
        }
    }
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let (job_id, attempt, worker_name) = (job.id, job.attempt, worker.to_owned());
        let still_ours = blocking(database, move |path| {
            let claimed = ClaimedJob {
                id: job_id,
                kind: JobKind::Install,
                attempt,
            };
            Ok(JobStore::open(path)?.renew(&claimed, &worker_name, now(), LEASE_SECONDS)?)
        })
        .await?;
        if !still_ours {
            return Err(RuntimeError::Conflict("install job lease lost"));
        }
        if let Ok(result) = agent
            .call(Operation::ServerStatus {
                server_id: server.0,
            })
            .await
            && let Some(outcome) = install_outcome(&result)
        {
            return Ok(outcome);
        }
    }
}

async fn advance_delete(
    database: &Path,
    agent: &AgentClient,
    server: ServerId,
    keep_backups: bool,
) -> Result<(), RuntimeError> {
    let status = agent
        .call(Operation::ServerStatus {
            server_id: server.0,
        })
        .await
        .map_err(|_| RuntimeError::Conflict("host agent unavailable during delete"))?;
    match status {
        AgentResult::Rejected(rejection) if rejection.code == "not_found" => {
            blocking(database, move |path| {
                RuntimeStore::open(path)?.finish_delete(server, keep_backups)
            })
            .await
        }
        AgentResult::ServerStatus(status)
            if status
                .operation
                .as_ref()
                .is_some_and(|op| op.state == OperationState::Running) =>
        {
            Ok(())
        }
        AgentResult::ServerStatus(_) => {
            // Not yet requested, or a previous attempt failed: (re)request deletion.
            let _ = agent
                .call(Operation::DeleteServer {
                    server_id: server.0,
                    keep_backups,
                })
                .await;
            Ok(())
        }
        _ => Ok(()),
    }
}
