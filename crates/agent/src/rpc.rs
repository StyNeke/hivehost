use manager_protocol::{AgentRequest, AgentResponse, AgentResult, MAX_FRAME_BYTES, VERSION};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt};

pub fn authorize_peer(actual: u32, expected: u32) -> io::Result<()> {
    if actual != expected {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unrecognized peer uid",
        ));
    }
    Ok(())
}

pub async fn read_request(mut reader: impl AsyncRead + Unpin) -> io::Result<AgentRequest> {
    let length = reader.read_u32().await? as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid frame length",
        ));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid request"))
}

pub async fn dispatch(request: AgentRequest, config: &crate::host::AgentConfig) -> AgentResponse {
    let result = if request.version != VERSION {
        AgentResult::UnsupportedVersion
    } else {
        match request.operation {
            manager_protocol::Operation::Ping => AgentResult::Pong,
            manager_protocol::Operation::InspectHost => {
                match crate::host::inspect_host(config).await {
                    Ok(status) => AgentResult::HostStatus(Box::new(status)),
                    Err(_) => AgentResult::Unavailable,
                }
            }
        }
    };
    AgentResponse {
        version: VERSION,
        request_id: request.request_id,
        result,
    }
}
