use manager_core::host::HostStatus;
use manager_protocol::{
    AgentRequest, AgentResponse, AgentResult, MAX_FRAME_BYTES, Operation, VERSION,
};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

// The agent allows 5s to read, 25s to inspect and 5s to reply.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(36);

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
        let request_id = Uuid::new_v4();
        let reply = tokio::time::timeout(
            EXCHANGE_TIMEOUT,
            self.exchange(&AgentRequest {
                version: VERSION,
                request_id,
                operation: Operation::InspectHost,
            }),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "agent did not reply in time"))??;
        let status = decode_host_status(request_id, &reply)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
            .ok_or_else(|| io::Error::other("system clock is before the Unix epoch"))?;
        if !status.admission.is_fresh(now) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "host snapshot is stale or from the future",
            ));
        }
        Ok(status)
    }

    #[cfg(unix)]
    async fn exchange(&self, request: &AgentRequest) -> io::Result<Vec<u8>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::UnixStream::connect(&self.socket).await?;
        // Only trust host state from the agent's service account, not whatever owns the path.
        if stream.peer_cred()?.uid() != self.expected_uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "agent socket peer has an unexpected uid",
            ));
        }
        let encoded = serde_json::to_vec(request).map_err(io::Error::other)?;
        stream.write_u32(encoded.len() as u32).await?;
        stream.write_all(&encoded).await?;
        let length = stream.read_u32().await? as usize;
        if length == 0 || length > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid agent response length",
            ));
        }
        let mut reply = vec![0; length];
        stream.read_exact(&mut reply).await?;
        Ok(reply)
    }

    #[cfg(not(unix))]
    async fn exchange(&self, _request: &AgentRequest) -> io::Result<Vec<u8>> {
        let _ = (&self.socket, self.expected_uid);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the host agent requires Linux",
        ))
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
