use manager_api::{
    agent_client::AgentClient,
    auth::AuthStore,
    config::validate_listener,
    http::{MODRINTH_API, router_with_services},
    jobs::JobStore,
};
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
    // The agent's UID is required so a process that merely creates the socket path is not trusted.
    let agent = match env::var("GM_AGENT_SOCKET") {
        Ok(socket) => {
            let uid: u32 = env::var("GM_AGENT_UID")
                .map_err(|_| "GM_AGENT_UID must identify the host agent service user")?
                .parse()?;
            Some(AgentClient::new(&PathBuf::from(socket), uid))
        }
        Err(_) => {
            println!(
                "GM_AGENT_SOCKET is not set; host-dependent routes will report the host as unavailable"
            );
            None
        }
    };
    let modrinth = env::var("GM_MODRINTH_API").unwrap_or_else(|_| MODRINTH_API.into());
    if !modrinth.starts_with("https://") {
        return Err("GM_MODRINTH_API must be an https:// URL".into());
    }
    // Installs and deletions are durable jobs; the worker resumes them after a restart.
    if let Some(agent) = agent.clone() {
        tokio::spawn(manager_api::schedules::run(database.clone(), agent.clone()));
        tokio::spawn(manager_api::worker::run(database.clone(), agent));
    }
    let backup_root = PathBuf::from(
        env::var("GM_BACKUP_ROOT").unwrap_or_else(|_| manager_api::http::BACKUP_ROOT.into()),
    );
    let mut app = router_with_services(database, origin.clone(), agent, &modrinth, &backup_root);
    if let Ok(directory) = env::var("GM_STATIC_DIR") {
        let directory = PathBuf::from(directory);
        if !directory.join("index.html").is_file() {
            return Err("GM_STATIC_DIR must contain the built interface (index.html)".into());
        }
        app = manager_api::http::with_static_files(app, &directory);
    }
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("API listening on {address}; expected private HTTPS origin: {origin}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            // systemd stops services with SIGTERM; interactive runs use Ctrl+C.
            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};
                match signal(SignalKind::terminate()) {
                    Ok(mut terminate) => {
                        tokio::select! {
                            _ = terminate.recv() => {}
                            _ = tokio::signal::ctrl_c() => {}
                        }
                    }
                    Err(_) => {
                        let _ = tokio::signal::ctrl_c().await;
                    }
                }
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
