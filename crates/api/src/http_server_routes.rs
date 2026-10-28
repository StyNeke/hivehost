// Routes for running servers. Every handler re-checks access on its own request (owner, enabled
// member or administrator), so revoking access stops console polling and controls immediately.
use super::{ApiError, AppState, ErrorBody, error, now, server_error, storage_error};
use crate::{
    agent_client::AgentClient,
    modrinth::{ModrinthError, SearchHit, target},
    runtime::{ExtensionRecord, InstallState, RuntimeError, RuntimeStore, ServerDetail},
    servers::ServerStore,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use manager_core::model::{EggRevisionId, ServerId, UserId};
use manager_protocol::{
    AgentResult, DetectedGame, Operation, OperationKind, OperationState, Rejection, RuntimeStatus,
};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, time::Duration};
use uuid::Uuid;

#[path = "http_backup_routes.rs"]
pub(super) mod backups;
#[path = "http_file_routes.rs"]
pub(super) mod files;

fn runtime_error(err: RuntimeError) -> ApiError {
    match err {
        RuntimeError::NotFound => error(StatusCode::NOT_FOUND, "not_found", "Not found"),
        RuntimeError::Forbidden => error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Administrator access required",
        ),
        RuntimeError::Conflict(message) => error(StatusCode::CONFLICT, "conflict", message),
        _ => storage_error(),
    }
}

fn agent_unavailable() -> ApiError {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "host_unavailable",
        "The host agent is not available",
    )
}

// Agent rejections are written for people ("stop the server before changing mods"), so their
// message is passed through with a stable code.
fn rejected(rejection: Rejection) -> ApiError {
    let code = match rejection.code.as_str() {
        "port_in_use" => "port_in_use",
        "running" => "server_running",
        "not_running" => "server_not_running",
        "busy" => "server_busy",
        "invalid_command" => "invalid_command",
        "not_installed" => "not_installed",
        _ => "host_rejected",
    };
    (
        StatusCode::CONFLICT,
        Json(ErrorBody {
            code,
            message: Cow::Owned(rejection.message),
            details: None,
        }),
    )
}

fn modrinth_error(err: ModrinthError) -> ApiError {
    match err {
        ModrinthError::Unavailable => error(
            StatusCode::BAD_GATEWAY,
            "modrinth_unavailable",
            "Modrinth could not be reached; try again later",
        ),
        ModrinthError::NotFound => error(StatusCode::NOT_FOUND, "not_found", "Project not found"),
        ModrinthError::Unsupported(message) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ErrorBody {
                code: "incompatible",
                message: Cow::Owned(message),
                details: None,
            }),
        ),
    }
}

fn agent(state: &AppState) -> Result<&AgentClient, ApiError> {
    state.agent.as_ref().ok_or_else(agent_unavailable)
}

async fn call(state: &AppState, operation: Operation) -> Result<AgentResult, ApiError> {
    agent(state)?
        .call(operation)
        .await
        .map_err(|_| agent_unavailable())
}

async fn accepted(state: &AppState, operation: Operation) -> Result<(), ApiError> {
    match call(state, operation).await? {
        AgentResult::Accepted => Ok(()),
        AgentResult::Rejected(rejection) => Err(rejected(rejection)),
        _ => Err(agent_unavailable()),
    }
}

async fn runtime_status(state: &AppState, server: ServerId) -> Result<RuntimeStatus, ApiError> {
    match call(
        state,
        Operation::ServerStatus {
            server_id: server.0,
        },
    )
    .await?
    {
        AgentResult::ServerStatus(status) => Ok(*status),
        AgentResult::Rejected(rejection) if rejection.code == "not_found" => Err(error(
            StatusCode::CONFLICT,
            "not_installed",
            "The server has not been installed yet",
        )),
        AgentResult::Rejected(rejection) => Err(rejected(rejection)),
        _ => Err(agent_unavailable()),
    }
}

// Authenticates, requires access to the server, then runs a database action.
async fn with_access<T: Send + 'static>(
    state: &AppState,
    headers: &HeaderMap,
    server: ServerId,
    action: impl FnOnce(&std::path::Path, UserId) -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    state
        .authenticated(headers, move |path, actor| {
            ServerStore::open(path)
                .map_err(server_error)?
                .require_access(actor, server)
                .map_err(server_error)?;
            action(path, actor)
        })
        .await
}

