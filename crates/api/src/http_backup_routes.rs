// Backup routes. Backups of a live server need access to that server; backups kept after a
// deletion need the deletion-time access list (or administrator). Downloads re-check access
// while streaming so revoking access stops a download that is already running.
use super::{accepted, call, managed, record, rejected, runtime_error};
use crate::{
    http::{ApiError, AppState, error, server_error, storage_error},
    runtime::RuntimeStore,
    servers::{ServerError, ServerStore},
};
use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use manager_core::model::{ServerId, UserId};
use manager_protocol::{AgentResult, BackupInfo, Operation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const RECHECK_BYTES: u64 = 8 * 1024 * 1024;

// Access to a server's backups: returns the server name, from the live server or the retained record.
async fn backup_access(
    state: &AppState,
    headers: &HeaderMap,
    server: ServerId,
) -> Result<(UserId, String), ApiError> {
    state
        .authenticated(headers, move |path, actor| {
            match ServerStore::open(path)
                .map_err(server_error)?
                .require_access(actor, server)
            {
                Ok(record) => Ok((actor, record.name)),
                Err(ServerError::NotFound) => RuntimeStore::open(path)
                    .and_then(|store| store.retained_access(actor, server))
                    .map_err(runtime_error)?
                    .map(|name| (actor, name))
                    .ok_or_else(|| error(StatusCode::NOT_FOUND, "not_found", "Server not found")),
                Err(err) => Err(server_error(err)),
            }
        })
        .await
}

async fn agent_backups(state: &AppState, server: ServerId) -> Result<Vec<BackupInfo>, ApiError> {
    match call(
        state,
        Operation::ListBackups {
            server_id: server.0,
        },
    )
    .await?
    {
        AgentResult::Backups(backups) => Ok(backups),
        AgentResult::Rejected(rejection) => Err(rejected(rejection)),
        _ => Err(storage_error()),
    }
}

#[derive(Serialize)]
pub(in crate::http) struct BackupList {
    retention: u8,
    backups: Vec<BackupInfo>,
}

pub(in crate::http) async fn list(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<BackupList>, ApiError> {
    let server = ServerId(id);
    backup_access(&state, &headers, server).await?;
    let path = state.database.clone();
    let retention = tokio::task::spawn_blocking(move || {
        RuntimeStore::open(&path).and_then(|s| s.retention(server))
    })
    .await
    .map_err(|_| storage_error())?
    .map_err(runtime_error)?;
    Ok(Json(BackupList {
        retention,
        backups: agent_backups(&state, server).await?,
    }))
}

pub(in crate::http) async fn create(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let server = ServerId(id);
    let (actor, _) = managed(&state, &headers, server).await?;
    let path = state.database.clone();
    let retention = tokio::task::spawn_blocking(move || {
        RuntimeStore::open(&path).and_then(|s| s.retention(server))
    })
    .await
    .map_err(|_| storage_error())?
    .map_err(runtime_error)?;
    let result = accepted(
        &state,
        Operation::CreateBackup {
            server_id: id,
            retention,
        },
    )
    .await;
    record(
        &state,
        actor,
        server,
        "backup",
        if result.is_ok() {
            "requested"
        } else {
            "failed"
        },
        None,
    )
    .await;
    result.map(|_| StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::http) struct RetentionRequest {
    retention: u8,
    // Lowering the limit below the current number of backups removes the oldest ones; the user
    // must confirm that explicitly.
    #[serde(default)]
    remove_oldest: bool,
}

#[derive(Serialize)]
pub(in crate::http) struct RetentionResponse {
    retention: u8,
    removed: Vec<Uuid>,
}

pub(in crate::http) async fn set_retention(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<RetentionRequest>,
) -> Result<Json<RetentionResponse>, ApiError> {
    let server = ServerId(id);
    let (actor, _) = managed(&state, &headers, server).await?;
    if !(1..=5).contains(&request.retention) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "Keep between 1 and 5 backups",
        ));
    }
    let manual: Vec<BackupInfo> = agent_backups(&state, server)
        .await?
        .into_iter()
        .filter(|b| b.kind == manager_protocol::BackupKind::Manual)
        .collect();
    let excess: Vec<Uuid> = manual
        .iter()
        .skip(request.retention as usize)
        .map(|b| b.id)
        .collect();
    if !excess.is_empty() && !request.remove_oldest {
        return Err(error(
            StatusCode::CONFLICT,
            "confirm_removal",
            "Lowering the limit removes the oldest backups; confirm to continue",
        ));
    }
    for backup in &excess {
        accepted(
            &state,
            Operation::DeleteBackup {
                server_id: id,
                backup_id: *backup,
            },
        )
        .await?;
    }
    let path = state.database.clone();
    let retention = request.retention;
    let detail = format!("keep {retention}, removed {}", excess.len());
    tokio::task::spawn_blocking(move || {
        RuntimeStore::open(&path).and_then(|mut s| s.set_retention(server, retention))
    })
    .await
    .map_err(|_| storage_error())?
    .map_err(runtime_error)?;
    record(
        &state,
        actor,
        server,
        "backup_retention",
        "succeeded",
        Some(detail),
    )
    .await;
    Ok(Json(RetentionResponse {
        retention,
        removed: excess,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::http) struct RestoreRequest {
    confirm: bool,
}

pub(in crate::http) async fn restore(
    State(state): State<AppState>,
    Path((id, backup)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(request): Json<RestoreRequest>,
) -> Result<StatusCode, ApiError> {
    let server = ServerId(id);
    let (actor, _) = managed(&state, &headers, server).await?;
    if !request.confirm {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "confirmation_required",
            "Confirm the restore to replace the server's current data",
        ));
    }
    let result = accepted(
        &state,
        Operation::RestoreBackup {
            server_id: id,
            backup_id: backup,
        },
    )
    .await;
    record(
        &state,
        actor,
        server,
        "restore",
        if result.is_ok() {
            "requested"
        } else {
            "failed"
        },
        None,
    )
    .await;
    result.map(|_| StatusCode::ACCEPTED)
}

pub(in crate::http) async fn remove(
    State(state): State<AppState>,
    Path((id, backup)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let server = ServerId(id);
    let (actor, _) = backup_access(&state, &headers, server).await?;
    accepted(
        &state,
        Operation::DeleteBackup {
            server_id: id,
            backup_id: backup,
        },
    )
    .await?;
    let remaining = agent_backups(&state, server).await?;
    let path = state.database.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let mut store = RuntimeStore::open(&path)?;
        let live = ServerStore::open(&path)
            .map(|s| s.exists(server))
            .unwrap_or(true);
        if live {
            store.record(
                Some(actor),
                server,
                "delete_backup",
                "succeeded",
                None,
                crate::runtime::unix_now(),
            )
        } else if remaining.is_empty() {
            store.forget_retained(server)
        } else {
            Ok(())
        }
    })
    .await;
    Ok(StatusCode::NO_CONTENT)
}

fn safe_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(60)
        .collect();
    if cleaned.trim_matches('-').is_empty() {
        "server".into()
    } else {
        cleaned
    }
}

