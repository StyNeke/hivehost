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
    agent_client::AgentClient, auth::AuthStore, eggs::EggStore, http::router_with_services, worker,
};
use manager_core::{
    capacity::Resources,
    host::{HostAdmission, HostHardware, HostStatus},
};
use manager_protocol::{
    AgentRequest, AgentResponse, AgentResult, DetectedGame, Operation, OperationKind,
    OperationState, OperationStatus, PowerState, Rejection, RuntimeStatus, VERSION,
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
}

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
                        OperationKind::Extensions => {
                            current.state = OperationState::Succeeded;
                            current.message = "Installed".into();
                        }
                        OperationKind::Delete => {
                            self.servers.remove(&server_id);
                            return rejection("not_found", "server not found");
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
            Operation::DeleteServer { server_id } => {
                if let Some(server) = self.servers.get_mut(&server_id) {
                    server.operation =
                        op(OperationKind::Delete, OperationState::Running, "Deleting");
                }
                AgentResult::Accepted
            }
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
    for name in ["owner", "outsider"] {
        let invite = auth.invite(&admin, now()).unwrap();
        auth.register(&invite, name, "a long test password", now())
            .unwrap();
    }
    for name in ["admin", "owner", "outsider"] {
        cookies.push(format!(
            "__Host-gm_session={}",
            auth.login(name, "a long test password", now()).unwrap()
        ));
    }
    let (admin_cookie, owner, outsider) = (&cookies[0], &cookies[1], &cookies[2]);
    let revision = EggStore::open(&database)
        .unwrap()
        .import(
            admin.id,
            include_bytes!("../../../tests/fixtures/eggs/fabric.json"),
        )
        .unwrap()
        .revision;
    let fake = Arc::new(Mutex::new(FakeAgent::default()));
    let socket = dir.path().join("agent.sock");
    spawn_agent(&socket, fake.clone());
    let agent = AgentClient::new(&socket, std::fs::metadata("/proc/self").unwrap().uid());
    let app = router_with_services(
        database.clone(),
        ORIGIN.into(),
        Some(agent.clone()),
        &spawn_modrinth().await,
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
            Some(json!({"confirm_name": "Bots"}))
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
}