async fn managed(
    state: &AppState,
    headers: &HeaderMap,
    server: ServerId,
) -> Result<(UserId, ServerDetail), ApiError> {
    with_access(state, headers, server, move |path, actor| {
        let detail = RuntimeStore::open(path)
            .and_then(|store| store.detail(server))
            .map_err(runtime_error)?;
        if detail.deleting {
            return Err(error(
                StatusCode::CONFLICT,
                "deleting",
                "The server is being deleted",
            ));
        }
        Ok((actor, detail))
    })
    .await
}

async fn record(
    state: &AppState,
    actor: UserId,
    server: ServerId,
    action: &'static str,
    outcome: &'static str,
    detail: Option<String>,
) {
    let path = state.database.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let now = now().unwrap_or(0);
        RuntimeStore::open(&path).and_then(|mut store| {
            store.record(Some(actor), server, action, outcome, detail.as_deref(), now)
        })
    })
    .await;
}

#[derive(Serialize)]
pub(super) struct DetailResponse {
    id: ServerId,
    name: String,
    owner: UserId,
    is_owner: bool,
    details: Option<ServerDetail>,
    runtime: Option<RuntimeStatus>,
    runtime_error: Option<&'static str>,
}

pub(super) async fn detail(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<DetailResponse>, ApiError> {
    let server = ServerId(id);
    let (record, actor, details) = state
        .authenticated(&headers, move |path, actor| {
            let record = ServerStore::open(path)
                .map_err(server_error)?
                .require_access(actor, server)
                .map_err(server_error)?;
            let details = match RuntimeStore::open(path).and_then(|store| store.detail(server)) {
                Ok(details) => Some(details),
                // Metadata-only servers (created before provisioning existed) have no details.
                Err(RuntimeError::NotFound) => None,
                Err(err) => return Err(runtime_error(err)),
            };
            Ok((record, actor, details))
        })
        .await?;
    let (runtime, runtime_error) = match &details {
        Some(details) if details.install_state != InstallState::Pending => {
            match runtime_status(&state, server).await {
                Ok(status) => (Some(status), None),
                Err(_) => (
                    None,
                    Some("The host agent did not report this server's state"),
                ),
            }
        }
        _ => (None, None),
    };
    Ok(Json(DetailResponse {
        id: record.id,
        name: record.name,
        is_owner: record.owner == actor,
        owner: record.owner,
        details,
        runtime,
        runtime_error,
    }))
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum PowerAction {
    Start,
    Stop,
    Restart,
    Kill,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PowerRequest {
    action: PowerAction,
}

pub(super) async fn power(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<PowerRequest>,
) -> Result<StatusCode, ApiError> {
    let server = ServerId(id);
    let (actor, detail) = managed(&state, &headers, server).await?;
    if detail.install_state != InstallState::Installed {
        return Err(error(
            StatusCode::CONFLICT,
            "not_installed",
            "The server can be controlled once installation has finished",
        ));
    }
    let action = match request.action {
        PowerAction::Start => "start",
        PowerAction::Stop => "stop",
        PowerAction::Restart => "restart",
        PowerAction::Kill => "kill",
    };
    let result = async {
        match request.action {
            PowerAction::Start => {
                let spec = spec(&state, server).await?;
                accepted(
                    &state,
                    Operation::StartServer {
                        spec: Box::new(spec),
                    },
                )
                .await
            }
            PowerAction::Stop => accepted(&state, Operation::StopServer { server_id: id }).await,
            PowerAction::Kill => accepted(&state, Operation::KillServer { server_id: id }).await,
            PowerAction::Restart => {
                let spec = spec(&state, server).await?;
                accepted(&state, Operation::StopServer { server_id: id }).await?;
                let agent = agent(&state)?.clone();
                tokio::spawn(async move {
                    // The agent force-stops after its grace period, so this wait is bounded.
                    for _ in 0..3600 {
                        if let Ok(AgentResult::ServerStatus(status)) =
                            agent.call(Operation::ServerStatus { server_id: id }).await
                        {
                            if status.power == manager_protocol::PowerState::Offline {
                                let _ = agent
                                    .call(Operation::StartServer {
                                        spec: Box::new(spec),
                                    })
                                    .await;
                                return;
                            }
                            if status.desired_running {
                                return; // someone started it again in the meantime
                            }
                        }
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                });
                Ok(())
            }
        }
    }
    .await;
    let outcome = if result.is_ok() {
        "requested"
    } else {
        "failed"
    };
    record(&state, actor, server, action, outcome, None).await;
    result.map(|_| StatusCode::ACCEPTED)
}

async fn spec(
    state: &AppState,
    server: ServerId,
) -> Result<manager_protocol::ServerSpec, ApiError> {
    let path = state.database.clone();
    tokio::task::spawn_blocking(move || {
        RuntimeStore::open(&path).and_then(|store| store.spec(server))
    })
    .await
    .map_err(|_| storage_error())?
    .map_err(runtime_error)
}

#[derive(Deserialize)]
pub(super) struct ConsoleQuery {
    lines: Option<u16>,
}

#[derive(Serialize)]
pub(super) struct ConsoleResponse {
    lines: Vec<String>,
}

pub(super) async fn console(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<ConsoleQuery>,
    headers: HeaderMap,
) -> Result<Json<ConsoleResponse>, ApiError> {
    managed(&state, &headers, ServerId(id)).await?;
    let lines = query
        .lines
        .unwrap_or(200)
        .clamp(1, manager_protocol::MAX_CONSOLE_LINES);
    match call(
        &state,
        Operation::ReadConsole {
            server_id: id,
            lines,
        },
    )
    .await?
    {
        AgentResult::Console(lines) => Ok(Json(ConsoleResponse { lines })),
        AgentResult::Rejected(rejection) if rejection.code == "not_found" => {
            Ok(Json(ConsoleResponse { lines: vec![] }))
        }
        AgentResult::Rejected(rejection) => Err(rejected(rejection)),
        _ => Err(agent_unavailable()),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CommandRequest {
    command: String,
}

pub(super) async fn command(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<CommandRequest>,
) -> Result<StatusCode, ApiError> {
    let server = ServerId(id);
    let (actor, _) = managed(&state, &headers, server).await?;
    let command = request.command.trim().trim_start_matches('/').to_owned();
    let result = accepted(
        &state,
        Operation::SendCommand {
            server_id: id,
            command,
        },
    )
    .await;
    // Command text can contain secrets (for example login plugins), so only the fact is logged.
    let outcome = if result.is_ok() {
        "succeeded"
    } else {
        "failed"
    };
    record(&state, actor, server, "console_command", outcome, None).await;
    result.map(|_| StatusCode::NO_CONTENT)
}

pub(super) async fn reinstall(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let server = ServerId(id);
    managed(&state, &headers, server).await?;
    with_access(&state, &headers, server, move |path, actor| {
        let mut store = RuntimeStore::open(path).map_err(runtime_error)?;
        store.requeue_install(server).map_err(runtime_error)?;
        store
            .record(Some(actor), server, "reinstall", "requested", None, now()?)
            .map_err(runtime_error)
    })
    .await?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeleteRequest {
    confirm_name: String,
    #[serde(default)]
    keep_backups: bool,
}

pub(super) async fn delete(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<DeleteRequest>,
) -> Result<StatusCode, ApiError> {
    let server = ServerId(id);
    with_access(&state, &headers, server, move |path, actor| {
        let name: String = ServerStore::open(path)
            .map_err(server_error)?
            .require_access(actor, server)
            .map_err(server_error)?
            .name;
        if request.confirm_name != name {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "confirmation_mismatch",
                "Type the server's exact name to delete it",
            ));
        }
        let mut store = RuntimeStore::open(path).map_err(runtime_error)?;
        if store
            .mark_deleting(server, request.keep_backups)
            .map_err(runtime_error)?
        {
            let detail = request.keep_backups.then_some("backups kept");
            store
                .record(Some(actor), server, "delete", "requested", detail, now()?)
                .map_err(runtime_error)?;
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::ACCEPTED)
}

pub(super) async fn activity(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<Vec<crate::runtime::ActivityEntry>>, ApiError> {
    let server = ServerId(id);
    with_access(&state, &headers, server, move |path, _| {
        RuntimeStore::open(path)
            .and_then(|store| store.activity(server, 50))
            .map_err(runtime_error)
    })
    .await
    .map(Json)
}

#[derive(Serialize)]
pub(super) struct ExtensionsResponse {
    game: Option<DetectedGame>,
    available: bool,
    reason: Option<String>,
    managed: Vec<ExtensionRecord>,
    other_files: Vec<String>,
}

pub(super) async fn extensions(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ExtensionsResponse>, ApiError> {
    let server = ServerId(id);
    managed(&state, &headers, server).await?;
    let status = runtime_status(&state, server).await?;
    let mut managed_records = with_access(&state, &headers, server, move |path, _| {
        RuntimeStore::open(path)
            .and_then(|store| store.extensions(server))
            .map_err(runtime_error)
    })
    .await?;
    let Some(game) = status.game.clone() else {
        return Ok(Json(ExtensionsResponse {
            game: None,
            available: false,
            reason: Some("The installed game version could not be detected".into()),
            managed: managed_records,
            other_files: vec![],
        }));
    };
    let folder = match target(&game) {
        Ok((_, folder)) => folder,
        Err(err) => {
            return Ok(Json(ExtensionsResponse {
                game: Some(game),
                available: false,
                reason: Some(err.to_string()),
                managed: managed_records,
                other_files: vec![],
            }));
        }
    };
    let files = match call(
        &state,
        Operation::ListFiles {
            server_id: id,
            folder,
        },
    )
    .await?
    {
        AgentResult::Files(files) => files,
        AgentResult::Rejected(rejection) => return Err(rejected(rejection)),
        _ => return Err(agent_unavailable()),
    };
    // Records whose file was removed by hand are dropped, unless a change is still running.
    let busy = status
        .operation
        .as_ref()
        .is_some_and(|op| op.state == OperationState::Running);
    let missing: Vec<String> = managed_records
        .iter()
        .filter(|r| !files.contains(&r.filename))
        .map(|r| r.project_id.clone())
        .collect();
    if !busy && !missing.is_empty() {
        let removed = missing.clone();
        with_access(&state, &headers, server, move |path, _| {
            RuntimeStore::open(path)
                .and_then(|mut store| store.record_extensions(server, &[], &removed))
                .map_err(runtime_error)
        })
        .await?;
        managed_records.retain(|r| !missing.contains(&r.project_id));
    }
    let other_files = files
        .into_iter()
        .filter(|f| !managed_records.iter().any(|r| &r.filename == f))
        .collect();
    Ok(Json(ExtensionsResponse {
        game: Some(game),
        available: true,
        reason: None,
        managed: managed_records,
        other_files,
    }))
}

#[derive(Deserialize)]
pub(super) struct SearchQuery {
    query: Option<String>,
}

pub(super) async fn search_modrinth(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<SearchQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<SearchHit>>, ApiError> {
    let server = ServerId(id);
    managed(&state, &headers, server).await?;
    let game = runtime_status(&state, server).await?.game.ok_or_else(|| {
        error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "incompatible",
            "The installed game version could not be detected",
        )
    })?;
    state
        .modrinth
        .search(query.query.as_deref().unwrap_or(""), &game)
        .await
        .map(Json)
        .map_err(modrinth_error)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AddExtension {
    project_id: String,
}

#[derive(Serialize)]
pub(super) struct ChangeResponse {
    installing: Vec<ExtensionRecord>,
    removing: Vec<String>,
}

// Waits for the agent's file operation, then records what is now installed.
fn finish_change(
    state: &AppState,
    actor: UserId,
    server: ServerId,
    installed: Vec<ExtensionRecord>,
    removed_projects: Vec<String>,
    action: &'static str,
) {
    let (Some(agent), path) = (state.agent.clone(), state.database.clone()) else {
        return;
    };
    tokio::spawn(async move {
        let mut outcome = Err("timed out waiting for the host agent".to_string());
        for _ in 0..900 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if let Ok(AgentResult::ServerStatus(status)) = agent
                .call(Operation::ServerStatus {
                    server_id: server.0,
                })
                .await
                && let Some(op) = status.operation
                && op.kind == OperationKind::Extensions
                && op.state != OperationState::Running
            {
                outcome = if op.state == OperationState::Succeeded {
                    Ok(op.message)
                } else {
                    Err(op.message)
                };
                break;
            }
        }
        let _ = tokio::task::spawn_blocking(move || {
            let mut store = RuntimeStore::open(&path)?;
            let at = now().unwrap_or(0);
            match outcome {
                Ok(message) => {
                    store.record_extensions(server, &installed, &removed_projects)?;
                    store.record(Some(actor), server, action, "succeeded", Some(&message), at)
                }
                Err(message) => {
                    store.record(Some(actor), server, action, "failed", Some(&message), at)
                }
            }
        })
        .await;
    });
}

pub(super) async fn add_extension(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<AddExtension>,
) -> Result<(StatusCode, Json<ChangeResponse>), ApiError> {
    let server = ServerId(id);
    let (actor, _) = managed(&state, &headers, server).await?;
    let status = runtime_status(&state, server).await?;
    if status.desired_running || status.power != manager_protocol::PowerState::Offline {
        return Err(error(
            StatusCode::CONFLICT,
            "server_running",
            "Stop the server before changing mods or plugins",
        ));
    }
    let game = status.game.ok_or_else(|| {
        error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "incompatible",
            "The installed game version could not be detected",
        )
    })?;
    let installed = with_access(&state, &headers, server, move |path, _| {
        RuntimeStore::open(path)
            .and_then(|store| store.extensions(server))
            .map_err(runtime_error)
    })
    .await?;
    let plan = state
        .modrinth
        .plan(&request.project_id, &game, &installed)
        .await
        .map_err(modrinth_error)?;
    if plan.files.is_empty() && plan.remove.is_empty() {
        return Ok((
            StatusCode::OK,
            Json(ChangeResponse {
                installing: vec![],
                removing: vec![],
            }),
        ));
    }
    accepted(
        &state,
        Operation::InstallFiles {
            server_id: id,
            folder: plan.folder,
            files: plan.files,
            remove: plan.remove.clone(),
        },
    )
    .await?;
    finish_change(
        &state,
        actor,
        server,
        plan.records.clone(),
        vec![],
        "install_extension",
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(ChangeResponse {
            installing: plan.records,
            removing: plan.remove,
        }),
    ))
}

pub(super) async fn remove_extension(
    State(state): State<AppState>,
    Path((id, project)): Path<(Uuid, String)>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<ChangeResponse>), ApiError> {
    let server = ServerId(id);
    let (actor, _) = managed(&state, &headers, server).await?;
    let project_id = project.clone();
    let record = with_access(&state, &headers, server, move |path, _| {
        RuntimeStore::open(path)
            .and_then(|store| store.extensions(server))
            .map_err(runtime_error)?
            .into_iter()
            .find(|r| r.project_id == project_id)
            .ok_or_else(|| {
                error(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    "That extension is not installed",
                )
            })
    })
    .await?;
    let status = runtime_status(&state, server).await?;
    let game = status.game.ok_or_else(|| {
        error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "incompatible",
            "The installed game version could not be detected",
        )
    })?;
    let (_, folder) = target(&game).map_err(modrinth_error)?;
    accepted(
        &state,
        Operation::InstallFiles {
            server_id: id,
            folder,
            files: vec![],
            remove: vec![record.filename.clone()],
        },
    )
    .await?;
    finish_change(
        &state,
        actor,
        server,
        vec![],
        vec![project],
        "remove_extension",
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(ChangeResponse {
            installing: vec![],
            removing: vec![record.filename],
        }),
    ))
}

pub(super) async fn qualify_revision(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    state
        .authenticated(&headers, move |path, actor| {
            let at = now()?;
            RuntimeStore::open(path)
                .and_then(|mut store| store.qualify(actor, EggRevisionId(id), at))
                .map_err(runtime_error)
        })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
