use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::Path as UrlPath,
    http::{HeaderMap, Request, StatusCode},
    response::IntoResponse,
    routing::get,
};
use manager_api::{auth::AuthStore, http::router_with_services};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tower::ServiceExt;

const ORIGIN: &str = "https://panel.example.test";

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

// A GitHub stand-in that can be switched into the rate-limited state.
async fn spawn_github(limited: Arc<AtomicBool>, calls: Arc<AtomicUsize>) -> String {
    let guard = move |limited: Arc<AtomicBool>, calls: Arc<AtomicUsize>, body: Value| {
        calls.fetch_add(1, Ordering::SeqCst);
        if limited.load(Ordering::SeqCst) {
            let mut headers = HeaderMap::new();
            headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
            headers.insert("x-ratelimit-reset", "1900000000".parse().unwrap());
            return (
                StatusCode::FORBIDDEN,
                headers,
                Json(json!({"message": "API rate limit exceeded"})),
            )
                .into_response();
        }
        Json(body).into_response()
    };
    let route = |body: fn(&str) -> Value| {
        let (limited, calls, guard) = (limited.clone(), calls.clone(), guard);
        get(move |UrlPath((owner, repo)): UrlPath<(String, String)>| {
            let response = guard(
                limited.clone(),
                calls.clone(),
                body(&format!("{owner}/{repo}")),
            );
            async move { response }
        })
    };
    let app = Router::new()
        .route("/repos/{owner}/{repo}", route(|name| json!({
            "description": format!("{name} <b>server</b>"), "html_url": format!("https://github.com/{name}"),
            "pushed_at": "2026-10-27T10:00:00Z", "stargazers_count": 12345
        })))
        .route("/repos/{owner}/{repo}/releases", route(|name| json!([{
            "tag_name": "nightly", "name": "Nightly", "published_at": "2026-10-27T09:00:00Z", "prerelease": true,
            "html_url": format!("https://github.com/{name}/releases/tag/nightly"),
            "assets": [{"name": "server-X64-Linux-musl"}]
        }])))
        .route("/repos/{owner}/{repo}/commits", route(|name| json!([{
            "sha": "0123456789abcdef0123", "html_url": format!("https://github.com/{name}/commit/0123"),
            "commit": {"message": "Add chunk saving\n\nLong body", "committer": {"date": "2026-10-27T08:00:00Z"}}
        }])))
        .route("/repos/{owner}/{repo}/milestones", route(|name| json!([{
            "title": format!("{name} 1.0"), "open_issues": 7, "closed_issues": 21, "due_on": null,
            "html_url": format!("https://github.com/{name}/milestone/1")
        }])));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}")
}

async fn send(app: &Router, method: &str, uri: &str, cookie: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("cookie", cookie)
        .header("origin", ORIGIN)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn upstream_status_is_cached_rate_limits_keep_last_data_and_eggs_are_labelled() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("app.sqlite");
    let mut auth = AuthStore::open(&database).unwrap();
    auth.bootstrap_admin("admin", "a long test password")
        .unwrap();
    let cookie = format!(
        "__Host-gm_session={}",
        auth.login("admin", "a long test password", now()).unwrap()
    );
    let limited = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let github = spawn_github(limited.clone(), calls.clone()).await;
    let app = router_with_services(
        database.clone(),
        ORIGIN.into(),
        None,
        "http://127.0.0.1:9",
        &github,
        &dir.path().join("backups"),
    );
    assert_eq!(
        send(&app, "GET", "/api/experimental", "").await.0,
        StatusCode::UNAUTHORIZED
    );

    // The first read answers at once with unknown status and starts a background refresh.
    let (code, first) = send(&app, "GET", "/api/experimental", &cookie).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(first.as_array().unwrap().len(), 2);
    assert!(first[0]["upstream"].is_null() && first[0]["stale"] == true);
    let mut entries = Value::Null;
    for _ in 0..100 {
        entries = send(&app, "GET", "/api/experimental", &cookie).await.1;
        if entries
            .as_array()
            .unwrap()
            .iter()
            .all(|e| !e["upstream"].is_null())
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    let pumpkin = &entries[0];
    assert_eq!(pumpkin["key"], "pumpkin");
    assert_eq!(pumpkin["stale"], false);
    assert_eq!(pumpkin["upstream"]["releases"][0]["tag"], "nightly");
    assert_eq!(
        pumpkin["upstream"]["commits"][0]["message"],
        "Add chunk saving"
    );
    assert_eq!(pumpkin["upstream"]["commits"][0]["sha"], "0123456789ab");
    assert_eq!(pumpkin["upstream"]["milestones"][0]["closed_issues"], 21);
    assert!(pumpkin["upstream"].get("stargazers_count").is_none());
    let fetched = calls.load(Ordering::SeqCst);
    assert_eq!(fetched, 8, "four requests per project");

    // Reads within the hour use the cache; manual refreshes are spaced.
    send(&app, "GET", "/api/experimental", &cookie).await;
    let (code, body) = send(&app, "POST", "/api/experimental/refresh", &cookie).await;
    assert_eq!(
        (code, body["code"].as_str()),
        (StatusCode::TOO_MANY_REQUESTS, Some("refresh_limited"))
    );
    assert_eq!(calls.load(Ordering::SeqCst), fetched);

    // An hour later GitHub rate-limits: the last good data stays, marked stale with the reason.
    rusqlite::Connection::open(&database)
        .unwrap()
        .execute(
            "UPDATE experimental_status SET attempted_at = attempted_at - 4000, fetched_at = fetched_at - 4000",
            [],
        )
        .unwrap();
    limited.store(true, Ordering::SeqCst);
    let (code, entries) = send(&app, "POST", "/api/experimental/refresh", &cookie).await;
    assert_eq!(code, StatusCode::OK);
    for entry in entries.as_array().unwrap() {
        assert_eq!(entry["stale"], true);
        assert!(
            entry["error"].as_str().unwrap().contains("rate limit"),
            "{entry}"
        );
        assert_eq!(entry["upstream"]["releases"][0]["tag"], "nightly");
    }

    // The bundled eggs join the library once, labelled experimental.
    let (code, egg) = send(&app, "POST", "/api/experimental/steelmc/egg", &cookie).await;
    assert_eq!(code, StatusCode::CREATED, "{egg}");
    assert_eq!(
        (egg["name"].as_str(), egg["experimental"].as_bool()),
        (Some("SteelMC (experimental)"), Some(true))
    );
    assert_eq!(
        send(&app, "POST", "/api/experimental/steelmc/egg", &cookie)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(&app, "POST", "/api/experimental/unknown/egg", &cookie)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let eggs = send(&app, "GET", "/api/eggs", &cookie).await.1;
    assert_eq!(eggs[0]["experimental"], true);
    let entries = send(&app, "GET", "/api/experimental", &cookie).await.1;
    assert_eq!(entries[1]["egg_id"], egg["id"]);
}
