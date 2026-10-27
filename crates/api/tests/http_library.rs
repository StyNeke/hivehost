use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use manager_api::{auth::AuthStore, http::router};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;

const ORIGIN: &str = "https://panel.example.test";
const JAVA: &[u8] = include_bytes!("../../../tests/fixtures/eggs/java.json");

#[tokio::test]
async fn members_can_import_and_every_authenticated_member_can_list_the_library() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.sqlite");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mut auth = AuthStore::open(&path).unwrap();
    let admin = auth
        .bootstrap_admin("admin", "a long test password")
        .unwrap();
    let invite = auth.invite(&admin, now).unwrap();
    auth.register(&invite, "friend", "a long test password", now)
        .unwrap();
    let token = auth.login("friend", "a long test password", now).unwrap();
    let cookie = format!("__Host-gm_session={token}");
    let app = router(path, ORIGIN.into());
    let request = Request::get("/api/eggs").body(Body::empty()).unwrap();
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let request = Request::post("/api/eggs")
        .header("origin", ORIGIN)
        .header("cookie", &cookie)
        .header("content-type", "application/json")
        .body(Body::from(JAVA))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap();
    assert_eq!(created["runtime_qualified"], false);
    let request = Request::get("/api/eggs")
        .header("cookie", &cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let listed: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap();
    assert_eq!(listed[0]["name"], "Paper");
    let request = Request::post("/api/eggs")
        .header("origin", ORIGIN)
        .header("cookie", &cookie)
        .body(Body::from("invalid JSON"))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let error: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap();
    assert_eq!(error["code"], "unsupported_egg");
    assert!(error["details"][0]["path"].is_string());
}
