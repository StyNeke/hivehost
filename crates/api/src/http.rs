use crate::auth::{AuthError, AuthStore};
use crate::eggs::{EggStore, EggSummary, LibraryError};
use crate::servers::{ServerError, ServerRecord, ServerStore, ServerSummary};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use manager_core::{
    eggs::CompatibilityIssue,
    model::{EggRevisionId, ServerId, UserId},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    database: PathBuf,
    origin: String,
    auth_slots: Arc<Semaphore>,
    attempts: Arc<Mutex<VecDeque<Instant>>>,
}

type ApiError = (StatusCode, Json<ErrorBody>);

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Vec<CompatibilityIssue>>,
}

fn error(status: StatusCode, code: &'static str, message: &'static str) -> ApiError {
    (
        status,
        Json(ErrorBody {
            code,
            message,
            details: None,
        }),
    )
}

fn storage_error() -> ApiError {
    error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "Request could not be completed",
    )
}
fn unauthorized() -> ApiError {
    error(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "Sign in to continue",
    )
}
fn limited() -> ApiError {
    error(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        "Too many attempts; try again later",
    )
}

fn auth_error(err: AuthError) -> ApiError {
    match err {
        AuthError::InvalidCredentials => error(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "Invalid username or password",
        ),
        AuthError::InvalidInvite => error(
            StatusCode::BAD_REQUEST,
            "invalid_invite",
            "Invalid or expired invitation",
        ),
        AuthError::InvalidInput => error(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "Username must be 3–32 letters, numbers, underscores or hyphens; password must be 12–1024 bytes",
        ),
        AuthError::Forbidden => error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Administrator access required",
        ),
        _ => storage_error(),
    }
}

impl AppState {
    async fn servers<T: Send + 'static>(
        &self,
        headers: &HeaderMap,
        action: impl FnOnce(&mut ServerStore, UserId) -> Result<T, ServerError> + Send + 'static,
    ) -> Result<T, ApiError> {
        self.authenticated(headers, move |path, actor| {
            action(&mut ServerStore::open(path).map_err(server_error)?, actor).map_err(server_error)
        })
        .await
    }

    async fn library<T: Send + 'static>(
        &self,
        headers: &HeaderMap,
        action: impl FnOnce(&mut EggStore, UserId) -> Result<T, LibraryError> + Send + 'static,
    ) -> Result<T, ApiError> {
        self.authenticated(headers, move |path, actor| {
            action(&mut EggStore::open(path).map_err(library_error)?, actor).map_err(library_error)
        })
        .await
    }

    async fn authenticated<T: Send + 'static>(
        &self,
        headers: &HeaderMap,
        action: impl FnOnce(&std::path::Path, UserId) -> Result<T, ApiError> + Send + 'static,
    ) -> Result<T, ApiError> {
        let token = session_token(headers)?;
        let now = now()?;
        let permit = self
            .auth_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| limited())?;
        let path = self.database.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let user = AuthStore::open(&path)
                .map_err(auth_error)?
                .authenticate(&token, now)
                .map_err(auth_error)?
                .ok_or_else(unauthorized)?;
            action(&path, user.id)
        })
        .await
        .map_err(|_| storage_error())?
    }
    async fn auth<T: Send + 'static>(
        &self,
        action: impl FnOnce(&mut AuthStore) -> Result<T, AuthError> + Send + 'static,
    ) -> Result<T, ApiError> {
        let permit = self
            .auth_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| limited())?;
        let path = self.database.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            action(&mut AuthStore::open(&path)?)
        })
        .await
        .map_err(|_| storage_error())?
        .map_err(auth_error)
    }

    fn allow_attempt(&self) -> Result<(), ApiError> {
        let mut attempts = self.attempts.lock().map_err(|_| storage_error())?;
        let now = Instant::now();
        while attempts
            .front()
            .is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(60))
        {
            attempts.pop_front();
        }
        if attempts.len() >= 10 {
            return Err(limited());
        }
        attempts.push_back(now);
        Ok(())
    }
}

pub fn router(database: PathBuf, origin: String) -> Router {
    let state = AppState {
        database,
        origin,
        auth_slots: Arc::new(Semaphore::new(4)),
        attempts: Arc::new(Mutex::new(VecDeque::new())),
    };
    Router::new()
        .route("/api/login", post(login))
        .route("/api/register", post(register))
        .route("/api/logout", post(logout))
        .route("/api/invites", post(invite))
        .route("/api/session", get(session))
        .route("/api/servers", get(list_servers).post(create_server))
        .route("/api/servers/{id}", get(get_server))
        .route(
            "/api/servers/{id}/members/{user_id}",
            axum::routing::put(set_member),
        )
        .route(
            "/api/eggs",
            get(list_eggs)
                .post(import_egg)
                .layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        .route(
            "/api/eggs/{id}/revisions",
            post(revise_egg).layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        .route("/api/eggs/revisions/{id}", get(get_revision))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if request.method() != Method::GET
        && request.method() != Method::HEAD
        && request
            .headers()
            .get(header::ORIGIN)
            .and_then(|h| h.to_str().ok())
            != Some(state.origin.as_str())
    {
        return error(
            StatusCode::FORBIDDEN,
            "origin_rejected",
            "Request origin is not allowed",
        )
        .into_response();
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

async fn login(
    State(state): State<AppState>,
    Json(input): Json<Credentials>,
) -> Result<Response, ApiError> {
    state.allow_attempt()?;
    let now = now()?;
    let token = state
        .auth(move |auth| auth.login(&input.username, &input.password, now))
        .await?;
    let mut response = StatusCode::OK.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "__Host-gm_session={token}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=604800"
        ))
        .map_err(|_| storage_error())?,
    );
    Ok(response)
}

#[derive(Deserialize)]
struct Registration {
    invite: String,
    username: String,
    password: String,
}

async fn register(
    State(state): State<AppState>,
    Json(input): Json<Registration>,
) -> Result<StatusCode, ApiError> {
    state.allow_attempt()?;
    let now = now()?;
    state
        .auth(move |auth| auth.register(&input.invite, &input.username, &input.password, now))
        .await?;
    Ok(StatusCode::CREATED)
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    let token = session_token(&headers)?;
    state.auth(move |auth| auth.logout(&token)).await?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "__Host-gm_session=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0",
        ),
    );
    Ok(response)
}

