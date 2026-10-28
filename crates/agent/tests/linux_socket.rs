#![cfg(target_os = "linux")]

use manager_agent::linux::{bind_private, serve_connection};
use manager_protocol::{AgentRequest, AgentResponse, AgentResult, Operation, VERSION};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};
use uuid::Uuid;

#[tokio::test]
async fn actual_linux_peer_credentials_gate_requests() {
    let (server, mut client) = UnixStream::pair().unwrap();
    let uid = server.peer_cred().unwrap().uid();
    let config = std::sync::Arc::new(manager_agent::runtime::Runtime::new(
        manager_agent::host::AgentConfig::default(),
    ));
    let task = tokio::spawn(serve_connection(server, uid, config.clone()));
    let request = AgentRequest {
        version: VERSION,
        request_id: Uuid::new_v4(),
        operation: Operation::Ping,
    };
    let bytes = serde_json::to_vec(&request).unwrap();
    client.write_u32(bytes.len() as u32).await.unwrap();
    client.write_all(&bytes).await.unwrap();
    let length = client.read_u32().await.unwrap();
    let mut response = vec![0; length as usize];
    client.read_exact(&mut response).await.unwrap();
    let response: AgentResponse = serde_json::from_slice(&response).unwrap();
    assert_eq!(response.result, AgentResult::Pong);
    assert_eq!(response.request_id, request.request_id);
    task.await.unwrap().unwrap();
    let (server, _client) = UnixStream::pair().unwrap();
    assert_eq!(
        serve_connection(server, uid + 1, config)
            .await
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
}

#[tokio::test]
async fn listener_does_not_replace_existing_paths_or_use_a_writable_parent() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent.sock");
    std::fs::write(&path, "preserve me").unwrap();
    assert!(bind_private(&path).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "preserve me");
    std::fs::remove_file(&path).unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(bind_private(&path).is_err());
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let _listener = bind_private(&path).unwrap();
    let metadata = std::fs::symlink_metadata(&path).unwrap();
    assert!(metadata.file_type().is_socket());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o660);
}

#[test]
fn the_api_users_group_can_reach_the_socket_and_nobody_else() {
    use std::os::unix::fs::MetadataExt;
    if std::fs::metadata("/proc/self").unwrap().uid() != 0 {
        eprintln!("skipped: changing group ownership needs root");
        return;
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let dir = tempfile::tempdir().unwrap();
    let socket_dir = dir.path().join("run");
    std::fs::create_dir(&socket_dir).unwrap();
    std::fs::set_permissions(&socket_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = socket_dir.join("agent.sock");
    let _listener = manager_agent::linux::bind_private(&path).unwrap();
    // uid 65534 is "nobody" on Debian and Ubuntu, whose primary group is nogroup (65534).
    manager_agent::linux::grant_group(&path, 65534).unwrap();
    let socket = std::fs::metadata(&path).unwrap();
    let parent = std::fs::metadata(&socket_dir).unwrap();
    assert_eq!((socket.gid(), socket.mode() & 0o777), (65534, 0o660));
    assert_eq!((parent.gid(), parent.mode() & 0o777), (65534, 0o750));
    assert!(manager_agent::linux::grant_group(&path, 4_000_000_000).is_err());
}
