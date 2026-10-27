use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use manager_api::{auth::AuthStore, http::router, servers::ServerStore};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn dashboard_does_not_expose_details_and_access_is_checked_on_every_request() {
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
    let friend = auth
        .register(&invite, "friend", "a long test password", now)
        .unwrap();
    let member_cookie = format!(
        "__Host-gm_session={}",
        auth.login("friend", "a long test password", now).unwrap()
    );
    let admin_cookie = format!(
        "__Host-gm_session={}",
        auth.login("admin", "a long test password", now).unwrap()
    );
    ServerStore::open(&path).unwrap();
    let id = Uuid::new_v4();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO servers VALUES (?1, 'Minecraft', ?2)",
            rusqlite::params![id.to_string(), admin.id.0.to_string()],
        )
        .unwrap();
    let app = router(path, "https://panel.example.test".into());
    let request = Request::get("/api/servers")
        .header("cookie", &member_cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let servers: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap();
    assert_eq!(servers[0]["name"], "Minecraft");
    assert_eq!(servers[0]["can_access"], false);
    assert!(servers[0].get("owner").is_none());
    let request = Request::get(format!("/api/servers/{id}"))
        .header("cookie", &member_cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let request = Request::put(format!("/api/servers/{id}/members/{}", friend.id.0))
        .header("origin", "https://panel.example.test")
        .header("cookie", admin_cookie)
        .header("content-type", "application/json")
        .body(Body::from(r#"{"enabled":true}"#))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    let request = Request::get(format!("/api/servers/{id}"))
        .header("cookie", &member_cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.oneshot(request).await.unwrap().status(), StatusCode::OK);
}
