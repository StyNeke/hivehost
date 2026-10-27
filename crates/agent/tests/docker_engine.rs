#![cfg(target_os = "linux")]

use manager_agent::engine::{console_lines, logs, write_stdin};
use std::{path::Path, process::Command, time::Duration};

const IMAGE: &str = "ghcr.io/ptero-eggs/yolks:java_21";

// Removes the test container even when an assertion fails.
struct Cleanup(String);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = Command::new("docker").args(["rm", "-f", &self.0]).output();
    }
}

#[tokio::test]
#[ignore = "requires a local Docker daemon and the yolks java_21 image"]
async fn stdin_commands_reach_a_running_container_repeatedly_and_logs_stay_ordered() {
    let socket = Path::new("/var/run/docker.sock");
    let name = format!("gm-enginetest-{}", uuid::Uuid::new_v4().simple());
    let created = Command::new("docker")
        .args([
            "run",
            "-d",
            "-i",
            "--name",
            &name,
            "--entrypoint",
            "sh",
            IMAGE,
            "-c",
        ])
        .arg("echo started; while read line; do echo \"got $line\"; echo \"err $line\" >&2; done")
        .status()
        .unwrap();
    let _cleanup = Cleanup(name.clone());
    assert!(created.success());
    for command in ["first", "second"] {
        write_stdin(socket, &name, format!("{command}\n").as_bytes())
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let lines = console_lines(&logs(socket, &name, "tail=10", 65536).await.unwrap());
    // stdout and stderr are separate pipes, so only order within each stream is guaranteed.
    let stdout: Vec<_> = lines.iter().filter(|l| !l.starts_with("err")).collect();
    let stderr: Vec<_> = lines.iter().filter(|l| l.starts_with("err")).collect();
    assert_eq!(stdout, ["started", "got first", "got second"]);
    assert_eq!(stderr, ["err first", "err second"]);
    let running = Command::new("docker")
        .args(["inspect", "-f", "{{.State.Running}}", &name])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&running.stdout).trim(), "true");
    assert_eq!(
        logs(socket, "gm-does-not-exist", "tail=1", 1024)
            .await
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(logs(socket, &name, "tail=10", 8).await.is_err());
}
