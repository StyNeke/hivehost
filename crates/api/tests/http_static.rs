use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use manager_api::http::{router, with_static_files};
use tower::ServiceExt;

#[tokio::test]
async fn interface_is_served_with_strict_headers_and_api_paths_never_fall_back() {
    let dir = tempfile::tempdir().unwrap();
    let site = dir.path().join("site");
    std::fs::create_dir_all(site.join("assets")).unwrap();
    std::fs::write(
        site.join("index.html"),
        "<!doctype html><title>Game Manager</title>",
    )
    .unwrap();
    std::fs::write(site.join("assets/app.js"), "console.log(1)").unwrap();
    std::fs::write(dir.path().join("secret.txt"), "outside").unwrap();
    let app = with_static_files(
        router(
            dir.path().join("app.sqlite"),
            "https://panel.example.test".into(),
        ),
        &site,
    );
    let get = |uri: &str| Request::get(uri).body(Body::empty()).unwrap();

    let page = app.clone().oneshot(get("/")).await.unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let policy = page.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(policy.contains("default-src 'self'") && policy.contains("frame-ancestors 'none'"));
    assert_eq!(page.headers()["x-frame-options"], "DENY");
    assert!(
        String::from_utf8(to_bytes(page.into_body(), 1024).await.unwrap().to_vec())
            .unwrap()
            .contains("Game Manager")
    );

    let script = app.clone().oneshot(get("/assets/app.js")).await.unwrap();
    assert_eq!(script.status(), StatusCode::OK);
    assert_eq!(script.headers()["x-content-type-options"], "nosniff");

    let deep = app.clone().oneshot(get("/servers/anything")).await.unwrap();
    assert_eq!(
        deep.status(),
        StatusCode::OK,
        "client routes fall back to the interface"
    );
    for escape in [
        "/../secret.txt",
        "/assets/../../secret.txt",
        "/%2e%2e/secret.txt",
    ] {
        let response = app.clone().oneshot(get(escape)).await.unwrap();
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert!(
            !String::from_utf8_lossy(&body).contains("outside"),
            "{escape} escaped the site directory"
        );
    }
    let api = app
        .clone()
        .oneshot(get("/api/does-not-exist"))
        .await
        .unwrap();
    assert_eq!(api.status(), StatusCode::NOT_FOUND);
    assert_eq!(api.headers()["content-type"], "application/json");
    assert_eq!(
        app.oneshot(get("/api/session")).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}
