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
// Lets the API service connect: the socket and its directory are given to the API user's
// primary group (socket 0660, directory 0750). Only the API's UID passes the peer check anyway.
pub fn grant_group(path: &Path, api_uid: u32) -> io::Result<()> {
    // SAFETY: getpwuid_r writes into the provided buffers only.
    let gid = unsafe {
        let mut entry: libc::passwd = std::mem::zeroed();
        let mut buffer = vec![0 as libc::c_char; 16 * 1024];
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        if libc::getpwuid_r(
            api_uid,
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        ) != 0
            || result.is_null()
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "GM_API_UID does not name an existing user",
            ));
        }
        entry.pw_gid
    };
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "socket parent required"))?;
    std::os::unix::fs::chown(parent, None, Some(gid))?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o750))?;
    std::os::unix::fs::chown(path, None, Some(gid))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))
}

pub async fn serve_connection(
    mut stream: UnixStream,
    expected_uid: u32,
    runtime: std::sync::Arc<crate::runtime::Runtime>,
) -> io::Result<()> {
    authorize_peer(stream.peer_cred()?.uid(), expected_uid)?;
    let request =
        tokio::time::timeout(std::time::Duration::from_secs(5), read_request(&mut stream))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "agent request timed out"))??;
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(25),
        dispatch(request, &runtime),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "agent operation timed out"))?;
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
