use manager_api::{auth::AuthStore, config::validate_listener, http::router, jobs::JobStore};
use std::{env, path::PathBuf};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database =
        PathBuf::from(env::var("GM_DATABASE").unwrap_or_else(|_| "game-manager.sqlite".into()));
    let arguments: Vec<_> = env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some("init-admin") && arguments.len() == 2 {
        let password = rpassword::prompt_password("New panel password (at least 12 bytes): ")?;
        let repeated = rpassword::prompt_password("Repeat password: ")?;
        if password != repeated {
            return Err("passwords did not match".into());
        }
        AuthStore::open(&database)?.bootstrap_admin(&arguments[1], &password)?;
        println!("Administrator initialized. No default credentials were created.");
        return Ok(());
    }
    if !arguments.is_empty() {
        return Err("usage: manager-api [init-admin USERNAME]".into());
    }
    let origin = env::var("GM_ORIGIN")
        .map_err(|_| "GM_ORIGIN is required, for example https://panel.example.test:8443")?;
    let bind = env::var("GM_BIND").unwrap_or_else(|_| "127.0.0.1:19090".into());
    let address = validate_listener(&bind, &origin)?;
    AuthStore::open(&database)?;
    JobStore::open(&database)?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("API listening on {address}; expected private HTTPS origin: {origin}");
    axum::serve(listener, router(database, origin))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
