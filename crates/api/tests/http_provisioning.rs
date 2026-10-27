#![cfg(target_os = "linux")]

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use manager_api::{
    agent_client::AgentClient,
    auth::AuthStore,
    eggs::EggStore,
    http::{router, router_with_agent},
};
use manager_core::{
    capacity::Resources,
    host::{HostAdmission, HostHardware, HostStatus},
    model::EggRevisionId,
};
use manager_protocol::{AgentRequest, AgentResponse, AgentResult, VERSION};
use serde_json::{Value, json};
use std::{
    os::unix::fs::MetadataExt,
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
};
use tower::ServiceExt;

const ORIGIN: &str = "https://panel.example.test";
const GIB: u64 = 1024 * 1024 * 1024;

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn status(revision: EggRevisionId) -> HostStatus {
    HostStatus {
        hardware: HostHardware {
            cpu_millis: 6000,
            memory_bytes: 24 * GIB,
            disk_total_bytes: 100 * GIB,
            disk_available_bytes: 70 * GIB,
        },
        admission: HostAdmission {
            measured_at: 0,
            budget: Resources {
                memory_bytes: 16 * GIB,
                disk_bytes: 40 * GIB,
                cpu_millis: 4000,
            },
            legacy_reserved: Resources {
                memory_bytes: 4 * GIB,
                disk_bytes: 0,
                cpu_millis: 1000,
            },
            disk_quotas_ready: true,
            qualified_revisions: vec![revision],
            private_ips: vec!["100.116.183.201".parse().unwrap()],
            unavailable_bindings: vec![],
        },
        blockers: vec![],
    }
}

// Answers every connection with the current template, stamped as freshly measured.
fn fake_agent(path: &Path, template: Arc<Mutex<HostStatus>>) {
    let listener = UnixListener::bind(path).unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let template = template.clone();
            tokio::spawn(async move {
                let length = stream.read_u32().await.unwrap();
                let mut bytes = vec![0; length as usize];
                stream.read_exact(&mut bytes).await.unwrap();
                let request: AgentRequest = serde_json::from_slice(&bytes).unwrap();
                let mut status = template.lock().unwrap().clone();
                status.admission.measured_at = now();
                let bytes = serde_json::to_vec(&AgentResponse {
                    version: VERSION,
                    request_id: request.request_id,
                    result: AgentResult::HostStatus(Box::new(status)),
                })
                .unwrap();
                stream.write_u32(bytes.len() as u32).await.unwrap();
                stream.write_all(&bytes).await.unwrap();
            });
        }
    });
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
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn creation(revision: EggRevisionId, allocation: &Value, memory_gib: u64) -> Value {
    json!({
        "name": "Survival",
        "egg_revision": revision,
        "limits": {"memory_bytes": memory_gib * GIB, "disk_bytes": 10 * GIB, "cpu_millis": 1000},
        "allocation_id": allocation,
        "image": "ghcr.io/ptero-eggs/yolks:java_21",
        "variables": {},
        "license_accepted": true
    })
}

