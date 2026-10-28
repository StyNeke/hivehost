use crate::agent_client::AgentClient;
use crate::auth::{AuthError, AuthStore};
use crate::eggs::{EggStore, EggSummary, LibraryError};
use crate::provisioning::{
    Allocation, CreateServer, CreatedServer, PortBinding, ProvisioningError, ProvisioningStore,
};
use crate::servers::{ServerError, ServerStore, ServerSummary};
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
    capacity::{CapacityError, Resources},
    eggs::CompatibilityIssue,
    host::{HostHardware, HostStatus},
    model::{EggRevisionId, ServerId, UserId},
};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
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
    agent: Option<AgentClient>,
    modrinth: crate::modrinth::Modrinth,
    backup_root: PathBuf,
}

#[path = "http_server_routes.rs"]
mod server_routes;

type ApiError = (StatusCode, Json<ErrorBody>);

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: Cow<'static, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Vec<CompatibilityIssue>>,
}

fn error(status: StatusCode, code: &'static str, message: &'static str) -> ApiError {
    (
        status,
        Json(ErrorBody {
            code,
            message: Cow::Borrowed(message),
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
    // Host state is fetched per request and never cached, so admission always sees a fresh snapshot.
    async fn host_status(&self) -> Result<HostStatus, ApiError> {
        let agent = self.agent.as_ref().ok_or_else(|| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "host_unavailable",
                "The host agent is not configured",
            )
        })?;
        agent.inspect_host().await.map_err(|_| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "host_unavailable",
                "The host agent did not return a trustworthy status",
            )
        })
    }

    async fn provisioning<T: Send + 'static>(
        &self,
        headers: &HeaderMap,
        action: impl FnOnce(
            &mut ProvisioningStore,
            UserId,
            &HostStatus,
            i64,
        ) -> Result<T, ProvisioningError>
        + Send
        + 'static,
    ) -> Result<T, ApiError> {
        // Authenticate before asking the agent so anonymous requests cannot trigger host inspection.
        self.authenticated(headers, |_, _| Ok(())).await?;
        let status = self.host_status().await?;
        self.authenticated(headers, move |path, actor| {
            let now = now()?;
            action(
                &mut ProvisioningStore::open(path).map_err(provisioning_error)?,
                actor,
                &status,
                now,
            )
            .map_err(provisioning_error)
        })
        .await
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

// Without an agent, host-dependent routes fail closed with host_unavailable/host_not_ready.
pub const MODRINTH_API: &str = "https://api.modrinth.com/v2";

pub const BACKUP_ROOT: &str = "/var/lib/game-manager-backups";

pub fn router(database: PathBuf, origin: String) -> Router {
    router_with_services(database, origin, None, MODRINTH_API, BACKUP_ROOT.as_ref())
}

pub fn router_with_agent(database: PathBuf, origin: String, agent: AgentClient) -> Router {
    router_with_services(
        database,
        origin,
        Some(agent),
        MODRINTH_API,
        BACKUP_ROOT.as_ref(),
    )
}

// Serves the built browser interface from `directory` for every non-API path.
pub fn with_static_files(router: Router, directory: &std::path::Path) -> Router {
    use tower::ServiceExt;
    use tower_http::services::{ServeDir, ServeFile};
    let files = ServeDir::new(directory)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(directory.join("index.html")));
    router
        .fallback(move |request: Request| {
            let files = files.clone();
            async move {
                // Unknown API paths stay JSON 404s instead of looking like a successful page load.
                if request.uri().path().starts_with("/api/") {
                    return error(StatusCode::NOT_FOUND, "not_found", "Not found").into_response();
                }
                match files.oneshot(request).await {
                    Ok(response) => response.into_response(),
                    Err(_) => storage_error().into_response(),
                }
            }
        })
        .layer(middleware::from_fn(page_headers))
}

// The interface uses no inline scripts, frames or third-party origins, so the policy can be strict.
async fn page_headers(request: Request, next: Next) -> Response {
    let api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if !api {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
            ),
        );
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
        headers.insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        );
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }
    response
}

