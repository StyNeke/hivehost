#![cfg(target_os = "linux")]
// Real Docker lifecycle acceptance with a synthetic game. Run explicitly as root with Docker:
// cargo test -p manager-agent --test runtime_docker -- --ignored --test-threads=1

use manager_agent::{
    host::AgentConfig,
    runtime::{Runtime, check_port_free, install_name, runtime_name},
};
use manager_core::{
    capacity::Resources,
    eggs::{FileTransform, Installer},
    host::{PortBinding, Transport},
};
use manager_protocol::{
    AgentResult, ExtensionFolder, OperationState, PowerState, RuntimeStatus, ServerSpec,
};
use std::{collections::BTreeMap, process::Command, sync::Arc, time::Duration};
use uuid::Uuid;

const IMAGE: &str = "ghcr.io/ptero-eggs/yolks:java_21";
const GAME: &str = r#"#!/bin/bash
cat > /mnt/server/game.sh <<'GAME'
echo "booting"
grep server-port server.properties
cat eula.txt
id -u
echo "Done (0.1s)! For help, type help"
while read -r line; do
  case "$line" in
    stop) echo "Stopping"; exit 0 ;;
    crash) echo "crashing"; exit 3 ;;
    hang) trap '' INT TERM; while true; do sleep 1; done ;;
    *) echo "cmd: $line" ;;
  esac
done
GAME
echo "installed"
"#;

fn spec(port: u16) -> ServerSpec {
    ServerSpec {
        server_id: Uuid::new_v4(),
        image: IMAGE.into(),
        startup: "bash game.sh".into(),
        stop: "stop".into(),
        ready_when: ")! For help, type ".into(),
        installer: Installer {
            script: GAME.into(),
            container: IMAGE.into(),
            entrypoint: "bash".into(),
        },
        variables: BTreeMap::new(),
        limits: Resources {
            memory_bytes: 512 * 1024 * 1024,
            disk_bytes: 128 * 1024 * 1024,
            cpu_millis: 500,
        },
        binding: PortBinding {
            ip: "127.0.0.1".parse().unwrap(),
            port,
            transport: Transport::Tcp,
        },
        files: [(
            "server.properties".to_string(),
            FileTransform {
                parser: "properties".into(),
                find: [(
                    "server-port".to_string(),
                    "{{server.build.default.port}}".to_string(),
                )]
                .into(),
            },
        )]
        .into(),
        accept_eula: true,
    }
}

// Removes containers, the mount and the network even when an assertion fails mid-test.
struct Cleanup {
    id: Uuid,
    volume: std::path::PathBuf,
    network: String,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        for name in [runtime_name(self.id), install_name(self.id)] {
            let _ = Command::new("docker").args(["rm", "-f", &name]).output();
        }
        let _ = Command::new("umount").arg(&self.volume).output();
        let _ = Command::new("docker")
            .args(["network", "rm", &self.network])
            .output();
    }
}

async fn status(runtime: &Arc<Runtime>, id: Uuid) -> RuntimeStatus {
    match runtime.status(id).await {
        AgentResult::ServerStatus(status) => *status,
        other => panic!("unexpected status reply: {other:?}"),
    }
}

