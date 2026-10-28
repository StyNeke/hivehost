#![cfg(target_os = "linux")]
// HTTP lifecycle through the API's own routes and worker, against a stateful fake agent on a real
// Unix socket and a fake Modrinth API. Real Docker behaviour is covered by the agent's tests.

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Path as UrlPath, Query},
    http::{Request, StatusCode},
    routing::get,
};
use manager_api::{
    agent_client::AgentClient, auth::AuthStore, eggs::EggStore, http::router_with_services,
    schedules, worker,
};
use manager_core::{
    capacity::Resources,
    host::{HostAdmission, HostHardware, HostStatus},
};
use manager_protocol::{
    AgentRequest, AgentResponse, AgentResult, BackupInfo, BackupKind, DetectedGame, Operation,
    OperationKind, OperationState, OperationStatus, PowerState, Rejection, RuntimeStatus,
    ServerUsage, UsageSummary, VERSION,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    os::unix::fs::MetadataExt,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
};
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "https://panel.example.test";
const GIB: u64 = 1024 * 1024 * 1024;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[derive(Default, Clone)]
struct FakeServer {
    installed: bool,
    fail_install: bool,
    running: bool,
    operation: Option<OperationStatus>,
    files: Vec<String>,
    commands: Vec<String>,
}

#[derive(Default)]
struct FakeAgent {
    servers: HashMap<Uuid, FakeServer>,
    fail_next_install: bool,
    backup_root: std::path::PathBuf,
    // File operations run against the agent's real implementation on this directory.
    volume_root: std::path::PathBuf,
    backups: HashMap<Uuid, Vec<BackupInfo>>,
    retention: HashMap<Uuid, u8>,
    last_start: Option<manager_protocol::ServerSpec>,
    updates: Vec<(Uuid, bool)>,
    imports: HashMap<Uuid, Vec<u8>>,
}

const BACKUP_BYTES: usize = 20 * 1024 * 1024;

fn op(kind: OperationKind, state: OperationState, message: &str) -> Option<OperationStatus> {
    Some(OperationStatus {
        kind,
        state,
        message: message.into(),
        updated_at: now(),
    })
}

fn rejection(code: &str, message: &str) -> AgentResult {
    AgentResult::Rejected(Rejection {
        code: code.into(),
        message: message.into(),
    })
}

impl FakeAgent {
    fn files(&mut self, operation: Operation) -> AgentResult {
        use base64::Engine;
        use manager_agent::file_manager::{Volume, validate};
        let engine = base64::engine::general_purpose::STANDARD;
        std::fs::create_dir_all(&self.volume_root).unwrap();
        let volume = Volume::open(&self.volume_root).unwrap();
        let reject = |error: std::io::Error| {
            let code = match error.kind() {
                std::io::ErrorKind::NotFound => "not_found",
                std::io::ErrorKind::AlreadyExists => "conflict",
                std::io::ErrorKind::InvalidInput => "invalid_path",
                _ => "files",
            };
            rejection(code, &error.to_string())
        };
        let result = match operation {
            Operation::ListDirectory { path, .. } => validate(&path, true)
                .and_then(|p| volume.list(&p))
                .map(AgentResult::Directory),
            Operation::ReadText { path, .. } => validate(&path, false)
                .and_then(|p| volume.read_text(&p))
                .map(|(text, version)| AgentResult::Text { text, version }),
            Operation::WriteText {
                path,
                text,
                expected_version,
                ..
            } => validate(&path, false)
                .and_then(|p| volume.write_text(&p, &text, expected_version.as_deref()))
                .map(|version| AgentResult::Saved { version }),
            Operation::ReadChunk {
                path,
                offset,
                length,
                ..
            } => validate(&path, false)
                .and_then(|p| volume.read_chunk(&p, offset, length as usize))
                .map(|(bytes, size)| AgentResult::Chunk {
                    data: engine.encode(bytes),
                    size,
                }),
            Operation::WriteChunk {
                upload_id,
                path,
                offset,
                data,
                last,
                ..
            } => validate(&path, false)
                .and_then(|p| {
                    volume.write_chunk(upload_id, &p, offset, &engine.decode(data).unwrap(), last)
                })
                .map(|_| AgentResult::Accepted),
            Operation::CreateDirectory { path, .. } => validate(&path, false)
                .and_then(|p| volume.create_directory(&p))
                .map(|_| AgentResult::Accepted),
            Operation::RenamePath { from, to, .. } => validate(&from, false)
                .and_then(|from| validate(&to, false).and_then(|to| volume.rename(&from, &to)))
                .map(|_| AgentResult::Accepted),
            Operation::DeletePath { path, .. } => validate(&path, false)
                .and_then(|p| volume.delete(&p))
                .map(|_| AgentResult::Accepted),
            _ => unreachable!(),
        };
        result.unwrap_or_else(reject)
    }