pub fn router_with_services(
    database: PathBuf,
    origin: String,
    agent: Option<AgentClient>,
    modrinth_api: &str,
    backup_root: &std::path::Path,
) -> Router {
    let state = AppState {
        database,
        origin,
        auth_slots: Arc::new(Semaphore::new(4)),
        attempts: Arc::new(Mutex::new(VecDeque::new())),
        agent,
        modrinth: crate::modrinth::Modrinth::new(modrinth_api),
        backup_root: backup_root.to_path_buf(),
    };
    Router::new()
        .route("/api/login", post(login))
        .route("/api/register", post(register))
        .route("/api/logout", post(logout))
        .route("/api/invites", post(invite))
        .route("/api/session", get(session))
        .route("/api/servers", get(list_servers).post(create_server))
        .route(
            "/api/servers/{id}",
            get(server_routes::detail).delete(server_routes::delete),
        )
        .route("/api/servers/{id}/power", post(server_routes::power))
        .route(
            "/api/servers/{id}/console",
            get(server_routes::console).post(server_routes::command),
        )
        .route(
            "/api/servers/{id}/reinstall",
            post(server_routes::reinstall),
        )
        .route("/api/servers/{id}/activity", get(server_routes::activity))
        .route(
            "/api/servers/{id}/extensions",
            get(server_routes::extensions).post(server_routes::add_extension),
        )
        .route(
            "/api/servers/{id}/extensions/{project}",
            axum::routing::delete(server_routes::remove_extension),
        )
        .route(
            "/api/servers/{id}/modrinth",
            get(server_routes::search_modrinth),
        )
        .route(
            "/api/servers/{id}/backups",
            get(server_routes::backups::list).post(server_routes::backups::create),
        )
        .route(
            "/api/servers/{id}/backups/retention",
            axum::routing::put(server_routes::backups::set_retention),
        )
        .route(
            "/api/servers/{id}/backups/{backup}",
            axum::routing::delete(server_routes::backups::remove),
        )
        .route(
            "/api/servers/{id}/backups/{backup}/restore",
            post(server_routes::backups::restore),
        )
        .route(
            "/api/servers/{id}/backups/{backup}/download",
            get(server_routes::backups::download),
        )
        .route(
            "/api/retained-backups",
            get(server_routes::backups::retained),
        )
        .route(
            "/api/eggs/revisions/{id}/qualification",
            post(server_routes::qualify_revision),
        )
        .route("/api/host", get(host_overview))
        .route(
            "/api/allocations",
            get(list_allocations).post(add_allocation),
        )
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
                message: Cow::Borrowed(
                    "Egg could not be imported; review the compatibility details",
                ),
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

fn provisioning_error(err: ProvisioningError) -> ApiError {
    match err {
        ProvisioningError::HostNotReady(message) => {
            error(StatusCode::SERVICE_UNAVAILABLE, "host_not_ready", message)
        }
        ProvisioningError::InvalidInput(message) => {
            error(StatusCode::BAD_REQUEST, "invalid_input", message)
        }
        // Egg variable rules come from the shared library, so echoing them reveals nothing private.
        ProvisioningError::Variable(message) => (
            StatusCode::BAD_REQUEST,
            Json(ErrorBody {
                code: "invalid_variable",
                message: Cow::Owned(message),
                details: None,
            }),
        ),
        ProvisioningError::Capacity(CapacityError::InvalidRequest) => error(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "CPU, RAM and disk limits must all be greater than zero",
        ),
        ProvisioningError::Capacity(CapacityError::Memory) => error(
            StatusCode::CONFLICT,
            "capacity_exceeded",
            "Not enough RAM remains in the safe host budget",
        ),
        ProvisioningError::Capacity(CapacityError::Disk) => error(
            StatusCode::CONFLICT,
            "capacity_exceeded",
            "Not enough disk remains in the safe host budget",
        ),
        ProvisioningError::Capacity(CapacityError::Cpu) => error(
            StatusCode::CONFLICT,
            "capacity_exceeded",
            "Not enough CPU remains in the safe host budget",
        ),
        ProvisioningError::Forbidden => error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Administrator access required",
        ),
        ProvisioningError::AllocationUnavailable => error(
            StatusCode::CONFLICT,
            "allocation_unavailable",
            "That IP and port are not available",
        ),
        ProvisioningError::NotFound => {
            error(StatusCode::NOT_FOUND, "not_found", "Egg revision not found")
        }
        ProvisioningError::Storage(_) => storage_error(),
    }
}

// Capacity explanation for every signed-in user; private IPs and port bindings stay admin data.
#[derive(Serialize)]
struct HostOverview {
    measured_at: i64,
    ready: bool,
    blockers: Vec<String>,
    hardware: HostHardware,
    budget: Resources,
    reserved: Resources,
    remaining: Resources,
}

async fn host_overview(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<HostOverview>, ApiError> {
    state
        .provisioning(&headers, |store, _, status, _| {
            let servers = store.reserved()?;
            let admission = &status.admission;
            let reserved = Resources {
                memory_bytes: admission
                    .legacy_reserved
                    .memory_bytes
                    .saturating_add(servers.memory_bytes),
                disk_bytes: admission
                    .legacy_reserved
                    .disk_bytes
                    .saturating_add(servers.disk_bytes),
                cpu_millis: admission
                    .legacy_reserved
                    .cpu_millis
                    .saturating_add(servers.cpu_millis),
            };
            Ok(HostOverview {
                measured_at: admission.measured_at,
                ready: admission.disk_quotas_ready && status.blockers.is_empty(),
                blockers: status.blockers.clone(),
                hardware: status.hardware.clone(),
                budget: admission.budget,
                reserved,
                remaining: Resources {
                    memory_bytes: admission
                        .budget
                        .memory_bytes
                        .saturating_sub(reserved.memory_bytes),
                    disk_bytes: admission
                        .budget
                        .disk_bytes
                        .saturating_sub(reserved.disk_bytes),
                    cpu_millis: admission
                        .budget
                        .cpu_millis
                        .saturating_sub(reserved.cpu_millis),
                },
            })
        })
        .await
        .map(Json)
}

async fn list_allocations(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Allocation>>, ApiError> {
    state
        .authenticated(&headers, |path, _| {
            ProvisioningStore::open(path)
                .and_then(|store| store.free_allocations())
                .map_err(provisioning_error)
        })
        .await
        .map(Json)
}

#[derive(Serialize)]
struct AllocationCreated {
    id: Uuid,
}

async fn add_allocation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(binding): Json<PortBinding>,
) -> Result<(StatusCode, Json<AllocationCreated>), ApiError> {
    let id = state
        .provisioning(&headers, move |store, actor, status, now| {
            store.add_allocation(actor, binding, &status.admission, now)
        })
        .await?;
    Ok((StatusCode::CREATED, Json(AllocationCreated { id })))
}

async fn create_server(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateServer>,
) -> Result<(StatusCode, Json<CreatedServer>), ApiError> {
    let created = state
        .provisioning(&headers, move |store, actor, status, now| {
            store.create(actor, request, &status.admission, now)
        })
        .await?;
    Ok((StatusCode::CREATED, Json(created)))
}