#[derive(Serialize)]
struct Session {
    id: String,
    is_admin: bool,
}

async fn session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Session>, ApiError> {
    let token = session_token(&headers)?;
    let now = now()?;
    let user = state
        .auth(move |auth| auth.authenticate(&token, now))
        .await?
        .ok_or_else(unauthorized)?;
    Ok(Json(Session {
        id: user.id.0.to_string(),
        is_admin: user.is_admin,
    }))
}

#[derive(Serialize)]
struct Invitation {
    token: String,
    expires_in_seconds: u32,
}

async fn invite(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Invitation>, ApiError> {
    let token = session_token(&headers)?;
    let now = now()?;
    let result = state
        .auth(move |auth| {
            let user = auth
                .authenticate(&token, now)?
                .ok_or(AuthError::InvalidCredentials)?;
            auth.invite(&user, now)
        })
        .await?;
    Ok(Json(Invitation {
        token: result,
        expires_in_seconds: 86400,
    }))
}

fn session_token(headers: &HeaderMap) -> Result<String, ApiError> {
    let tokens: Vec<_> = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(';'))
        .filter_map(|part| part.trim().strip_prefix("__Host-gm_session="))
        .collect();
    match tokens.as_slice() {
        [token] if token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()) => {
            Ok(token.to_string())
        }
        _ => Err(unauthorized()),
    }
}

fn now() -> Result<i64, ApiError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .ok_or_else(storage_error)
}

fn library_error(err: LibraryError) -> ApiError {
    match err {
        LibraryError::Unsupported(issues) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(ErrorBody {
                code: "unsupported_egg",
                message: "Egg could not be imported; review the compatibility details",
                details: Some(issues),
            }),
        ),
        LibraryError::Forbidden => error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Only the importer or administrator may change this egg",
        ),
        LibraryError::NotFound => {
            error(StatusCode::NOT_FOUND, "not_found", "Egg revision not found")
        }
        _ => storage_error(),
    }
}

async fn list_eggs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<EggSummary>>, ApiError> {
    state
        .library(&headers, |store, _| store.list())
        .await
        .map(Json)
}

async fn import_egg(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<EggSummary>), ApiError> {
    let imported = state
        .library(&headers, move |store, actor| store.import(actor, &body))
        .await?;
    Ok((StatusCode::CREATED, Json(imported)))
}

async fn revise_egg(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<EggSummary>), ApiError> {
    let revised = state
        .library(&headers, move |store, actor| store.revise(actor, id, &body))
        .await?;
    Ok((StatusCode::CREATED, Json(revised)))
}

async fn get_revision(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<manager_core::eggs::EggRevision>, ApiError> {
    state
        .library(&headers, move |store, _| {
            store.revision(EggRevisionId(id)).map(|r| r.normalized)
        })
        .await
        .map(Json)
}

fn server_error(err: ServerError) -> ApiError {
    match err {
        ServerError::NotFound => error(StatusCode::NOT_FOUND, "not_found", "Server not found"),
        ServerError::Forbidden => error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "You do not have access to this server or action",
        ),
        _ => storage_error(),
    }
}

async fn list_servers(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ServerSummary>>, ApiError> {
    state
        .servers(&headers, |store, actor| store.list(actor))
        .await
        .map(Json)
}

async fn get_server(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ServerRecord>, ApiError> {
    state
        .servers(&headers, move |store, actor| {
            store.require_access(actor, ServerId(id))
        })
        .await
        .map(Json)
}

#[derive(Deserialize)]
struct MemberAccess {
    enabled: bool,
}

async fn set_member(
    State(state): State<AppState>,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(input): Json<MemberAccess>,
) -> Result<StatusCode, ApiError> {
    state
        .servers(&headers, move |store, actor| {
            store.set_member(actor, ServerId(id), UserId(user_id), input.enabled)
        })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn create_server(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    // shortcut: fail closed until trusted agent capacity, quotas and runnable egg revisions are connected.
    state
        .authenticated(&headers, |_, _| {
            Err(error(
                StatusCode::SERVICE_UNAVAILABLE,
                "host_not_ready",
                "Finish host resource limits and storage setup before creating servers",
            ))
        })
        .await
}
