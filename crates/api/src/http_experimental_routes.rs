// Experimental implementations: sourced upstream status and adding their bundled eggs to the
// library. Reading never waits for GitHub; a due refresh runs in the background.
use super::{ApiError, AppState, error, library_error, now, storage_error};
use crate::{
    eggs::{EggStore, EggSummary},
    experimental::{Entry, StatusStore, project},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};

async fn entries(state: &AppState, headers: &HeaderMap) -> Result<Vec<Entry>, ApiError> {
    state
        .authenticated(headers, |path, _| {
            let at = now()?;
            StatusStore::open(path)
                .and_then(|store| store.entries(at))
                .map_err(|_| storage_error())
        })
        .await
}

pub(super) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Entry>>, ApiError> {
    let listed = entries(&state, &headers).await?;
    let path = state.database.clone();
    let at = now()?;
    let due = tokio::task::spawn_blocking(move || {
        StatusStore::open(&path).and_then(|mut store| store.claim_attempt(at, true))
    })
    .await
    .map_err(|_| storage_error())?
    .map_err(|_| storage_error())?;
    if due {
        let github = state.github.clone();
        let database = state.database.clone();
        tokio::spawn(async move {
            let _ = github.refresh(&database, at).await;
        });
    }
    Ok(Json(listed))
}

pub(super) async fn refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Entry>>, ApiError> {
    entries(&state, &headers).await?;
    let path = state.database.clone();
    let at = now()?;
    let claimed = tokio::task::spawn_blocking(move || {
        StatusStore::open(&path).and_then(|mut store| store.claim_attempt(at, false))
    })
    .await
    .map_err(|_| storage_error())?
    .map_err(|_| storage_error())?;
    if !claimed {
        return Err(error(
            StatusCode::TOO_MANY_REQUESTS,
            "refresh_limited",
            "Upstream status was refreshed less than a minute ago",
        ));
    }
    state
        .github
        .refresh(&state.database, at)
        .await
        .map_err(|_| storage_error())?;
    entries(&state, &headers).await.map(Json)
}

pub(super) async fn add_egg(
    State(state): State<AppState>,
    Path(key): Path<String>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<EggSummary>), ApiError> {
    let project = project(&key)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "not_found", "Unknown project"))?;
    state
        .authenticated(&headers, move |path, actor| {
            let mut status = StatusStore::open(path).map_err(|_| storage_error())?;
            if status
                .entries(0)
                .map_err(|_| storage_error())?
                .iter()
                .any(|e| e.key == project.key && e.egg_id.is_some())
            {
                return Err(error(
                    StatusCode::CONFLICT,
                    "already_added",
                    "This experimental egg is already in the library",
                ));
            }
            let summary = EggStore::open(path)
                .and_then(|mut store| store.import(actor, project.egg))
                .map_err(library_error)?;
            status
                .mark_egg(&summary.id.to_string(), project.key)
                .map_err(|_| storage_error())?;
            Ok((
                StatusCode::CREATED,
                Json(EggSummary {
                    experimental: true,
                    ..summary
                }),
            ))
        })
        .await
}