async fn wait_for(
    runtime: &Arc<Runtime>,
    id: Uuid,
    seconds: u64,
    done: impl Fn(&RuntimeStatus) -> bool,
) -> RuntimeStatus {
    for _ in 0..seconds * 4 {
        let current = status(runtime, id).await;
        if done(&current) {
            return current;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("timed out; last status: {:?}", status(runtime, id).await);
}

async fn console(runtime: &Arc<Runtime>, id: Uuid) -> String {
    match runtime.console(id, 200).await {
        AgentResult::Console(lines) => lines.join("\n"),
        other => panic!("unexpected console reply: {other:?}"),
    }
}

fn rejected_code(result: AgentResult) -> String {
    match result {
        AgentResult::Rejected(rejection) => rejection.code,
        other => panic!("expected rejection, got {other:?}"),
    }
}

#[tokio::test]
#[ignore = "requires root, Docker and network access to pull the yolks image"]
async fn a_server_installs_runs_restarts_stops_and_is_deleted_cleanly() {
    let data = tempfile::tempdir().unwrap();
    let network = format!("gm-test-{}", &Uuid::new_v4().simple().to_string()[..8]);
    let config = AgentConfig {
        data_root: data.path().to_path_buf(),
        uid_base: 300_000,
        stop_grace_secs: 5,
        network_name: network.clone(),
        network_subnet: "172.31.251.0/24".into(),
        ..Default::default()
    };
    let runtime = Arc::new(Runtime::new(config));
    runtime.load().await.unwrap();
    let port = 25690;
    let spec = spec(port);
    let id = spec.server_id;
    let _cleanup = Cleanup {
        id,
        volume: data.path().join("volumes").join(id.to_string()),
        network: network.clone(),
    };

    assert_eq!(runtime.install(spec.clone()).await, AgentResult::Accepted);
    assert_eq!(rejected_code(runtime.install(spec.clone()).await), "busy");
    let installed = wait_for(&runtime, id, 180, |s| {
        s.operation
            .as_ref()
            .is_some_and(|op| op.state != OperationState::Running)
    })
    .await;
    assert_eq!(
        installed.operation.unwrap().state,
        OperationState::Succeeded
    );
    assert!(installed.installed);
    assert!(installed.disk_total_bytes.unwrap() <= 128 * 1024 * 1024);
    assert_eq!(
        rejected_code(runtime.install(spec.clone()).await),
        "already_installed"
    );
    assert_eq!(
        rejected_code(runtime.send_command(id, "hello").await),
        "not_running"
    );

    // Another process on the port blocks the start cleanly.
    let squatter = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    assert!(check_port_free(&spec.binding).is_err());
    assert_eq!(
        rejected_code(runtime.start(spec.clone()).await),
        "port_in_use"
    );
    drop(squatter);

    assert_eq!(runtime.start(spec.clone()).await, AgentResult::Accepted);
    let running = wait_for(&runtime, id, 60, |s| s.power == PowerState::Running).await;
    assert!(running.desired_running && running.started_at.is_some());
    let output = console(&runtime, id).await;
    assert!(output.contains(&format!("server-port={port}")), "{output}");
    assert!(output.contains("eula=true"), "{output}");
    assert!(
        output.contains("300000"),
        "runs as the server's own UID: {output}"
    );
    assert_eq!(
        runtime.start(spec.clone()).await,
        AgentResult::Accepted,
        "start is idempotent"
    );

    assert_eq!(
        runtime.send_command(id, "hello world").await,
        AgentResult::Accepted
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(console(&runtime, id).await.contains("cmd: hello world"));
    assert_eq!(
        rejected_code(runtime.send_command(id, "say a\nstop").await),
        "invalid_command"
    );
    assert_eq!(
        rejected_code(
            runtime
                .install_files(id, ExtensionFolder::Mods, vec![], vec!["old.jar".into()])
                .await
        ),
        "running"
    );

    // An unexpected exit is restarted after the backoff delay.
    runtime.send_command(id, "crash").await;
    wait_for(&runtime, id, 20, |s| s.power == PowerState::Offline).await;
    runtime.supervise_once().await;
    let crashed = status(&runtime, id).await;
    assert!(
        crashed
            .problem
            .as_deref()
            .unwrap()
            .contains("restarting in 10 seconds"),
        "{crashed:?}"
    );
    runtime.supervise_once().await;
    assert_eq!(
        status(&runtime, id).await.power,
        PowerState::Offline,
        "waits for the backoff"
    );
    tokio::time::sleep(Duration::from_secs(11)).await;
    runtime.supervise_once().await;
    wait_for(&runtime, id, 60, |s| s.power == PowerState::Running).await;

    // A requested stop is graceful and never triggers the crash policy.
    assert_eq!(runtime.stop(id).await, AgentResult::Accepted);
    let stopped = wait_for(&runtime, id, 30, |s| s.power == PowerState::Offline).await;
    assert!(!stopped.desired_running);
    assert_eq!(stopped.exit_code, Some(0));
    runtime.supervise_once().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(status(&runtime, id).await.power, PowerState::Offline);
    assert!(console(&runtime, id).await.contains("Stopping"));

    // A hung server is force-stopped after the grace period and the reason is recorded.
    assert_eq!(runtime.start(spec.clone()).await, AgentResult::Accepted);
    wait_for(&runtime, id, 60, |s| s.power == PowerState::Running).await;
    runtime.send_command(id, "hang").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(runtime.stop(id).await, AgentResult::Accepted);
    assert_eq!(status(&runtime, id).await.power, PowerState::Stopping);
    let forced = wait_for(&runtime, id, 30, |s| {
        s.power == PowerState::Offline && s.problem.is_some()
    })
    .await;
    assert!(forced.problem.unwrap().contains("force-stopped"));

    // The registry survives an agent restart.
    let reloaded = Arc::new(Runtime::new(runtime.config.clone()));
    reloaded.load().await.unwrap();
    assert!(status(&reloaded, id).await.installed);

    assert_eq!(runtime.delete(id).await, AgentResult::Accepted);
    for _ in 0..120 {
        if matches!(runtime.status(id).await, AgentResult::Rejected(_)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(rejected_code(runtime.status(id).await), "not_found");
    for name in [runtime_name(id), install_name(id)] {
        let exists = Command::new("docker")
            .args(["inspect", &name])
            .output()
            .unwrap()
            .status
            .success();
        assert!(!exists, "{name} should be removed");
    }
    assert!(!data.path().join("volumes").join(id.to_string()).exists());
    assert!(
        !data
            .path()
            .join("images")
            .join(format!("{id}.img"))
            .exists()
    );
    assert!(
        !data
            .path()
            .join("registry")
            .join(format!("{id}.json"))
            .exists()
    );
}
