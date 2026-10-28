// File manager routes. The API never touches server files: every operation goes to the agent,
// which enforces containment. Each request re-checks access; streaming downloads re-check while
// they run.
use super::{call, managed, record};
use crate::http::{ApiError, AppState, ErrorBody, error, storage_error};
use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::Engine;
use manager_core::model::ServerId;
use manager_protocol::{AgentResult, FileEntry, Operation, Rejection};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use uuid::Uuid;

const CHUNK: u32 = 1024 * 1024;
const RECHECK_CHUNKS: u64 = 8;

fn file_rejection(rejection: Rejection) -> ApiError {
    let status = match rejection.code.as_str() {
        "not_found" => StatusCode::NOT_FOUND,
        "invalid_path" | "invalid" => StatusCode::BAD_REQUEST,
        "denied" => StatusCode::FORBIDDEN,
        "disk_full" => StatusCode::INSUFFICIENT_STORAGE,
        _ => StatusCode::CONFLICT,
    };
    let code = match rejection.code.as_str() {
        "not_found" => "not_found",
        "invalid_path" | "invalid" => "invalid_path",
        "denied" => "denied",
        "disk_full" => "disk_full",
        "conflict" => "conflict",
        "busy" => "server_busy",
        "not_empty" => "not_empty",
        _ => "files",
    };
    (
        status,
        Json(ErrorBody {
            code,
            message: Cow::Owned(rejection.message),
            details: None,
        }),
    )
}

async fn file_call(state: &AppState, operation: Operation) -> Result<AgentResult, ApiError> {
    match call(state, operation).await? {
        AgentResult::Rejected(rejection) => Err(file_rejection(rejection)),
        result => Ok(result),
    }
}

#[derive(Deserialize)]
pub(in crate::http) struct PathQuery {
    #[serde(default)]
    path: String,
}

pub(in crate::http) async fn list(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<FileEntry>>, ApiError> {
    managed(&state, &headers, ServerId(id)).await?;
    match file_call(
        &state,
        Operation::ListDirectory {
            server_id: id,
            path: query.path,
        },
    )
    .await?
    {
        AgentResult::Directory(entries) => Ok(Json(entries)),
        _ => Err(storage_error()),
    }
}

#[derive(Serialize)]
pub(in crate::http) struct TextFile {
    text: String,
    version: String,
}

pub(in crate::http) async fn read_text(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
) -> Result<Json<TextFile>, ApiError> {
    managed(&state, &headers, ServerId(id)).await?;
    match file_call(
        &state,
        Operation::ReadText {
            server_id: id,
            path: query.path,
        },
    )
    .await?
    {
        AgentResult::Text { text, version } => Ok(Json(TextFile { text, version })),
        _ => Err(storage_error()),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::http) struct SaveText {
    path: String,
    text: String,
    version: Option<String>,
}

#[derive(Serialize)]
pub(in crate::http) struct Saved {
    version: String,
}

pub(in crate::http) async fn save_text(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<SaveText>,
) -> Result<Json<Saved>, ApiError> {
    let server = ServerId(id);
    let (actor, _) = managed(&state, &headers, server).await?;
    let path = request.path.clone();
    let result = file_call(
        &state,
        Operation::WriteText {
            server_id: id,
            path: request.path,
            text: request.text,
            expected_version: request.version,
        },
    )
    .await;
    record(
        &state,
        actor,
        server,
        "edit_file",
        if result.is_ok() {
            "succeeded"
        } else {
            "failed"
        },
        Some(path),
    )
    .await;
    match result? {
        AgentResult::Saved { version } => Ok(Json(Saved { version })),
        _ => Err(storage_error()),
    }
}

fn file_name(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or("download");
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-+".contains(c) {
                c
            } else {
                '_'
            }
        })
        .take(100)
        .collect();
    if cleaned.is_empty() {
        "download".into()
    } else {
        cleaned
    }
}

