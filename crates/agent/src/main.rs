#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use manager_agent::linux::{bind_private, serve_connection};
    use std::{env, os::unix::fs::MetadataExt, path::PathBuf, sync::Arc};
    use tokio::sync::Semaphore;
    let config = Arc::new(manager_agent::host::AgentConfig {
        data_root: env::var("GM_DATA_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/var/lib/game-manager")),
        private_ips: env::var("GM_PRIVATE_IPS")
            .unwrap_or_default()
            .split(',')
            .filter(|ip| !ip.trim().is_empty())
            .map(|ip| ip.trim().parse())
            .collect::<Result<_, _>>()?,
    });
    let arguments: Vec<_> = env::args().skip(1).collect();
    if arguments == ["inspect-host"] {
        println!(
            "{}",
            serde_json::to_string(&manager_agent::host::inspect_host(&config).await?)?
        );
        return Ok(());
    }
    if !arguments.is_empty() {
        return Err("usage: manager-agent [inspect-host]".into());
    }

    let path = PathBuf::from(
        env::var("GM_AGENT_SOCKET").unwrap_or_else(|_| "/run/game-manager/agent.sock".into()),
    );
    let api_uid: u32 = env::var("GM_API_UID")
        .map_err(|_| "GM_API_UID must identify the authorized API service user")?
        .parse()?;
    let listener = bind_private(&path)?;
    let original = std::fs::symlink_metadata(&path)?;
    let slots = Arc::new(Semaphore::new(16));
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
                let config = config.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(error) = serve_connection(stream, api_uid, config).await {
                        eprintln!("agent request rejected: {:?}", error.kind());
                    }
                });
            }
        }
    }
    drop(listener);
    if let Ok(current) = std::fs::symlink_metadata(&path) {
        if current.ino() == original.ino() && current.dev() == original.dev() {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("The host agent requires Linux. Protocol/domain tests can run on this platform.");
    std::process::exit(1);
}