#[tokio::test]
async fn creation_uses_trusted_agent_state_and_explains_capacity() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("app.sqlite");
    let mut auth = AuthStore::open(&database).unwrap();
    let admin = auth
        .bootstrap_admin("admin", "a long test password")
        .unwrap();
    let invite = auth.invite(&admin, now()).unwrap();
    let member = auth
        .register(&invite, "friend", "a long test password", now())
        .unwrap();
    let admin_cookie = format!(
        "__Host-gm_session={}",
        auth.login("admin", "a long test password", now()).unwrap()
    );
    let member_cookie = format!(
        "__Host-gm_session={}",
        auth.login("friend", "a long test password", now()).unwrap()
    );
    let revision = EggStore::open(&database)
        .unwrap()
        .import(
            member.id,
            include_bytes!("../../../tests/fixtures/eggs/java.json"),
        )
        .unwrap()
        .revision;

    // Without an agent nothing host-dependent is admitted.
    let unconfigured = router(database.clone(), ORIGIN.into());
    assert_eq!(
        send(&unconfigured, "GET", "/api/host", &member_cookie, None)
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let (code, body) = send(
        &unconfigured,
        "POST",
        "/api/servers",
        &member_cookie,
        Some(creation(revision, &json!(uuid::Uuid::new_v4()), 4)),
    )
    .await;
    assert_eq!(
        (code, body["code"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("host_unavailable"))
    );

    let socket = directory.path().join("agent.sock");
    let template = Arc::new(Mutex::new(status(revision)));
    fake_agent(&socket, template.clone());
    let uid = std::fs::metadata("/proc/self").unwrap().uid();
    let app = router_with_agent(
        database.clone(),
        ORIGIN.into(),
        AgentClient::new(&socket, uid),
    );

    let (code, host) = send(&app, "GET", "/api/host", &member_cookie, None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(host["ready"], true);
    assert_eq!(host["remaining"]["memory_bytes"], 12 * GIB);
    assert!(host.get("private_ips").is_none());

    let binding = json!({"ip": "100.116.183.201", "port": 25600, "transport": "tcp"});
    assert_eq!(
        send(
            &app,
            "POST",
            "/api/allocations",
            &member_cookie,
            Some(binding.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (code, created) = send(
        &app,
        "POST",
        "/api/allocations",
        &admin_cookie,
        Some(binding.clone()),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED);
    let allocation = created["id"].clone();
    assert_eq!(
        send(
            &app,
            "POST",
            "/api/allocations",
            &admin_cookie,
            Some(binding)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, pool) = send(&app, "GET", "/api/allocations", &member_cookie, None).await;
    assert_eq!(pool.as_array().unwrap().len(), 1);
    assert_eq!(pool[0]["id"], allocation);
    assert_eq!(pool[0]["port"], 25600);

    let (code, body) = send(
        &app,
        "POST",
        "/api/servers",
        &member_cookie,
        Some(creation(revision, &allocation, 13)),
    )
    .await;
    assert_eq!(
        (code, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("capacity_exceeded"))
    );

    let (code, server) = send(
        &app,
        "POST",
        "/api/servers",
        &member_cookie,
        Some(creation(revision, &allocation, 4)),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED);
    assert!(server["job_id"].is_string());
    let id = server["id"].as_str().unwrap();
    let (code, detail) = send(
        &app,
        "GET",
        &format!("/api/servers/{id}"),
        &member_cookie,
        None,
    )
    .await;
    assert_eq!(
        (code, detail["name"].as_str()),
        (StatusCode::OK, Some("Survival"))
    );
    assert_eq!(
        send(&app, "GET", "/api/allocations", &member_cookie, None)
            .await
            .1,
        json!([])
    );
    let (_, host) = send(&app, "GET", "/api/host", &member_cookie, None).await;
    assert_eq!(host["remaining"]["memory_bytes"], 8 * GIB);

    let (code, body) = send(
        &app,
        "POST",
        "/api/servers",
        &member_cookie,
        Some(creation(revision, &allocation, 1)),
    )
    .await;
    assert_eq!(
        (code, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("allocation_unavailable"))
    );

    // Agent-reported blockers are passed through and stop creation.
    {
        let mut template = template.lock().unwrap();
        template.admission.disk_quotas_ready = false;
        template.blockers = vec!["Storage quota setup has not been qualified".into()];
    }
    let (_, host) = send(&app, "GET", "/api/host", &member_cookie, None).await;
    assert_eq!(host["ready"], false);
    assert_eq!(
        host["blockers"][0],
        "Storage quota setup has not been qualified"
    );
    let (code, body) = send(
        &app,
        "POST",
        "/api/servers",
        &member_cookie,
        Some(creation(revision, &allocation, 1)),
    )
    .await;
    assert_eq!(
        (code, body["code"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("host_not_ready"))
    );
}
