// File manager operations of the runtime: kernel-contained, as the server's own file identity.
use super::{Runtime, rejected};
use crate::{file_manager::Volume, files::as_server, storage};
use manager_protocol::AgentResult;
use std::io;
use uuid::Uuid;

fn file_error(error: io::Error) -> AgentResult {
    let code = match error.kind() {
        io::ErrorKind::NotFound => "not_found",
        io::ErrorKind::AlreadyExists => "conflict",
        io::ErrorKind::PermissionDenied => "denied",
        io::ErrorKind::InvalidInput => "invalid_path",
        _ if error.raw_os_error() == Some(libc::ENOSPC) => "disk_full",
        _ if error.raw_os_error() == Some(libc::ENOTEMPTY) => "not_empty",
        _ => "files",
    };
    let message = match code {
        "not_found" => "No such file or folder".to_owned(),
        "disk_full" => "The server's disk limit is full".to_owned(),
        "conflict" if !error.to_string().contains("changed since") => {
            "Something with that name already exists".to_owned()
        }
        _ => error.to_string(),
    };
    rejected(code, message)
}

impl Runtime {
    // Runs `action` on the server's volume. Changes are refused while installs, backups,
    // restores or deletion run; reading is always allowed.
    pub(super) async fn with_volume<T: Send + 'static>(
        &self,
        id: Uuid,
        changes: bool,
        action: impl FnOnce(&Volume) -> io::Result<T> + Send + 'static,
    ) -> Result<T, AgentResult> {
        let uid = {
            let records = self.records.lock().await;
            let record = records
                .get(&id)
                .ok_or_else(|| rejected("not_found", "server not found"))?;
            if !record.installed {
                return Err(rejected("not_installed", "install the server first"));
            }
            if changes && record.busy() {
                return Err(rejected(
                    "busy",
                    "another operation is running for this server",
                ));
            }
            record.uid
        };
        let root = self.layout.volume(id);
        if !storage::mounted(&root).unwrap_or(false) {
            storage::ensure_mounted(&self.layout, id)
                .await
                .map_err(|e| rejected("storage", e.to_string()))?;
        }
        tokio::task::spawn_blocking(move || as_server(uid, || action(&Volume::open(&root)?)))
            .await
            .map_err(|_| rejected("files", "file operation failed"))?
            .map_err(file_error)
    }

    pub async fn file_operation(&self, operation: manager_protocol::Operation) -> AgentResult {
        use crate::file_manager::validate;
        use base64::Engine;
        use manager_protocol::Operation;
        let engine = base64::engine::general_purpose::STANDARD;
        macro_rules! path {
            ($value:expr, $root:expr) => {
                match validate(&$value, $root) {
                    Ok(path) => path,
                    Err(error) => return file_error(error),
                }
            };
        }
        let outcome = match operation {
            Operation::ListDirectory { server_id, path } => {
                let path = path!(path, true);
                self.with_volume(server_id, false, move |v| v.list(&path))
                    .await
                    .map(AgentResult::Directory)
            }
            Operation::ReadText { server_id, path } => {
                let path = path!(path, false);
                self.with_volume(server_id, false, move |v| v.read_text(&path))
                    .await
                    .map(|(text, version)| AgentResult::Text { text, version })
            }
            Operation::WriteText {
                server_id,
                path,
                text,
                expected_version,
            } => {
                let path = path!(path, false);
                self.with_volume(server_id, true, move |v| {
                    v.write_text(&path, &text, expected_version.as_deref())
                })
                .await
                .map(|version| AgentResult::Saved { version })
            }
            Operation::ReadChunk {
                server_id,
                path,
                offset,
                length,
            } => {
                let path = path!(path, false);
                self.with_volume(server_id, false, move |v| {
                    v.read_chunk(&path, offset, length as usize)
                })
                .await
                .map(|(bytes, size)| AgentResult::Chunk {
                    data: engine.encode(bytes),
                    size,
                })
            }
            Operation::WriteChunk {
                server_id,
                upload_id,
                path,
                offset,
                data,
                last,
            } => {
                let path = path!(path, false);
                let Ok(bytes) = engine.decode(data.as_bytes()) else {
                    return rejected("invalid", "upload data is not valid base64");
                };
                self.with_volume(server_id, true, move |v| {
                    v.write_chunk(upload_id, &path, offset, &bytes, last)
                })
                .await
                .map(|_| AgentResult::Accepted)
            }
            Operation::CreateDirectory { server_id, path } => {
                let path = path!(path, false);
                self.with_volume(server_id, true, move |v| v.create_directory(&path))
                    .await
                    .map(|_| AgentResult::Accepted)
            }
            Operation::RenamePath {
                server_id,
                from,
                to,
            } => {
                let (from, to) = (path!(from, false), path!(to, false));
                self.with_volume(server_id, true, move |v| v.rename(&from, &to))
                    .await
                    .map(|_| AgentResult::Accepted)
            }
            Operation::DeletePath { server_id, path } => {
                let path = path!(path, false);
                self.with_volume(server_id, true, move |v| v.delete(&path))
                    .await
                    .map(|_| AgentResult::Accepted)
            }
            _ => Err(rejected("unsupported", "not a file operation")),
        };
        outcome.unwrap_or_else(|rejection| rejection)
    }

    // Upload staging files left by interrupted uploads are removed at start.
    pub(super) async fn clean_uploads(&self) {
        let ids: Vec<Uuid> = self
            .records
            .lock()
            .await
            .iter()
            .filter(|(_, r)| r.installed)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let _ = self.with_volume(id, false, |v| v.clean_uploads()).await;
        }
    }
}