    // Operations complete instantly; each status read after an accepted change reports success.
    fn handle(&mut self, operation: Operation) -> AgentResult {
        match operation {
            Operation::InspectHost => AgentResult::HostStatus(Box::new(HostStatus {
                hardware: HostHardware {
                    cpu_millis: 4000,
                    memory_bytes: 16 * GIB,
                    disk_total_bytes: 100 * GIB,
                    disk_available_bytes: 60 * GIB,
                },
                admission: HostAdmission {
                    measured_at: now(),
                    budget: Resources {
                        memory_bytes: 12 * GIB,
                        disk_bytes: 40 * GIB,
                        cpu_millis: 3000,
                    },
                    legacy_reserved: Resources {
                        memory_bytes: 0,
                        disk_bytes: 0,
                        cpu_millis: 0,
                    },
                    disk_quotas_ready: true,
                    qualified_revisions: vec![],
                    private_ips: vec!["127.0.0.1".parse().unwrap()],
                    unavailable_bindings: vec![],
                },
                blockers: vec![],
            })),
            Operation::InstallServer { spec } => {
                let fail = std::mem::take(&mut self.fail_next_install);
                let server = self.servers.entry(spec.server_id).or_default();
                if server.installed {
                    return rejection("already_installed", "server is already installed");
                }
                server.fail_install = fail;
                server.operation = op(
                    OperationKind::Install,
                    OperationState::Running,
                    "Installing",
                );
                AgentResult::Accepted
            }
            Operation::StartServer { spec } => match self.servers.get_mut(&spec.server_id) {
                Some(server) if server.installed => {
                    server.running = true;
                    self.last_start = Some(*spec);
                    AgentResult::Accepted
                }
                _ => rejection("not_installed", "install the server before starting it"),
            },
            Operation::StopServer { server_id } | Operation::KillServer { server_id } => {
                if let Some(server) = self.servers.get_mut(&server_id) {
                    server.running = false;
                }
                AgentResult::Accepted
            }
            Operation::SendCommand { server_id, command } => match self.servers.get_mut(&server_id)
            {
                Some(server) if server.running => {
                    server.commands.push(command);
                    AgentResult::Accepted
                }
                _ => rejection("not_running", "the server is not running"),
            },
            Operation::ReadConsole { server_id, .. } => match self.servers.get(&server_id) {
                Some(server) => {
                    AgentResult::Console(server.commands.iter().map(|c| format!("> {c}")).collect())
                }
                None => rejection("not_found", "server not found"),
            },
            Operation::ServerStatus { server_id } => {
                let Some(server) = self.servers.get_mut(&server_id) else {
                    return rejection("not_found", "server not found");
                };
                if let Some(current) = server
                    .operation
                    .as_mut()
                    .filter(|o| o.state == OperationState::Running)
                {
                    match current.kind {
                        OperationKind::Install if server.fail_install => {
                            current.state = OperationState::Failed;
                            current.message = "installer exited with code 1".into();
                        }
                        OperationKind::Install => {
                            server.installed = true;
                            current.state = OperationState::Succeeded;
                            current.message = "Installed Minecraft 26.3 with fabric".into();
                        }
                        OperationKind::Extensions | OperationKind::Update => {
                            current.state = OperationState::Succeeded;
                            current.message = "Installed".into();
                        }
                        OperationKind::Delete => {
                            self.servers.remove(&server_id);
                            return rejection("not_found", "server not found");
                        }
                        OperationKind::Backup => {
                            let id = Uuid::new_v4();
                            let directory = self.backup_root.join(server_id.to_string());
                            std::fs::create_dir_all(&directory).unwrap();
                            std::fs::write(
                                directory.join(format!("{id}.tar.gz")),
                                vec![9u8; BACKUP_BYTES],
                            )
                            .unwrap();
                            let list = self.backups.entry(server_id).or_default();
                            list.insert(
                                0,
                                BackupInfo {
                                    id,
                                    kind: BackupKind::Manual,
                                    created_at: now(),
                                    size_bytes: BACKUP_BYTES as u64,
                                    sha256: "0".repeat(64),
                                },
                            );
                            list.truncate(self.retention[&server_id] as usize);
                            current.state = OperationState::Succeeded;
                            current.message = "Backup created".into();
                        }
                        OperationKind::Restore => {
                            current.state = OperationState::Succeeded;
                            current.message = "Backup restored".into();
                        }
                    }
                }
                AgentResult::ServerStatus(Box::new(RuntimeStatus {
                    installed: server.installed,
                    operation: server.operation.clone(),
                    power: if server.running {
                        PowerState::Running
                    } else {
                        PowerState::Offline
                    },
                    desired_running: server.running,
                    problem: None,
                    started_at: server.running.then(now),
                    exit_code: None,
                    game: server.installed.then(|| DetectedGame {
                        version: "26.3".into(),
                        loader: Some("fabric".into()),
                    }),
                    disk_used_bytes: Some(1024),
                    disk_total_bytes: Some(4 * GIB),
                    usage: None,
                }))
            }
            Operation::InstallFiles {
                server_id,
                files,
                remove,
                ..
            } => match self.servers.get_mut(&server_id) {
                Some(server) if server.running => {
                    rejection("running", "stop the server before changing mods or plugins")
                }
                Some(server) => {
                    server.files.retain(|f| !remove.contains(f));
                    server.files.extend(files.into_iter().map(|f| f.filename));
                    server.operation = op(
                        OperationKind::Extensions,
                        OperationState::Running,
                        "Downloading",
                    );
                    AgentResult::Accepted
                }
                None => rejection("not_found", "server not found"),
            },
            Operation::ListFiles { server_id, .. } => AgentResult::Files(
                self.servers
                    .get(&server_id)
                    .map(|s| s.files.clone())
                    .unwrap_or_default(),
            ),
            operation @ (Operation::ListDirectory { .. }
            | Operation::ReadText { .. }
            | Operation::WriteText { .. }
            | Operation::ReadChunk { .. }
            | Operation::WriteChunk { .. }
            | Operation::CreateDirectory { .. }
            | Operation::RenamePath { .. }
            | Operation::DeletePath { .. }) => self.files(operation),
            Operation::CreateBackup {
                server_id,
                retention,
            } => {
                let Some(server) = self.servers.get_mut(&server_id) else {
                    return rejection("not_found", "server not found");
                };
                self.retention.insert(server_id, retention);
                server.operation = op(OperationKind::Backup, OperationState::Running, "Writing");
                AgentResult::Accepted
            }
            Operation::ListBackups { server_id } => {
                AgentResult::Backups(self.backups.get(&server_id).cloned().unwrap_or_default())
            }
            Operation::RestoreBackup {
                server_id,
                backup_id,
            } => match self.servers.get_mut(&server_id) {
                Some(server) if server.running => {
                    rejection("running", "stop the server before restoring a backup")
                }
                Some(server)
                    if self
                        .backups
                        .get(&server_id)
                        .is_some_and(|b| b.iter().any(|b| b.id == backup_id)) =>
                {
                    server.operation =
                        op(OperationKind::Restore, OperationState::Running, "Restoring");
                    AgentResult::Accepted
                }
                _ => rejection("not_found", "backup not found"),
            },
            Operation::DeleteBackup {
                server_id,
                backup_id,
            } => {
                let list = self.backups.entry(server_id).or_default();
                let before = list.len();
                list.retain(|b| b.id != backup_id);
                if list.len() == before {
                    rejection("not_found", "backup not found")
                } else {
                    AgentResult::Accepted
                }
            }
            Operation::DeleteServer {
                server_id,
                keep_backups,
            } => {
                if !keep_backups {
                    self.backups.remove(&server_id);
                }
                if let Some(server) = self.servers.get_mut(&server_id) {
                    server.operation =
                        op(OperationKind::Delete, OperationState::Running, "Deleting");
                }
                AgentResult::Accepted
            }
            Operation::UpdateServer {
                spec,
                keep_game_version,
            } => match self.servers.get_mut(&spec.server_id) {
                Some(server) if server.installed => {
                    self.updates.push((spec.server_id, keep_game_version));
                    server.operation =
                        op(OperationKind::Update, OperationState::Running, "Updating");
                    AgentResult::Accepted
                }
                _ => rejection("not_installed", "install the server first"),
            },
            Operation::ImportChunk {
                server_id,
                upload_id,
                offset,
                data,
                last,
            } => {
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap();
                let buffer = self.imports.entry(upload_id).or_default();
                if buffer.len() as u64 != offset {
                    return rejection("invalid", "upload chunks arrived out of order");
                }
                buffer.extend_from_slice(&bytes);
                if !last {
                    return AgentResult::Accepted;
                }
                let archive = self.imports.remove(&upload_id).unwrap();
                // The real verifier decides, as on the host.
                if let Err(error) = manager_agent::backups::verify_archive(&archive[..]) {
                    return rejection(
                        "invalid",
                        &format!("not a usable .tar.gz archive of server files: {error}"),
                    );
                }
                let info = BackupInfo {
                    id: upload_id,
                    kind: BackupKind::Imported,
                    created_at: now(),
                    size_bytes: archive.len() as u64,
                    sha256: "0".repeat(64),
                };
                self.backups
                    .entry(server_id)
                    .or_default()
                    .insert(0, info.clone());
                AgentResult::Backups(vec![info])
            }
            Operation::UsageAll => AgentResult::Usage(
                self.servers
                    .iter()
                    .map(|(id, server)| {
                        let usage = server.running.then_some(ServerUsage {
                            cpu_millis: 1500,
                            memory_bytes: 2 * GIB,
                            memory_limit_bytes: 3 * GIB,
                            network_rx_bytes_per_sec: 10,
                            network_tx_bytes_per_sec: 20,
                            measured_at: now(),
                        });
                        (
                            *id,
                            UsageSummary {
                                usage,
                                disk_used_bytes: Some(1024),
                            },
                        )
                    })
                    .collect(),
            ),
            Operation::Ping => AgentResult::Pong,
        }
    }
}

