use manager_api::agent_client::decode_host_status;
use manager_core::{
    capacity::Resources,
    host::{HostAdmission, HostHardware, HostStatus},
};
use manager_protocol::{AgentResponse, AgentResult, MAX_FRAME_BYTES, VERSION};
use uuid::Uuid;

fn status() -> HostStatus {
    let resources = Resources {
        memory_bytes: 1024,
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
            measured_at: 100,
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

#[test]
fn reply_must_match_request_protocol_and_expected_operation() {
    let id = Uuid::new_v4();
    let mut response = AgentResponse {
        version: VERSION,
        request_id: id,
        result: AgentResult::HostStatus(Box::new(status())),
    };
    let encoded = serde_json::to_vec(&response).unwrap();
    assert_eq!(decode_host_status(id, &encoded).unwrap(), status());
    assert!(decode_host_status(Uuid::new_v4(), &encoded).is_err());
    response.version += 1;
    assert!(decode_host_status(id, &serde_json::to_vec(&response).unwrap()).is_err());
    response.version = VERSION;
    response.result = AgentResult::Pong;
    assert!(decode_host_status(id, &serde_json::to_vec(&response).unwrap()).is_err());
    response.result = AgentResult::Unavailable;
    assert!(decode_host_status(id, &serde_json::to_vec(&response).unwrap()).is_err());
    assert!(decode_host_status(id, &vec![b' '; MAX_FRAME_BYTES + 1]).is_err());
    assert!(decode_host_status(id, b"invalid JSON").is_err());
}
