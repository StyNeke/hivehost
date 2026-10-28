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

pub async fn dispatch(
    request: AgentRequest,
    runtime: &std::sync::Arc<crate::runtime::Runtime>,
) -> AgentResponse {
    use manager_protocol::Operation;
    let result = if request.version != VERSION {
        AgentResult::UnsupportedVersion
    } else {
        match request.operation {
            Operation::Ping => AgentResult::Pong,
            Operation::InspectHost => inspect(runtime).await,
            #[cfg(target_os = "linux")]
            operation => runtime_operation(runtime, operation).await,
            #[cfg(not(target_os = "linux"))]
            _ => AgentResult::Unavailable,
        }
    };
    AgentResponse {
        version: VERSION,
        request_id: request.request_id,
        result,
    }
}

async fn inspect(runtime: &crate::runtime::Runtime) -> AgentResult {
    #[cfg(target_os = "linux")]
    let (owned, bytes) = (runtime.owned_ids().await, runtime.owned_image_bytes().await);
    #[cfg(not(target_os = "linux"))]
    let (owned, bytes) = (std::collections::HashSet::new(), 0);
    match crate::host::inspect_host(&runtime.config, &owned, bytes).await {
        Ok(status) => AgentResult::HostStatus(Box::new(status)),
        Err(_) => AgentResult::Unavailable,
    }
}

#[cfg(target_os = "linux")]
async fn runtime_operation(
    runtime: &std::sync::Arc<crate::runtime::Runtime>,
    operation: manager_protocol::Operation,
) -> AgentResult {
    use manager_protocol::Operation;
    match operation {
        Operation::InstallServer { spec } => runtime.install(*spec).await,
        Operation::StartServer { spec } => runtime.start(*spec).await,
        Operation::StopServer { server_id } => runtime.stop(server_id).await,
        Operation::KillServer { server_id } => runtime.kill(server_id).await,
        Operation::SendCommand { server_id, command } => {
            runtime.send_command(server_id, &command).await
        }
        Operation::ServerStatus { server_id } => runtime.status(server_id).await,
        Operation::ReadConsole { server_id, lines } => runtime.console(server_id, lines).await,
        Operation::InstallFiles {
            server_id,
            folder,
            files,
            remove,
        } => {
            runtime
                .install_files(server_id, folder, files, remove)
                .await
        }
        Operation::ListFiles { server_id, folder } => runtime.list_files(server_id, folder).await,
        Operation::DeleteServer {
            server_id,
            keep_backups,
        } => runtime.delete(server_id, keep_backups).await,
        Operation::CreateBackup {
            server_id,
            retention,
        } => runtime.create_backup(server_id, retention).await,
        Operation::ListBackups { server_id } => runtime.list_backups(server_id).await,
        Operation::RestoreBackup {
            server_id,
            backup_id,
        } => runtime.restore_backup(server_id, backup_id).await,
        Operation::DeleteBackup {
            server_id,
            backup_id,
        } => runtime.delete_backup(server_id, backup_id).await,
        Operation::Ping | Operation::InspectHost => AgentResult::Unavailable,
    }
}
