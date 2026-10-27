use crate::rpc::{authorize_peer, dispatch, read_request};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::{io, path::Path};
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};

pub fn bind_private(path: &Path) -> io::Result<UnixListener> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "socket parent required"))?;
    let metadata = std::fs::symlink_metadata(parent)?;
    let own_uid = std::fs::metadata("/proc/self")?.uid();
    if !metadata.is_dir() || metadata.uid() != own_uid || metadata.mode() & 0o022 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "socket parent must be owned by the agent and not writable by others",
        ));
    }
    // Never unlink an existing socket or file: it may belong to another live service.
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))?;
    Ok(listener)
}
pub async fn serve_connection(
    mut stream: UnixStream,
    expected_uid: u32,
    config: std::sync::Arc<crate::host::AgentConfig>,
) -> io::Result<()> {
    authorize_peer(stream.peer_cred()?.uid(), expected_uid)?;
    let request =
        tokio::time::timeout(std::time::Duration::from_secs(5), read_request(&mut stream))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "agent request timed out"))??;
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(25),
        dispatch(request, &config),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "host inspection timed out"))?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let encoded = serde_json::to_vec(&response).map_err(io::Error::other)?;
        if encoded.len() > manager_protocol::MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "agent response exceeds frame limit",
            ));
        }
        stream.write_u32(encoded.len() as u32).await?;
        stream.write_all(&encoded).await?;
        stream.shutdown().await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "agent request timed out"))?
}