pub(in crate::http) async fn download(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let server = ServerId(id);
    managed(&state, &headers, server).await?;
    let engine = base64::engine::general_purpose::STANDARD;
    let (first, size) = match file_call(
        &state,
        Operation::ReadChunk {
            server_id: id,
            path: query.path.clone(),
            offset: 0,
            length: CHUNK,
        },
    )
    .await?
    {
        AgentResult::Chunk { data, size } => {
            (engine.decode(data).map_err(|_| storage_error())?, size)
        }
        _ => return Err(storage_error()),
    };
    let path = query.path.clone();
    let start = first.len() as u64;
    let stream = futures_util::stream::unfold(
        (
            Some(first),
            start,
            0u64,
            state.clone(),
            headers.clone(),
            false,
        ),
        move |(pending, offset, chunks, state, headers, done)| {
            let path = path.clone();
            async move {
                if let Some(bytes) = pending {
                    return Some((
                        Ok(Bytes::from(bytes)),
                        (None, offset, chunks + 1, state, headers, done),
                    ));
                }
                if done || offset >= size {
                    return None;
                }
                if chunks % RECHECK_CHUNKS == 0 && managed(&state, &headers, server).await.is_err()
                {
                    return Some((
                        Err(std::io::Error::other("access revoked")),
                        (None, offset, chunks, state, headers, true),
                    ));
                }
                let length = CHUNK.min((size - offset) as u32);
                match call(
                    &state,
                    Operation::ReadChunk {
                        server_id: id,
                        path,
                        offset,
                        length,
                    },
                )
                .await
                {
                    Ok(AgentResult::Chunk { data, .. }) => {
                        match base64::engine::general_purpose::STANDARD.decode(data) {
                            Ok(bytes) if !bytes.is_empty() => {
                                let next = offset + bytes.len() as u64;
                                Some((
                                    Ok(Bytes::from(bytes)),
                                    (None, next, chunks + 1, state, headers, false),
                                ))
                            }
                            // The file shrank while downloading; the length mismatch reports it.
                            _ => Some((
                                Err(std::io::Error::other("file changed")),
                                (None, offset, chunks, state, headers, true),
                            )),
                        }
                    }
                    _ => Some((
                        Err(std::io::Error::other("read failed")),
                        (None, offset, chunks, state, headers, true),
                    )),
                }
            }
        },
    );
    let mut response = Body::from_stream(stream).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"{}\"",
            file_name(&query.path)
        ))
        .map_err(|_| storage_error())?,
    );
    Ok(response)
}

pub(in crate::http) async fn upload(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
    body: Body,
) -> Result<StatusCode, ApiError> {
    use futures_util::StreamExt;
    let server = ServerId(id);
    let (actor, _) = managed(&state, &headers, server).await?;
    let engine = base64::engine::general_purpose::STANDARD;
    let upload_id = Uuid::new_v4();
    let mut stream = body.into_data_stream();
    let mut buffer: Vec<u8> = Vec::with_capacity(CHUNK as usize);
    let mut offset = 0u64;
    let result: Result<(), ApiError> = async {
        loop {
            let next = stream.next().await;
            let finished = next.is_none();
            if let Some(chunk) = next {
                let chunk = chunk.map_err(|_| {
                    error(
                        StatusCode::BAD_REQUEST,
                        "upload_failed",
                        "The upload was interrupted",
                    )
                })?;
                buffer.extend_from_slice(&chunk);
            }
            while buffer.len() > CHUNK as usize || (finished && (offset == 0 || !buffer.is_empty()))
            {
                let take = buffer.len().min(CHUNK as usize);
                let piece: Vec<u8> = buffer.drain(..take).collect();
                let last = finished && buffer.is_empty();
                file_call(
                    &state,
                    Operation::WriteChunk {
                        server_id: id,
                        upload_id,
                        path: query.path.clone(),
                        offset,
                        data: engine.encode(&piece),
                        last,
                    },
                )
                .await?;
                offset += piece.len() as u64;
                if last {
                    return Ok(());
                }
            }
            if finished {
                return Ok(());
            }
        }
    }
    .await;
    let detail = format!("{} ({} bytes)", query.path, offset);
    record(
        &state,
        actor,
        server,
        "upload_file",
        if result.is_ok() {
            "succeeded"
        } else {
            "failed"
        },
        Some(detail),
    )
    .await;
    result.map(|_| StatusCode::CREATED)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::http) struct NewFolder {
    path: String,
}

pub(in crate::http) async fn create_folder(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<NewFolder>,
) -> Result<StatusCode, ApiError> {
    let server = ServerId(id);
    let (actor, _) = managed(&state, &headers, server).await?;
    let result = file_call(
        &state,
        Operation::CreateDirectory {
            server_id: id,
            path: request.path.clone(),
        },
    )
    .await;
    record(
        &state,
        actor,
        server,
        "create_folder",
        if result.is_ok() {
            "succeeded"
        } else {
            "failed"
        },
        Some(request.path),
    )
    .await;
    result.map(|_| StatusCode::CREATED)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::http) struct RenameRequest {
    from: String,
    to: String,
}

pub(in crate::http) async fn rename(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<RenameRequest>,
) -> Result<StatusCode, ApiError> {
    let server = ServerId(id);
    let (actor, _) = managed(&state, &headers, server).await?;
    let detail = format!("{} -> {}", request.from, request.to);
    let result = file_call(
        &state,
        Operation::RenamePath {
            server_id: id,
            from: request.from,
            to: request.to,
        },
    )
    .await;
    record(
        &state,
        actor,
        server,
        "rename_file",
        if result.is_ok() {
            "succeeded"
        } else {
            "failed"
        },
        Some(detail),
    )
    .await;
    result.map(|_| StatusCode::NO_CONTENT)
}

pub(in crate::http) async fn remove(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<PathQuery>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let server = ServerId(id);
    let (actor, _) = managed(&state, &headers, server).await?;
    let result = file_call(
        &state,
        Operation::DeletePath {
            server_id: id,
            path: query.path.clone(),
        },
    )
    .await;
    record(
        &state,
        actor,
        server,
        "delete_file",
        if result.is_ok() {
            "succeeded"
        } else {
            "failed"
        },
        Some(query.path),
    )
    .await;
    result.map(|_| StatusCode::NO_CONTENT)
}