pub(in crate::http) async fn download(
    State(state): State<AppState>,
    Path((id, backup)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    use tokio::io::AsyncReadExt;
    let server = ServerId(id);
    let (_, name) = backup_access(&state, &headers, server).await?;
    // Both IDs are UUIDs, so the path cannot leave the backup root.
    let file_path = state
        .backup_root
        .join(id.to_string())
        .join(format!("{backup}.tar.gz"));
    let file = tokio::fs::File::open(&file_path)
        .await
        .map_err(|_| error(StatusCode::NOT_FOUND, "not_found", "Backup not found"))?;
    let length = file.metadata().await.map_err(|_| storage_error())?.len();
    let stream = futures_util::stream::unfold(
        (file, 0u64, state.clone(), headers.clone(), false),
        move |(mut file, sent, state, headers, done)| async move {
            if done {
                return None;
            }
            if sent > 0
                && sent % RECHECK_BYTES == 0
                && backup_access(&state, &headers, server).await.is_err()
            {
                // Ending early leaves the response shorter than Content-Length, so it fails.
                return Some((
                    Err(std::io::Error::other("access revoked")),
                    (file, sent, state, headers, true),
                ));
            }
            let mut buffer = vec![0u8; 64 * 1024];
            match file.read(&mut buffer).await {
                Ok(0) => None,
                Ok(read) => {
                    buffer.truncate(read);
                    Some((
                        Ok(Bytes::from(buffer)),
                        (file, sent + read as u64, state, headers, false),
                    ))
                }
                Err(err) => Some((Err(err), (file, sent, state, headers, true))),
            }
        },
    );
    let mut response = Body::from_stream(stream).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/gzip"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    let filename = format!(
        "{}-{}.tar.gz",
        safe_filename(&name),
        &backup.simple().to_string()[..8]
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .map_err(|_| storage_error())?,
    );
    Ok(response)
}

