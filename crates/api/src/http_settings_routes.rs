// Startup settings, resource limits and members. Changes are stored and take effect at the next
// start; a running server keeps the settings it was started with.
use super::{managed, record, runtime_error, with_access};
use crate::{
    http::{ApiError, AppState, server_error},
    runtime::{RuntimeStore, Settings},
    servers::{Member, Person, ServerStore},
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use manager_core::{capacity::Resources, model::ServerId};
use serde::{Deserialize, Serialize};
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
