use manager_core::host::HostStatus;
use manager_protocol::{AgentResponse, AgentResult, MAX_FRAME_BYTES, VERSION};
use std::io;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Clone)]
pub struct AgentClient {
    socket: PathBuf,
    expected_uid: u32,
}

impl AgentClient {
    pub fn new(socket: &Path, expected_uid: u32) -> Self {
        Self {
            socket: socket.to_owned(),
            expected_uid,
        }
    }
    pub async fn inspect_host(&self) -> io::Result<HostStatus> {
        let _ = (&self.socket, self.expected_uid);
        todo!()
    }
}

pub fn decode_host_status(expected_request: Uuid, bytes: &[u8]) -> io::Result<HostStatus> {
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid agent response length",
        ));
    }
    let response: AgentResponse = serde_json::from_slice(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid agent response"))?;
    if response.version != VERSION || response.request_id != expected_request {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "agent response does not match request",
        ));
    }
    match response.result {
        AgentResult::HostStatus(status) => Ok(*status),
        AgentResult::Unavailable => Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "host inspection unavailable",
        )),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected agent response",
        )),
    }
}
