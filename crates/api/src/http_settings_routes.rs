// Startup settings, resource limits and members. Changes are stored and take effect at the next
// start; a running server keeps the settings it was started with.
use super::{accepted, managed, record, runtime_error, spec, with_access};
use crate::{
    http::{ApiError, AppState, ErrorBody, error, now, server_error, storage_error},
    runtime::{InstallState, RuntimeStore, Settings},
    schedules::{Action, Schedule, ScheduleError, ScheduleInput, ScheduleStore},
    servers::{Member, Person, ServerStore},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use manager_core::{capacity::Resources, model::ServerId};
use manager_protocol::Operation;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;
use uuid::Uuid;

pub(in crate::http) async fn settings(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<Settings>, ApiError> {
    let server = ServerId(id);
    with_access(&state, &headers, server, move |path, _| {
        RuntimeStore::open(path)
            .and_then(|store| store.settings(server))
            .map_err(runtime_error)
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::http) struct SettingsChange {
    image: String,
    #[serde(default)]
    variables: BTreeMap<String, String>,
}

#[derive(Serialize)]
pub(in crate::http) struct Changed {
    changed: Vec<String>,
}

pub(in crate::http) async fn save_settings(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(change): Json<SettingsChange>,
) -> Result<Json<Changed>, ApiError> {
    let server = ServerId(id);
    let (actor, _) = managed(&state, &headers, server).await?;
    let changed = with_access(&state, &headers, server, move |path, _| {
        RuntimeStore::open(path)
            .and_then(|mut store| store.update_settings(server, &change.image, &change.variables))
            .map_err(runtime_error)
    })
    .await?;
    if !changed.is_empty() {
        // Names only: values can be secrets (RCON passwords, tokens).
        record(
            &state,
            actor,
            server,
            "change_settings",
            "succeeded",
            Some(changed.join(", ")),
        )
        .await;
    }
    Ok(Json(Changed { changed }))
}

pub(in crate::http) async fn save_limits(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(limits): Json<Resources>,
) -> Result<Json<Resources>, ApiError> {
    let server = ServerId(id);
    let (actor, _) = managed(&state, &headers, server).await?;
    let saved = state
        .provisioning(&headers, move |store, _, status, now| {
            store.change_limits(server, limits, &status.admission, now)
        })
        .await?;
    let gib = |bytes: u64| bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    record(
        &state,
        actor,
        server,
        "change_limits",
        "succeeded",
        Some(format!(
            "{:.1} GiB RAM, {:.1} GiB disk, {:.2} CPU",
            gib(saved.memory_bytes),
            gib(saved.disk_bytes),
            saved.cpu_millis as f64 / 1000.0
        )),
    )
    .await;
    Ok(Json(saved))
}

pub(in crate::http) async fn members(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<Vec<Member>>, ApiError> {
    state
        .authenticated(&headers, move |path, actor| {
            ServerStore::open(path)
                .and_then(|store| store.members(actor, ServerId(id)))
                .map_err(server_error)
        })
        .await
        .map(Json)
}

pub(in crate::http) async fn people(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Person>>, ApiError> {
    state
        .authenticated(&headers, move |path, actor| {
            ServerStore::open(path)
                .and_then(|store| store.people(actor))
                .map_err(server_error)
        })
        .await
        .map(Json)
}

fn schedule_error(err: ScheduleError) -> ApiError {
    match err {
        ScheduleError::NotFound => error(StatusCode::NOT_FOUND, "not_found", "Schedule not found"),
        ScheduleError::Invalid(message) => (
            StatusCode::BAD_REQUEST,
            Json(ErrorBody {
                code: "invalid_schedule",
                message: Cow::Owned(message),
                details: None,
            }),
        ),
        ScheduleError::Storage(_) => storage_error(),
    }
}

pub(in crate::http) async fn schedules(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<Vec<Schedule>>, ApiError> {
    let server = ServerId(id);
    with_access(&state, &headers, server, move |path, _| {
        ScheduleStore::open(path)
            .and_then(|store| store.list(server))
            .map_err(schedule_error)
    })
    .await
    .map(Json)
}

fn describe(input: &ScheduleInput) -> String {
    let action = match input.action {
        Action::Restart => "restart".to_string(),
        Action::Backup => "backup".to_string(),
        Action::Command => "console command".to_string(),
        Action::Update => "update".to_string(),
    };
    format!(
        "{action} at {:02}:{:02} {}{}",
        input.hour,
        input.minute,
        input.timezone,
        if input.enabled { "" } else { " (paused)" }
    )
}

pub(in crate::http) async fn create_schedule(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<ScheduleInput>,
) -> Result<(StatusCode, Json<Schedule>), ApiError> {
    let server = ServerId(id);
    let (actor, _) = managed(&state, &headers, server).await?;
    let detail = describe(&input);
    let created = with_access(&state, &headers, server, move |path, actor| {
        let at = now()?;
        ScheduleStore::open(path)
            .and_then(|mut store| store.create(server, actor.0, input, at))
            .map_err(schedule_error)
    })
    .await?;
    record(
        &state,
        actor,
        server,
        "create_schedule",
        "succeeded",
        Some(detail),
    )
    .await;
    Ok((StatusCode::CREATED, Json(created)))
}

pub(in crate::http) async fn update_schedule(
    State(state): State<AppState>,
    Path((id, schedule)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(input): Json<ScheduleInput>,
) -> Result<StatusCode, ApiError> {
    let server = ServerId(id);
    let (actor, _) = managed(&state, &headers, server).await?;
    let detail = describe(&input);
    with_access(&state, &headers, server, move |path, _| {
        let at = now()?;
        ScheduleStore::open(path)
            .and_then(|mut store| store.update(server, schedule, input, at))
            .map_err(schedule_error)
    })
    .await?;
    record(
        &state,
        actor,
        server,
        "change_schedule",
        "succeeded",
        Some(detail),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

pub(in crate::http) async fn delete_schedule(
    State(state): State<AppState>,
    Path((id, schedule)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let server = ServerId(id);
    let (actor, _) = managed(&state, &headers, server).await?;
    with_access(&state, &headers, server, move |path, _| {
        ScheduleStore::open(path)
            .and_then(|mut store| store.delete(server, schedule))
            .map_err(schedule_error)
    })
    .await?;
    record(&state, actor, server, "delete_schedule", "succeeded", None).await;
    Ok(StatusCode::NO_CONTENT)
}

// Runs the egg installer again on the existing files after a pre-update backup. A manual update
// may change the game version when the settings ask for another one.
pub(in crate::http) async fn update_server(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let server = ServerId(id);
    let (actor, detail) = managed(&state, &headers, server).await?;
    if detail.install_state != InstallState::Installed {
        return Err(error(
            StatusCode::CONFLICT,
            "not_installed",
            "The server can be updated once installation has finished",
        ));
    }
    let spec = spec(&state, server).await?;
    let result = accepted(
        &state,
        Operation::UpdateServer {
            spec: Box::new(spec),
            keep_game_version: false,
        },
    )
    .await;
    record(
        &state,
        actor,
        server,
        "update",
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