fn spawn_agent(path: &std::path::Path, agent: Arc<Mutex<FakeAgent>>) {
    let listener = UnixListener::bind(path).unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let agent = agent.clone();
            tokio::spawn(async move {
                let length = stream.read_u32().await.unwrap();
                let mut bytes = vec![0; length as usize];
                stream.read_exact(&mut bytes).await.unwrap();
                let request: AgentRequest = serde_json::from_slice(&bytes).unwrap();
                let result = agent.lock().unwrap().handle(request.operation);
                let bytes = serde_json::to_vec(&AgentResponse {
                    version: VERSION,
                    request_id: request.request_id,
                    result,
                })
                .unwrap();
                stream.write_u32(bytes.len() as u32).await.unwrap();
                stream.write_all(&bytes).await.unwrap();
            });
        }
    });
}

fn file(project: &str) -> Value {
    json!({
        "url": format!("https://cdn.modrinth.com/data/{project}/versions/v1/{project}.jar"),
        "filename": format!("{project}-1.0.jar"),
        "primary": true,
        "size": 1000,
        "hashes": {"sha512": "ab".repeat(64), "sha1": "cd".repeat(20)}
    })
}

// (server_side, has a fabric 26.3 release, dependencies)
type FakeProject = (&'static str, bool, Vec<(&'static str, &'static str)>);

async fn spawn_modrinth() -> String {
    // project id -> (server_side, has 26.3 fabric release, dependencies)
    let projects: HashMap<&str, FakeProject> = HashMap::from([
        ("carpet", ("optional", true, vec![])),
        (
            "needsapi",
            (
                "required",
                true,
                vec![("fabricapi", "required"), ("optionalthing", "optional")],
            ),
        ),
        ("fabricapi", ("optional", true, vec![])),
        ("clientonly", ("unsupported", true, vec![])),
        ("oldonly", ("required", false, vec![])),
        (
            "conflicts",
            ("required", true, vec![("carpet", "incompatible")]),
        ),
    ]);
    let projects = Arc::new(projects);
    let versions = projects.clone();
    let app = Router::new()
        .route(
            "/project/{id}",
            get(move |UrlPath(id): UrlPath<String>| {
                let projects = projects.clone();
                async move {
                    match projects.get(id.as_str()) {
                        Some((side, _, _)) => axum::Json(json!({"id": id, "title": format!("Title {id}"), "project_type": "mod", "server_side": side})).into_response_ok(),
                        None => (StatusCode::NOT_FOUND, "missing").into_response_err(),
                    }
                }
            }),
        )
        .route(
            "/project/{id}/version",
            get(move |UrlPath(id): UrlPath<String>, Query(query): Query<HashMap<String, String>>| {
                let versions = versions.clone();
                async move {
                    assert_eq!(query.get("loaders").map(String::as_str), Some(r#"["fabric"]"#));
                    assert_eq!(query.get("game_versions").map(String::as_str), Some(r#"["26.3"]"#));
                    let Some((_, compatible, dependencies)) = versions.get(id.as_str()) else {
                        return (StatusCode::NOT_FOUND, "missing").into_response_err();
                    };
                    if !compatible {
                        return axum::Json(json!([])).into_response_ok();
                    }
                    let dependencies: Vec<Value> = dependencies
                        .iter()
                        .map(|(project, kind)| json!({"project_id": project, "version_id": null, "dependency_type": kind}))
                        .collect();
                    axum::Json(json!([
                        {"id": format!("{id}alpha"), "project_id": id, "version_number": "2.0-alpha", "version_type": "alpha", "files": [file(&id)], "dependencies": []},
                        {"id": format!("{id}v1"), "project_id": id, "version_number": "1.0", "version_type": "release", "files": [file(&id)], "dependencies": dependencies}
                    ]))
                    .into_response_ok()
                }
            }),
        )
        .route(
            "/search",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                assert!(query["facets"].contains("categories:fabric") && query["facets"].contains("versions:26.3"));
                axum::Json(json!({"hits": [{"project_id": "carpet", "slug": "carpet", "title": "Carpet", "description": "Tools", "icon_url": null, "downloads": 5}]}))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}")
}

trait Reply {
    fn into_response_ok(self) -> axum::response::Response;
    fn into_response_err(self) -> axum::response::Response;
}
impl<T: axum::response::IntoResponse> Reply for T {
    fn into_response_ok(self) -> axum::response::Response {
        self.into_response()
    }
    fn into_response_err(self) -> axum::response::Response {
        self.into_response()
    }
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("cookie", cookie)
        .header("origin", ORIGIN);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let body = body.map_or_else(Body::empty, |value| Body::from(value.to_string()));
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn eventually(
    app: &Router,
    uri: &str,
    cookie: &str,
    check: impl Fn(&Value) -> bool,
) -> Value {
    for _ in 0..100 {
        let (_, body) = send(app, "GET", uri, cookie, None).await;
        if check(&body) {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "condition not reached for {uri}: {}",
        send(app, "GET", uri, cookie, None).await.1
    );
}

#[tokio::test]
async fn servers_install_run_take_mods_and_delete_through_the_api() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("app.sqlite");
    let mut auth = AuthStore::open(&database).unwrap();
    let admin = auth
        .bootstrap_admin("admin", "a long test password")
        .unwrap();
    let mut cookies = vec![];
    let mut member_id = None;
    for name in ["owner", "outsider", "member"] {
        let invite = auth.invite(&admin, now()).unwrap();
        let user = auth
            .register(&invite, name, "a long test password", now())
            .unwrap();
        if name == "member" {
            member_id = Some(user.id);
        }
    }
    for name in ["admin", "owner", "outsider", "member"] {
        cookies.push(format!(
            "__Host-gm_session={}",
            auth.login(name, "a long test password", now()).unwrap()
        ));
    }
    let (admin_cookie, owner, outsider, member) =
        (&cookies[0], &cookies[1], &cookies[2], &cookies[3]);
    let member_id = member_id.unwrap();
    let revision = EggStore::open(&database)
        .unwrap()
        .import(
            admin.id,
            include_bytes!("../../../tests/fixtures/eggs/fabric.json"),
        )
        .unwrap()
        .revision;
    let backup_root = dir.path().join("backups");
    let fake = Arc::new(Mutex::new(FakeAgent {
        backup_root: backup_root.clone(),
        volume_root: dir.path().join("volume"),
        ..Default::default()
    }));
    let socket = dir.path().join("agent.sock");
    spawn_agent(&socket, fake.clone());
    let agent = AgentClient::new(&socket, std::fs::metadata("/proc/self").unwrap().uid());
    let app = router_with_services(
        database.clone(),
        ORIGIN.into(),
        Some(agent.clone()),
        &spawn_modrinth().await,
        "http://127.0.0.1:9",
        &backup_root,
    );
    let slots = Arc::new(tokio::sync::Semaphore::new(2));
    let tick = || worker::tick(&database, &agent, "test-worker", &slots);

    let mut allocations = vec![];
    for port in [25600, 25601] {
        let (code, body) = send(
            &app,
            "POST",
            "/api/allocations",
            admin_cookie,
            Some(json!({"ip": "127.0.0.1", "port": port, "transport": "tcp"})),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED);
        allocations.push(body["id"].clone());
    }
    let create = |name: &str, allocation: &Value| {
        json!({
            "name": name, "egg_revision": revision, "image": "ghcr.io/ptero-eggs/yolks:java_25",
            "allocation_id": allocation, "variables": {}, "license_accepted": true,
            "limits": {"memory_bytes": 3 * GIB, "disk_bytes": 4 * GIB, "cpu_millis": 1000}
        })
    };

    // Unqualified revisions are refused for teammates until an administrator qualifies them.
    let (code, body) = send(
        &app,
        "POST",
        "/api/servers",
        owner,
        Some(create("Bots", &allocations[0])),
    )
    .await;
    assert_eq!(
        (code, body["code"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("host_not_ready"))
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("/api/eggs/revisions/{}/qualification", revision.0),
            owner,
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("/api/eggs/revisions/{}/qualification", revision.0),
            admin_cookie,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(&app, "GET", "/api/eggs", owner, None).await.1[0]["runtime_qualified"],
        true
    );
    let (code, created) = send(
        &app,
        "POST",
        "/api/servers",
        owner,
        Some(create("Bots", &allocations[0])),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED);
    let id = created["id"].as_str().unwrap().to_owned();
    let server = format!("/api/servers/{id}");

    // Nothing can be controlled before the install job has run.
    let detail = send(&app, "GET", &server, owner, None).await.1;
    assert_eq!(detail["details"]["install_state"], "pending");
    assert!(detail["is_owner"].as_bool().unwrap());
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{server}/power"),
            owner,
            Some(json!({"action": "start"}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    tick().await.unwrap();
    let detail = eventually(&app, &server, owner, |d| {
        d["details"]["install_state"] == "installed"
    })
    .await;
    assert_eq!(
        detail["details"]["install_message"],
        "Installed Minecraft 26.3 with fabric"
    );
    assert_eq!(detail["runtime"]["game"]["loader"], "fabric");

    // People without access cannot read or control the server.
    for (method, path, body) in [
        ("GET", server.clone(), None),
        ("GET", format!("{server}/console"), None),
        (
            "POST",
            format!("{server}/console"),
            Some(json!({"command": "op outsider"})),
        ),
        (
            "POST",
            format!("{server}/power"),
            Some(json!({"action": "stop"})),
        ),
        ("GET", format!("{server}/extensions"), None),
        ("GET", format!("{server}/activity"), None),
        (
            "DELETE",
            server.clone(),
            Some(json!({"confirm_name": "Bots"})),
        ),
    ] {
        assert_eq!(
            send(&app, method, &path, outsider, body).await.0,
            StatusCode::FORBIDDEN,
            "{method} {path}"
        );
    }

    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{server}/console"),
            owner,
            Some(json!({"command": "list"}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{server}/power"),
            owner,
            Some(json!({"action": "start"}))
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{server}/power"),
            owner,
            Some(json!({"action": "explode"}))
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{server}/console"),
            owner,
            Some(json!({"command": "/player Bot_Alpha spawn"}))
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    // Everyone sees usage, including people without access to the server.
    let listed = send(&app, "GET", "/api/servers", outsider, None).await.1;
    let bots = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "Bots")
        .unwrap();
    assert_eq!(
        (
            bots["can_access"].as_bool(),
            bots["usage"]["cpu_percent"].as_f64()
        ),
        (Some(false), Some(150.0))
    );
    assert_eq!(bots["usage"]["memory_bytes"], 2 * GIB);
    let console = send(
        &app,
        "GET",
        &format!("{server}/console?lines=50"),
        owner,
        None,
    )
    .await
    .1;
    assert_eq!(console["lines"], json!(["> player Bot_Alpha spawn"]));

    // Mods only change while the server is stopped.
    let (code, body) = send(
        &app,
        "POST",
        &format!("{server}/extensions"),
        owner,
        Some(json!({"project_id": "carpet"})),
    )
    .await;
    assert_eq!(
        (code, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("server_running"))
    );
    let hits = send(
        &app,
        "GET",
        &format!("{server}/modrinth?query=carpet"),
        owner,
        None,
    )
    .await
    .1;
    assert_eq!(hits[0]["project_id"], "carpet");
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{server}/power"),
            owner,
            Some(json!({"action": "stop"}))
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );

    let (code, body) = send(
        &app,
        "POST",
        &format!("{server}/extensions"),
        owner,
        Some(json!({"project_id": "carpet"})),
    )
    .await;
    assert_eq!(code, StatusCode::ACCEPTED, "{body}");
    assert_eq!(
        body["installing"][0]["version_number"], "1.0",
        "alpha builds are never chosen"
    );
    eventually(&app, &format!("{server}/extensions"), owner, |e| {
        e["managed"].as_array().is_some_and(|m| m.len() == 1)
    })
    .await;
    let (code, body) = send(
        &app,
        "POST",
        &format!("{server}/extensions"),
        owner,
        Some(json!({"project_id": "needsapi"})),
    )
    .await;
    assert_eq!(code, StatusCode::ACCEPTED, "{body}");
    let titles: Vec<_> = body["installing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["project_id"].as_str().unwrap(),
                r["dependency"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        titles,
        [("needsapi", false), ("fabricapi", true)],
        "required dependency added, optional skipped"
    );
    let extensions = eventually(&app, &format!("{server}/extensions"), owner, |e| {
        e["managed"].as_array().is_some_and(|m| m.len() == 3)
    })
    .await;
    assert_eq!(extensions["other_files"], json!([]));
    let (code, body) = send(
        &app,
        "POST",
        &format!("{server}/extensions"),
        owner,
        Some(json!({"project_id": "carpet"})),
    )
    .await;
    assert_eq!(
        (code, &body["installing"]),
        (StatusCode::OK, &json!([])),
        "already installed is a no-op"
    );
    for (project, expected) in [
        ("clientonly", "only runs on game clients"),
        ("oldonly", "has no release for fabric 26.3"),
        ("conflicts", "incompatible"),
    ] {
        let (code, body) = send(
            &app,
            "POST",
            &format!("{server}/extensions"),
            owner,
            Some(json!({"project_id": project})),
        )
        .await;
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY, "{project}");
        assert!(
            body["message"].as_str().unwrap().contains(expected),
            "{project}: {body}"
        );
    }
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{server}/extensions"),
            owner,
            Some(json!({"project_id": "../etc"}))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &app,
            "DELETE",
            &format!("{server}/extensions/carpet"),
            owner,
            None
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    eventually(&app, &format!("{server}/extensions"), owner, |e| {
        e["managed"].as_array().is_some_and(|m| m.len() == 2)
    })
    .await;
    // A file removed by hand is dropped from the managed list instead of being reported as present.
    fake.lock()
        .unwrap()
        .servers
        .get_mut(&id.parse().unwrap())
        .unwrap()
        .files
        .retain(|f| f != "fabricapi-1.0.jar");
    let extensions = send(&app, "GET", &format!("{server}/extensions"), owner, None)
        .await
        .1;
    assert_eq!(extensions["managed"].as_array().unwrap().len(), 1);

    let activity = send(&app, "GET", &format!("{server}/activity"), owner, None)
        .await
        .1;
    let actions: Vec<_> = activity
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["action"].as_str().unwrap())
        .collect();
    assert!(
        actions.contains(&"install")
            && actions.contains(&"start")
            && actions.contains(&"console_command")
            && actions.contains(&"install_extension")
    );
    assert!(
        !activity.to_string().contains("Bot_Alpha"),
        "command text is not logged"
    );

    // A failed install can be retried; nothing else can be retried.
    fake.lock().unwrap().fail_next_install = true;
    let (_, second) = send(
        &app,
        "POST",
        "/api/servers",
        owner,
        Some(create("Broken", &allocations[1])),
    )
    .await;
    let second = format!("/api/servers/{}", second["id"].as_str().unwrap());
    tick().await.unwrap();
    let failed = eventually(&app, &second, owner, |d| {
        d["details"]["install_state"] == "failed"
    })
    .await;
    assert!(
        failed["details"]["install_message"]
            .as_str()
            .unwrap()
            .contains("exited with code 1")
    );
    assert_eq!(
        send(&app, "POST", &format!("{server}/reinstall"), owner, None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(&app, "POST", &format!("{second}/reinstall"), owner, None)
            .await
            .0,
        StatusCode::ACCEPTED
    );
    fake.lock()
        .unwrap()
        .servers
        .values_mut()
        .for_each(|s| s.fail_install = false);
    tick().await.unwrap();
    eventually(&app, &second, owner, |d| {
        d["details"]["install_state"] == "installed"
    })
    .await;

    // File manager: edits detect conflicts, uploads and downloads stream in chunks.
    let files = format!("{server}/files");
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{files}/folder"),
            owner,
            Some(json!({"path": "config"}))
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let (code, saved) = send(
        &app,
        "PUT",
        &format!("{files}/text"),
        owner,
        Some(json!({"path": "config/motd.txt", "text": "hello", "version": null})),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    let opened = send(
        &app,
        "GET",
        &format!("{files}/text?path=config/motd.txt"),
        owner,
        None,
    )
    .await
    .1;
    assert_eq!(
        (opened["text"].as_str(), &opened["version"]),
        (Some("hello"), &saved["version"])
    );
    let (code, _) = send(
        &app,
        "PUT",
        &format!("{files}/text"),
        owner,
        Some(json!({"path": "config/motd.txt", "text": "new", "version": "0".repeat(64)})),
    )
    .await;
    assert_eq!(code, StatusCode::CONFLICT, "stale version is refused");
    let (code, _) = send(
        &app,
        "PUT",
        &format!("{files}/text"),
        owner,
        Some(json!({"path": "config/motd.txt", "text": "new", "version": null})),
    )
    .await;
    assert_eq!(code, StatusCode::CONFLICT, "creating never overwrites");
    let payload: Vec<u8> = (0..(3 * 1024 * 1024 + 12345))
        .map(|i| (i % 251) as u8)
        .collect();
    let uploaded = app
        .clone()
        .oneshot(
            Request::post(format!("{files}/upload?path=config/world.bin"))
                .header("cookie", owner)
                .header("origin", ORIGIN)
                .body(Body::from(payload.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(uploaded.status(), StatusCode::CREATED);
    let downloaded = app
        .clone()
        .oneshot(
            Request::get(format!("{files}/download?path=config/world.bin"))
                .header("cookie", owner)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        downloaded.headers()["content-length"],
        payload.len().to_string()
    );
    assert_eq!(
        downloaded.headers()["content-disposition"],
        "attachment; filename=\"world.bin\""
    );
    assert_eq!(
        to_bytes(downloaded.into_body(), payload.len() + 1)
            .await
            .unwrap()
            .to_vec(),
        payload
    );
    let empty = app
        .clone()
        .oneshot(
            Request::post(format!("{files}/upload?path=config/empty.txt"))
                .header("cookie", owner)
                .header("origin", ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(empty.status(), StatusCode::CREATED);
    let listing = send(&app, "GET", &format!("{files}?path=config"), owner, None)
        .await
        .1;
    let names: Vec<_> = listing
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["empty.txt", "motd.txt", "world.bin"]);
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{files}/rename"),
            owner,
            Some(json!({"from": "config/motd.txt", "to": "config/world.bin"}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{files}/rename"),
            owner,
            Some(json!({"from": "config/motd.txt", "to": "motd.txt"}))
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    for bad in ["../outside", "config/../../x"] {
        let code = send(
            &app,
            "GET",
            &format!("{files}/text?path={bad}"),
            owner,
            None,
        )
        .await
        .0;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{bad}");
    }
    // A leading slash means the server's own root, never the host's.
    let (code, body) = send(
        &app,
        "GET",
        &format!("{files}/text?path=/etc/passwd"),
        owner,
        None,
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND);
    assert!(!body.to_string().contains("root:x:0:0"));
    assert_eq!(
        send(&app, "GET", &format!("{files}?path=config"), outsider, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "DELETE", &format!("{files}?path=config"), owner, None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(&app, "GET", &format!("{files}?path=config"), owner, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let activity = send(&app, "GET", &format!("{server}/activity"), owner, None)
        .await
        .1
        .to_string();
    assert!(
        activity.contains("upload_file")
            && activity.contains("edit_file")
            && activity.contains("delete_file")
    );

    // Backups: retention lowering and restores need confirmation; downloads stream and stop
    // when access is revoked midway.
    let backups = format!("{server}/backups");
    assert_eq!(
        send(&app, "GET", &backups, owner, None).await.1["retention"],
        3
    );
    for _ in 0..3 {
        assert_eq!(
            send(&app, "POST", &backups, owner, None).await.0,
            StatusCode::ACCEPTED
        );
        eventually(&app, &server, owner, |d| {
            d["runtime"]["operation"]["state"] == "succeeded"
        })
        .await;
    }
    let list = send(&app, "GET", &backups, owner, None).await.1;
    assert_eq!(list["backups"].as_array().unwrap().len(), 3);
    let (code, body) = send(
        &app,
        "PUT",
        &format!("{backups}/retention"),
        owner,
        Some(json!({"retention": 1})),
    )
    .await;
    assert_eq!(
        (code, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("confirm_removal"))
    );
    let (code, body) = send(
        &app,
        "PUT",
        &format!("{backups}/retention"),
        owner,
        Some(json!({"retention": 2, "remove_oldest": true})),
    )
    .await;
    assert_eq!(
        (code, body["removed"].as_array().unwrap().len()),
        (StatusCode::OK, 1)
    );
    assert_eq!(
        send(
            &app,
            "PUT",
            &format!("{backups}/retention"),
            owner,
            Some(json!({"retention": 6}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let list = send(&app, "GET", &backups, owner, None).await.1;
    assert_eq!(
        (
            list["retention"].as_u64(),
            list["backups"].as_array().unwrap().len()
        ),
        (Some(2), 2)
    );
    let newest = list["backups"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{backups}/{newest}/restore"),
            owner,
            Some(json!({"confirm": false}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    send(
        &app,
        "POST",
        &format!("{server}/power"),
        owner,
        Some(json!({"action": "start"})),
    )
    .await;
    let (code, body) = send(
        &app,
        "POST",
        &format!("{backups}/{newest}/restore"),
        owner,
        Some(json!({"confirm": true})),
    )
    .await;
    assert_eq!(
        (code, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("server_running"))
    );
    send(
        &app,
        "POST",
        &format!("{server}/power"),
        owner,
        Some(json!({"action": "stop"})),
    )
    .await;
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{backups}/{newest}/restore"),
            owner,
            Some(json!({"confirm": true}))
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    eventually(&app, &server, owner, |d| {
        d["runtime"]["operation"]["kind"] == "restore"
            && d["runtime"]["operation"]["state"] == "succeeded"
    })
    .await;
    assert_eq!(
        send(
            &app,
            "GET",
            &format!("{backups}/{newest}/download"),
            outsider,
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let full = app
        .clone()
        .oneshot(
            Request::get(format!("{backups}/{newest}/download"))
                .header("cookie", owner)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(full.status(), StatusCode::OK);
    assert_eq!(full.headers()["content-length"], BACKUP_BYTES.to_string());
    assert!(
        full.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment; filename=\"Bots-")
    );
    assert_eq!(
        to_bytes(full.into_body(), BACKUP_BYTES + 1)
            .await
            .unwrap()
            .len(),
        BACKUP_BYTES
    );
    let grant = |enabled: bool| json!({"enabled": enabled});
    assert_eq!(
        send(
            &app,
            "PUT",
            &format!("{server}/members/{}", member_id.0),
            owner,
            Some(grant(true))
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let streaming = app
        .clone()
        .oneshot(
            Request::get(format!("{backups}/{newest}/download"))
                .header("cookie", member)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(streaming.status(), StatusCode::OK);
    let mut chunks = streaming.into_body().into_data_stream();
    use futures_util::StreamExt;
    let mut received = 0usize;
    while received < 1024 * 1024 {
        received += chunks.next().await.unwrap().unwrap().len();
    }
    assert_eq!(
        send(
            &app,
            "PUT",
            &format!("{server}/members/{}", member_id.0),
            owner,
            Some(grant(false))
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let mut failed = false;
    while let Some(chunk) = chunks.next().await {
        match chunk {
            Ok(bytes) => received += bytes.len(),
            Err(_) => {
                failed = true;
                break;
            }
        }
    }
    assert!(
        failed && received < BACKUP_BYTES,
        "revoked download stopped after {received} bytes"
    );

    // Startup settings: only editable variables and the egg's images; applied at the next start.
    let settings = send(&app, "GET", &format!("{server}/settings"), member, None).await;
    assert_eq!(settings.0, StatusCode::FORBIDDEN, "revoked member");
    let (code, settings) = send(&app, "GET", &format!("{server}/settings"), owner, None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(settings["image"], "ghcr.io/ptero-eggs/yolks:java_25");
    assert_eq!(settings["variables"][1]["env_variable"], "MC_VERSION");
    assert_eq!(settings["variables"][1]["value"], "latest");
    for (change, code) in [
        (
            json!({"image": "docker.io/library/alpine", "variables": {}}),
            "invalid_setting",
        ),
        (
            json!({"image": "ghcr.io/ptero-eggs/yolks:java_21", "variables": {"SERVER_JARFILE": "../x"}}),
            "invalid_setting",
        ),
        (
            json!({"image": "ghcr.io/ptero-eggs/yolks:java_21", "variables": {"UNKNOWN": "1"}}),
            "invalid_setting",
        ),
    ] {
        let (status, body) = send(
            &app,
            "PUT",
            &format!("{server}/settings"),
            owner,
            Some(change),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::BAD_REQUEST, Some(code)),
            "{body}"
        );
    }
    assert_eq!(
        send(
            &app,
            "PUT",
            &format!("{server}/settings"),
            outsider,
            Some(json!({"image": "ghcr.io/ptero-eggs/yolks:java_21"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (code, body) = send(
        &app,
        "PUT",
        &format!("{server}/settings"),
        owner,
        Some(json!({"image": "ghcr.io/ptero-eggs/yolks:java_21", "variables": {"MC_VERSION": "1.21.10", "LOADER_VERSION": "latest"}})),
    )
    .await;
    assert_eq!(
        (code, body["changed"].clone()),
        (StatusCode::OK, json!(["image", "MC_VERSION"]))
    );

    // Limits: admitted against the rest of the host, disk only grows.
    let limits = |memory: u64, disk: u64, cpu: u32| json!({"memory_bytes": memory * GIB, "disk_bytes": disk * GIB, "cpu_millis": cpu});
    for (change, status, code) in [
        (limits(3, 2, 1000), StatusCode::BAD_REQUEST, "invalid_input"),
        (
            limits(64, 4, 1000),
            StatusCode::CONFLICT,
            "capacity_exceeded",
        ),
        (
            limits(3, 4, 64_000),
            StatusCode::CONFLICT,
            "capacity_exceeded",
        ),
        (limits(0, 4, 1000), StatusCode::BAD_REQUEST, "invalid_input"),
    ] {
        let (got, body) = send(
            &app,
            "PUT",
            &format!("{server}/limits"),
            owner,
            Some(change),
        )
        .await;
        assert_eq!((got, body["code"].as_str()), (status, Some(code)), "{body}");
    }
    let (code, body) = send(
        &app,
        "PUT",
        &format!("{server}/limits"),
        owner,
        Some(limits(4, 6, 1500)),
    )
    .await;
    assert_eq!((code, body), (StatusCode::OK, limits(4, 6, 1500)));
    for action in ["stop", "start"] {
        let (code, body) = send(
            &app,
            "POST",
            &format!("{server}/power"),
            owner,
            Some(json!({"action": action})),
        )
        .await;
        assert!(code.is_success(), "{action}: {body}");
    }
    let started = fake.lock().unwrap().last_start.clone().unwrap();
    assert_eq!(started.image, "ghcr.io/ptero-eggs/yolks:java_21");
    assert_eq!(started.variables["MC_VERSION"], "1.21.10");
    assert_eq!(
        (
            started.limits.memory_bytes,
            started.limits.disk_bytes,
            started.limits.cpu_millis
        ),
        (4 * GIB, 6 * GIB, 1500)
    );
    let activity = send(&app, "GET", &format!("{server}/activity"), owner, None)
        .await
        .1;
    let text = activity.to_string();
    assert!(
        text.contains("image, MC_VERSION") && !text.contains("1.21.10"),
        "{text}"
    );
    assert!(
        text.contains("4.0 GiB RAM, 6.0 GiB disk, 1.50 CPU"),
        "{text}"
    );

    // Members and teammates are listed for whoever manages the server.
    let (code, members) = send(&app, "GET", &format!("{server}/members"), owner, None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        members,
        json!([{"user_id": member_id.0, "username": "member", "enabled": false}])
    );
    assert_eq!(
        send(&app, "GET", &format!("{server}/members"), outsider, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let people = send(&app, "GET", "/api/users", outsider, None).await.1;
    assert_eq!(
        people
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["username"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["admin", "member", "outsider", "owner"]
    );

    // Schedules: validated, each occurrence runs once, and missed ones are not run late.
    let schedules = format!("{server}/schedules");
    let plan = |action: &str| json!({"action": action, "hour": 4, "minute": 30, "weekdays": 127, "timezone": "Europe/Amsterdam", "enabled": true});
    let mut invalid = vec![
        plan("command"),
        plan("restart"),
        plan("restart"),
        plan("restart"),
    ];
    invalid[1]["timezone"] = json!("Mars/Base");
    invalid[2]["weekdays"] = json!(0);
    invalid[3]["hour"] = json!(24);
    for body in invalid {
        let (code, body) = send(&app, "POST", &schedules, owner, Some(body)).await;
        assert_eq!(
            (code, body["code"].as_str()),
            (StatusCode::BAD_REQUEST, Some("invalid_schedule")),
            "{body}"
        );
    }
    assert_eq!(
        send(&app, "POST", &schedules, outsider, Some(plan("restart")))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let mut command = plan("command");
    command["command"] = json!("say hello");
    let (code, created) = send(&app, "POST", &schedules, owner, Some(command)).await;
    assert_eq!(code, StatusCode::CREATED, "{created}");
    let due = created["next_run_at"].as_i64().unwrap();
    assert!(due > now() && due <= now() + 7 * 86400);
    let mut update = plan("update");
    update["hour"] = json!(5);
    let (_, update) = send(&app, "POST", &schedules, owner, Some(update)).await;
    let update_due = update["next_run_at"].as_i64().unwrap();
    assert!(update_due > due);
    schedules::tick(&database, &agent, due + 5).await.unwrap();
    schedules::tick(&database, &agent, due + 6).await.unwrap();
    for _ in 0..100 {
        if fake.lock().unwrap().servers[&Uuid::parse_str(&id).unwrap()]
            .commands
            .iter()
            .any(|c| c == "say hello")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let sent = fake.lock().unwrap().servers[&Uuid::parse_str(&id).unwrap()]
        .commands
        .iter()
        .filter(|c| *c == "say hello")
        .count();
    assert_eq!(sent, 1, "one occurrence runs once");
    // The panel was down for the update's time: it is recorded as missed, not run late.
    schedules::tick(&database, &agent, update_due + 3600)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(fake.lock().unwrap().updates.is_empty());
    let listed = eventually(&app, &schedules, owner, |list| {
        list.as_array()
            .is_some_and(|l| l.iter().any(|s| s["last_outcome"] == "command sent"))
    })
    .await;
    let missed = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["action"] == "update")
        .unwrap();
    assert!(
        missed["last_outcome"]
            .as_str()
            .unwrap()
            .starts_with("missed"),
        "{missed}"
    );
    assert!(missed["next_run_at"].as_i64().unwrap() > update_due + 3600);
    let activity = send(&app, "GET", &format!("{server}/activity"), owner, None)
        .await
        .1
        .to_string();
    assert!(
        activity.contains("scheduled_command") && activity.contains("create_schedule"),
        "{activity}"
    );
    let schedule_id = created["id"].as_str().unwrap();
    let mut paused = plan("command");
    paused["command"] = json!("say bye");
    paused["enabled"] = json!(false);
    assert_eq!(
        send(
            &app,
            "PUT",
            &format!("{schedules}/{schedule_id}"),
            owner,
            Some(paused)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    schedules::tick(&database, &agent, due + 7 * 86400 + 5)
        .await
        .unwrap();
    assert_eq!(
        send(
            &app,
            "DELETE",
            &format!("{schedules}/{schedule_id}"),
            owner,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(
            &app,
            "DELETE",
            &format!("{schedules}/{schedule_id}"),
            owner,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !fake.lock().unwrap().servers[&Uuid::parse_str(&id).unwrap()]
            .commands
            .iter()
            .any(|c| c == "say bye"),
        "paused schedules do not run"
    );

    // A manual update may change versions; scheduled ones keep the installed game version.
    assert_eq!(
        send(&app, "POST", &format!("{server}/update"), owner, None)
            .await
            .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        fake.lock().unwrap().updates,
        [(Uuid::parse_str(&id).unwrap(), false)]
    );
    eventually(&app, &server, owner, |d| {
        d["runtime"]["operation"]["state"] == "succeeded"
    })
    .await;

    // A copied legacy server (tar of its folder) becomes an imported backup, ready to restore.
    let legacy = dir.path().join("legacy-volume");
    std::fs::create_dir_all(legacy.join("world")).unwrap();
    std::fs::write(legacy.join("world/level.dat"), b"legacy world").unwrap();
    let archive = dir.path().join("legacy.tar.gz");
    assert!(
        std::process::Command::new("tar")
            .arg("czf")
            .arg(&archive)
            .arg("-C")
            .arg(&legacy)
            .arg(".")
            .status()
            .unwrap()
            .success()
    );
    let upload = |cookie: &str, bytes: Vec<u8>| {
        Request::post(format!("{server}/import"))
            .header("cookie", cookie)
            .header("origin", ORIGIN)
            .body(Body::from(bytes))
            .unwrap()
    };
    let response = app
        .clone()
        .oneshot(upload(owner, std::fs::read(&archive).unwrap()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let imported: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert_eq!(imported["kind"], "imported");
    let listed = send(&app, "GET", &format!("{server}/backups"), owner, None)
        .await
        .1;
    assert!(
        listed["backups"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["id"] == imported["id"])
    );
    let response = app
        .clone()
        .oneshot(upload(owner, vec![7u8; 3 * 1024 * 1024 + 5]))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let response = app
        .clone()
        .oneshot(upload(outsider, std::fs::read(&archive).unwrap()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let activity = send(&app, "GET", &format!("{server}/activity"), owner, None)
        .await
        .1
        .to_string();
    assert!(activity.contains("import_files"), "{activity}");

    // After a failed update, a scheduled update pauses itself instead of retrying.
    {
        let mut agent = fake.lock().unwrap();
        let server = agent
            .servers
            .get_mut(&Uuid::parse_str(&id).unwrap())
            .unwrap();
        server.operation = op(
            OperationKind::Update,
            OperationState::Failed,
            "The update failed: installer exited with code 1",
        );
    }
    let (_, retry) = send(&app, "POST", &schedules, owner, Some(plan("update"))).await;
    let retry_due = retry["next_run_at"].as_i64().unwrap();
    schedules::tick(&database, &agent, retry_due + 5)
        .await
        .unwrap();
    let paused = eventually(&app, &schedules, owner, |list| {
        list.as_array().is_some_and(|l| {
            l.iter()
                .any(|s| s["id"] == retry["id"] && s["enabled"] == false)
        })
    })
    .await;
    let paused = paused
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == retry["id"])
        .unwrap();
    assert!(
        paused["last_outcome"]
            .as_str()
            .unwrap()
            .contains("the last update failed"),
        "{paused}"
    );
    assert_eq!(fake.lock().unwrap().updates.len(), 1, "no automatic retry");

    // Deletion needs the exact name, then frees the reservation and the allocation.
    let (code, body) = send(
        &app,
        "DELETE",
        &server,
        owner,
        Some(json!({"confirm_name": "bots"})),
    )
    .await;
    assert_eq!(
        (code, body["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("confirmation_mismatch"))
    );
    assert_eq!(
        send(
            &app,
            "DELETE",
            &server,
            owner,
            Some(json!({"confirm_name": "Bots", "keep_backups": true}))
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{server}/power"),
            owner,
            Some(json!({"action": "start"}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    for _ in 0..3 {
        tick().await.unwrap();
    }
    assert_eq!(
        send(&app, "GET", &server, owner, None).await.0,
        StatusCode::NOT_FOUND
    );
    let pool = send(&app, "GET", "/api/allocations", owner, None).await.1;
    assert_eq!(pool[0]["id"], allocations[0]);
    let host = send(&app, "GET", "/api/host", owner, None).await.1;
    assert_eq!(
        host["reserved"]["memory_bytes"],
        3 * GIB,
        "only the remaining server is reserved"
    );

    // Kept backups stay visible to the deletion-time access list only.
    let retained = send(&app, "GET", "/api/retained-backups", owner, None)
        .await
        .1;
    // Two backups plus the imported legacy archive.
    assert_eq!(
        (
            retained[0]["name"].as_str(),
            retained[0]["backups"].as_array().unwrap().len()
        ),
        (Some("Bots"), 3)
    );
    assert_eq!(
        send(&app, "GET", "/api/retained-backups", outsider, None)
            .await
            .1,
        json!([])
    );
    assert_eq!(
        send(&app, "GET", "/api/retained-backups", member, None)
            .await
            .1,
        json!([]),
        "member access was revoked before deletion"
    );
    assert_eq!(
        send(&app, "GET", "/api/retained-backups", admin_cookie, None)
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let download = format!("{backups}/{newest}/download");
    assert_eq!(
        app.clone()
            .oneshot(
                Request::get(&download)
                    .header("cookie", owner)
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(&app, "GET", &download, outsider, None).await.0,
        StatusCode::NOT_FOUND
    );
    for backup in retained[0]["backups"].as_array().unwrap() {
        let id = backup["id"].as_str().unwrap();
        assert_eq!(
            send(&app, "DELETE", &format!("{backups}/{id}"), owner, None)
                .await
                .0,
            StatusCode::NO_CONTENT
        );
    }
    assert_eq!(
        send(&app, "GET", "/api/retained-backups", owner, None)
            .await
            .1,
        json!([])
    );
}