#[derive(Serialize)]
pub(in crate::http) struct RetainedServer {
    server_id: ServerId,
    name: String,
    deleted_at: i64,
    backups: Vec<BackupInfo>,
}

pub(in crate::http) async fn retained(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<RetainedServer>>, ApiError> {
    let servers = state
        .authenticated(&headers, |path, actor| {
            RuntimeStore::open(path)
                .and_then(|s| s.retained_for(actor))
                .map_err(runtime_error)
        })
        .await?;
    let mut result = vec![];
    for (server, name, deleted_at) in servers {
        let backups = agent_backups(&state, server).await.unwrap_or_default();
        result.push(RetainedServer {
            server_id: server,
            name,
            deleted_at,
            backups,
        });
    }
    Ok(Json(result))
}

// Uploads a .tar.gz of server files (for example a copy of a Pterodactyl server's volume) as an
// imported backup. Nothing changes on the server until someone restores it.
pub(in crate::http) async fn import(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Body,
) -> Result<(StatusCode, Json<BackupInfo>), ApiError> {
    use base64::Engine;
    use futures_util::StreamExt;
    const CHUNK: usize = 1024 * 1024;
    let server = ServerId(id);
    let (actor, _) = managed(&state, &headers, server).await?;
    let engine = base64::engine::general_purpose::STANDARD;
    let upload_id = Uuid::new_v4();
    let mut stream = body.into_data_stream();
    let mut buffer: Vec<u8> = Vec::with_capacity(CHUNK);
    let mut offset = 0u64;
    let result: Result<BackupInfo, ApiError> = async {
        loop {
            let next = stream.next().await;
            let finished = next.is_none();
            if let Some(chunk) = next {
                buffer.extend_from_slice(&chunk.map_err(|_| {
                    error(
                        StatusCode::BAD_REQUEST,
                        "upload_failed",
                        "The upload was interrupted",
                    )
                })?);
            }
            while buffer.len() > CHUNK || (finished && (offset == 0 || !buffer.is_empty())) {
                let piece: Vec<u8> = buffer.drain(..buffer.len().min(CHUNK)).collect();
                let last = finished && buffer.is_empty();
                let answer = call(
                    &state,
                    Operation::ImportChunk {
                        server_id: id,
                        upload_id,
                        offset,
                        data: engine.encode(&piece),
                        last,
                    },
                )
                .await?;
                offset += piece.len() as u64;
                match answer {
                    AgentResult::Accepted if !last => {}
                    AgentResult::Backups(mut imported) if last && imported.len() == 1 => {
                        return Ok(imported.remove(0));
                    }
                    AgentResult::Rejected(rejection) if rejection.code == "invalid" => {
                        return Err((
                            StatusCode::UNPROCESSABLE_ENTITY,
                            Json(crate::http::ErrorBody {
                                code: "invalid_archive",
                                message: std::borrow::Cow::Owned(rejection.message),
                                details: None,
                            }),
                        ));
                    }
                    AgentResult::Rejected(rejection) => return Err(rejected(rejection)),
                    _ => return Err(storage_error()),
                }
            }
            if finished {
                return Err(storage_error());
            }
        }
    }
    .await;
    record(
        &state,
        actor,
        server,
        "import_files",
        if result.is_ok() {
            "succeeded"
        } else {
            "failed"
        },
        Some(format!("{} MiB archive", offset.div_ceil(1024 * 1024))),
    )
    .await;
    result.map(|info| (StatusCode::CREATED, Json(info)))
}
