use manager_agent::rpc::{authorize_peer, dispatch, read_request};
use manager_protocol::{AgentRequest, AgentResult, MAX_FRAME_BYTES, Operation, VERSION};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

#[test]
fn only_the_configured_api_uid_is_authorized() {
    assert!(authorize_peer(1001, 1001).is_ok());
    assert!(authorize_peer(1002, 1001).is_err());
    assert!(authorize_peer(0, 1001).is_err());
}

#[tokio::test]
async fn valid_ping_roundtrips_and_unknown_protocol_fails_explicitly() {
    let request = AgentRequest {
        version: VERSION,
        request_id: Uuid::new_v4(),
        operation: Operation::Ping,
    };
    let id = request.request_id;
    let encoded = serde_json::to_vec(&request).unwrap();
    let (mut writer, reader) = tokio::io::duplex(4096);
    writer.write_u32(encoded.len() as u32).await.unwrap();
    writer.write_all(&encoded).await.unwrap();
    let response = dispatch(
        read_request(reader).await.unwrap(),
        &std::sync::Arc::new(manager_agent::runtime::Runtime::new(
            manager_agent::host::AgentConfig::default(),
        )),
    )
    .await;
    assert_eq!(response.request_id, id);
    assert_eq!(response.result, AgentResult::Pong);
    let request = AgentRequest {
        version: VERSION + 1,
        request_id: id,
        operation: Operation::InspectHost,
    };
    assert_eq!(
        dispatch(
            request,
            &std::sync::Arc::new(manager_agent::runtime::Runtime::new(
                manager_agent::host::AgentConfig::default(),
            )),
        )
        .await
        .result,
        AgentResult::UnsupportedVersion
    );
}

#[tokio::test]
async fn frames_are_bounded_and_unknown_operations_are_not_accepted() {
    for length in [0, MAX_FRAME_BYTES as u32 + 1, u32::MAX] {
        let (mut writer, reader) = tokio::io::duplex(16);
        writer.write_u32(length).await.unwrap();
        assert_eq!(
            read_request(reader).await.unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }
    let invalid = format!(
        r#"{{"version":1,"request_id":"{}","operation":{{"type":"exec","command":"host-command"}}}}"#,
        Uuid::new_v4()
    );
    let (mut writer, reader) = tokio::io::duplex(4096);
    writer.write_u32(invalid.len() as u32).await.unwrap();
    writer.write_all(invalid.as_bytes()).await.unwrap();
    assert_eq!(
        read_request(reader).await.unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
}
