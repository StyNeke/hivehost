use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use manager_api::{auth::AuthStore, http::router};
use serde_json::json;
use tower::ServiceExt;

const ORIGIN: &str = "https://panel.example.test";

#[tokio::test]
async fn login_rejects_cross_origin_and_uses_secure_revocable_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.sqlite");
    AuthStore::open(&path)
        .unwrap()
        .bootstrap_admin("stijn", "a long test password")
        .unwrap();
    let app = router(path, ORIGIN.into());
    let login = || json!({"username":"stijn","password":"a long test password"}).to_string();
    let request = Request::post("/api/login")
        .header("content-type", "application/json")
        .body(Body::from(login()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let request = Request::post("/api/login")
        .header("origin", "https://evil.test")
        .header("content-type", "application/json")
        .body(Body::from(login()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let request = Request::post("/api/login")
        .header("origin", ORIGIN)
        .header("content-type", "application/json")
        .body(Body::from(login()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("Secure"));
    assert!(cookie.contains("SameSite=Strict"));
    let session = cookie.split(';').next().unwrap().to_owned();
    let request = Request::get("/api/session")
        .header("cookie", &session)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::OK
    );
    let request = Request::post("/api/logout")
        .header("origin", ORIGIN)
        .header("cookie", &session)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    let request = Request::get("/api/session")
        .header("cookie", &session)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.oneshot(request).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn login_attempts_are_bounded_and_admin_invite_requires_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(dir.path().join("app.sqlite"), ORIGIN.into());
    let request = Request::post("/api/invites")
        .header("origin", ORIGIN)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let mut limited = false;
    for _ in 0..12 {
        let request = Request::post("/api/login")
            .header("origin", ORIGIN)
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"username":"unknown","password":"incorrect password"}).to_string(),
            ))
            .unwrap();
        if app.clone().oneshot(request).await.unwrap().status() == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
    }
    assert!(limited);
}
