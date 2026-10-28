#[cfg(target_os = "linux")]
fn config_from_env() -> Result<manager_agent::host::AgentConfig, Box<dyn std::error::Error>> {
    use std::{env, path::PathBuf};
    let mut config = manager_agent::host::AgentConfig {
        data_root: env::var("GM_DATA_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/var/lib/game-manager")),
        private_ips: env::var("GM_PRIVATE_IPS")
            .unwrap_or_default()
            .split(',')
            .filter(|ip| !ip.trim().is_empty())
            .map(|ip| ip.trim().parse())
            .collect::<Result<_, _>>()?,
        ..Default::default()
    };
    if !config.data_root.is_absolute() {
        return Err("GM_DATA_ROOT must be an absolute path".into());
    }
    if let Ok(proxy) = env::var("GM_CONTAINER_PROXY") {
        config.container_proxy = Some(manager_agent::host::parse_proxy(&proxy)?);
    }
    for (variable, slot) in [
        ("GM_CONTAINER_CA_FILE", &mut config.container_ca_file),
        (
            "GM_CONTAINER_JAVA_TRUSTSTORE",
            &mut config.container_java_truststore,
        ),
    ] {
        if let Ok(path) = env::var(variable) {
            let path = PathBuf::from(path);
            // Commas would split the --mount specification.
            if !path.is_absolute()
                || !path.is_file()
                || path.to_str().is_none_or(|p| p.contains(','))
            {
                return Err(
                    format!("{variable} must be an absolute path to an existing file").into(),
                );
            }
            *slot = Some(path);
        }
    }
    if let Ok(base) = env::var("GM_GAME_UID_BASE") {
        config.uid_base = base.parse()?;
        if config.uid_base < 10_000 {
            return Err(
                "GM_GAME_UID_BASE must be at least 10000 to stay clear of real accounts".into(),
            );
        }
    }
    if let Ok(reserve) = env::var("GM_DISK_RESERVE_BYTES") {
        config.disk_reserve_bytes = reserve.parse()?;
    }
    if let Ok(grace) = env::var("GM_STOP_GRACE_SECS") {
        config.stop_grace_secs = grace.parse()?;
        if !(5..=3600).contains(&config.stop_grace_secs) {
            return Err("GM_STOP_GRACE_SECS must be between 5 and 3600".into());
        }
    }
    if let Ok(root) = env::var("GM_BACKUP_ROOT") {
        config.backup_root = PathBuf::from(root);
        if !config.backup_root.is_absolute() {
            return Err("GM_BACKUP_ROOT must be an absolute path".into());
        }
    }
    if let Ok(subnet) = env::var("GM_NETWORK_SUBNET") {
        if manager_agent::firewall::default_gateway(&subnet).is_none() {
            return Err("GM_NETWORK_SUBNET must be an IPv4 subnet such as 172.30.250.0/24".into());
        }
        config.network_subnet = subnet;
    }
    match env::var("GM_NETWORK_ISOLATION").as_deref() {
        Err(_) | Ok("on") => {}
        Ok("off") => config.network_isolation = false,
        Ok(_) => return Err("GM_NETWORK_ISOLATION must be on or off".into()),
    }
    Ok(config)
}

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use manager_agent::{
        linux::{bind_private, serve_connection},
        runtime::Runtime,
    };
    use std::{env, os::unix::fs::MetadataExt, path::PathBuf, sync::Arc, time::Duration};
    use tokio::{
        signal::unix::{SignalKind, signal},
        sync::Semaphore,
    };
    let mut config = config_from_env()?;
    let arguments: Vec<_> = env::args().skip(1).collect();
    if arguments == ["inspect-host"] {
        println!(
            "{}",
            serde_json::to_string(
                &manager_agent::host::inspect_host(&config, &Default::default(), 0).await?
            )?
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
    let _instance = manager_agent::storage::lock_instance(&manager_agent::storage::Layout {
        data_root: config.data_root.clone(),
    })?;
    config.api_gid = Some(manager_agent::linux::primary_gid(api_uid)?);
    let runtime = Arc::new(Runtime::new(config));
    runtime.load().await?;
    runtime.ensure_network().await.map_err(|error| {
        format!(
            "could not prepare the game network and its isolation rules: {error} (GM_NETWORK_ISOLATION=off runs without the rules)"
        )
    })?;
    let listener = bind_private(&path)?;
    manager_agent::linux::grant_group(&path, api_uid)?;
    let original = std::fs::symlink_metadata(&path)?;
    {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime.restore().await;
            loop {
                runtime.supervise_once().await;
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        });
    }
    {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            loop {
                runtime.sample_usage().await;
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
    }
    let slots = Arc::new(Semaphore::new(16));
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    loop {
        tokio::select! {
            _ = terminate.recv() => break,
            _ = interrupt.recv() => break,
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
                let runtime = runtime.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(error) = serve_connection(stream, api_uid, runtime).await {
                        eprintln!("agent request rejected: {:?}", error.kind());
                    }
                });
            }
        }
    }
    drop(listener);
    if let Ok(current) = std::fs::symlink_metadata(&path)
        && current.ino() == original.ino()
        && current.dev() == original.dev()
    {
        std::fs::remove_file(&path)?;
    }
    eprintln!("stopping game servers before exit");
    runtime.shutdown().await;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("The host agent requires Linux. Protocol/domain tests can run on this platform.");
    std::process::exit(1);
}
