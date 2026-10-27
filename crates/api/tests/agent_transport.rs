#![cfg(target_os = "linux")]

use manager_api::agent_client::AgentClient;
use manager_core::{
    capacity::Resources,
    host::{HostAdmission, HostHardware, HostStatus},
};
use manager_protocol::{AgentRequest, AgentResponse, AgentResult, Operation, VERSION};
use std::{
    io::ErrorKind,
    os::unix::fs::MetadataExt,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
};

fn sample_status(age: i64) -> HostStatus {
    let measured_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        - age;
    let resources = Resources {
        memory_bytes: 1000,
        disk_bytes: 0,
        cpu_millis: 1000,
    };
    HostStatus {
        hardware: HostHardware {
            cpu_millis: 6000,
            memory_bytes: 24000,
            disk_total_bytes: 100000,
            disk_available_bytes: 70000,
        },
        admission: HostAdmission {
            measured_at,
            budget: resources,
            legacy_reserved: resources,
            disk_quotas_ready: false,
            qualified_revisions: vec![],
            private_ips: vec![],
            unavailable_bindings: vec![],
        },
        blockers: vec!["quota setup required".into()],
    }
}

#[tokio::test]
async fn client_checks_real_peer_uid_and_response_before_trusting_host_state() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let uid = std::fs::metadata("/proc/self").unwrap().uid();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let length = stream.read_u32().await.unwrap();
        let mut bytes = vec![0; length as usize];
        stream.read_exact(&mut bytes).await.unwrap();
        let request: AgentRequest = serde_json::from_slice(&bytes).unwrap();
        assert!(matches!(request.operation, Operation::InspectHost));
        let bytes = serde_json::to_vec(&AgentResponse {
            version: VERSION,
            request_id: request.request_id,
            result: AgentResult::HostStatus(Box::new(sample_status(0))),
        })
        .unwrap();
        stream.write_u32(bytes.len() as u32).await.unwrap();
        stream.write_all(&bytes).await.unwrap();
    });
    assert_eq!(
        AgentClient::new(&path, uid)
            .inspect_host()
            .await
            .unwrap()
            .hardware
            .cpu_millis,
        6000
    );
    server.await.unwrap();
}

#[tokio::test]
async fn a_socket_owned_by_an_unexpected_uid_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let uid = std::fs::metadata("/proc/self").unwrap().uid();
    let server = tokio::spawn(async move {
        listener.accept().await.unwrap();
    });
    assert_eq!(
        AgentClient::new(&path, uid + 1)
            .inspect_host()
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::PermissionDenied
    );
    server.await.unwrap();
}

#[tokio::test]
async fn oversized_or_stale_host_replies_fail_closed() {
    for age in [None, Some(60), Some(-60)] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let uid = std::fs::metadata("/proc/self").unwrap().uid();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let length = stream.read_u32().await.unwrap();
            let mut bytes = vec![0; length as usize];
            stream.read_exact(&mut bytes).await.unwrap();
            let request: AgentRequest = serde_json::from_slice(&bytes).unwrap();
            if let Some(age) = age {
                let bytes = serde_json::to_vec(&AgentResponse {
                    version: VERSION,
                    request_id: request.request_id,
                    result: AgentResult::HostStatus(Box::new(sample_status(age))),
                })
                .unwrap();
                stream.write_u32(bytes.len() as u32).await.unwrap();
                stream.write_all(&bytes).await.unwrap();
            } else {
                stream.write_u32(u32::MAX).await.unwrap();
            }
        });
        assert_eq!(
            AgentClient::new(&path, uid)
                .inspect_host()
                .await
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidData
        );
        server.await.unwrap();
    }
}
