// Backup operations of the runtime. A running server is stopped gracefully for a consistent
// archive and started again afterwards; failures restore the previous running state.
use super::{Runtime, now, persist, rejected, runtime_name};
use crate::{
    backups,
    files::as_server,
    storage::{self, filesystem_space},
};
use manager_protocol::{
    AgentResult, BackupInfo, BackupKind, OperationKind, OperationState, OperationStatus,
};
use std::{
    io::{self, Read},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;

fn sha256_file(path: &std::path::Path) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn fd_available(file: &std::fs::File) -> io::Result<u64> {
    use std::os::fd::AsRawFd;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid descriptor and out pointer.
    if unsafe { libc::fstatvfs(file.as_raw_fd(), &mut stat) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

impl Runtime {
    fn backup_dir(&self, id: Uuid) -> PathBuf {
        self.config.backup_root.join(id.to_string())
    }

    // Backup directories are readable by the API's group so it can stream downloads.
    fn prepare_backup_dir(&self, id: Uuid) -> io::Result<PathBuf> {
        let root = &self.config.backup_root;
        let directory = self.backup_dir(id);
        std::fs::create_dir_all(&directory)?;
        for path in [root, &directory] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o750))?;
            if let Some(gid) = self.config.api_gid {
                std::os::unix::fs::chown(path, Some(0), Some(gid))?;
            }
        }
        Ok(directory)
    }

    pub fn backups_of(&self, id: Uuid) -> io::Result<Vec<BackupInfo>> {
        let directory = self.backup_dir(id);
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(error) => return Err(error),
        };
        let mut backups = vec![];
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let info: BackupInfo = serde_json::from_slice(&std::fs::read(&path)?)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "corrupt backup record"))?;
            if directory.join(format!("{}.tar.gz", info.id)).is_file() {
                backups.push(info);
            }
        }
        backups.sort_by_key(|b| std::cmp::Reverse(b.created_at));
        Ok(backups)
    }

    fn remove_backup_files(&self, id: Uuid, backup: Uuid) -> io::Result<()> {
        for name in [format!("{backup}.tar.gz"), format!("{backup}.json")] {
            match std::fs::remove_file(self.backup_dir(id).join(name)) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        Ok(())
    }

    // Interrupted archive writes leave `.partial` files; they are never valid backups.
    pub(super) fn clean_partial_backups(&self) -> io::Result<()> {
        let Ok(servers) = std::fs::read_dir(&self.config.backup_root) else {
            return Ok(());
        };
        for server in servers.flatten() {
            for file in std::fs::read_dir(server.path())?.flatten() {
                if file.path().extension().and_then(|e| e.to_str()) == Some("partial") {
                    std::fs::remove_file(file.path())?;
                }
            }
        }
        Ok(())
    }

    pub async fn list_backups(&self, id: Uuid) -> AgentResult {
        match self.backups_of(id) {
            Ok(backups) => AgentResult::Backups(backups),
            Err(error) => rejected("backups", error.to_string()),
        }
    }

    pub(super) async fn begin(
        &self,
        id: Uuid,
        kind: OperationKind,
        message: &str,
        require_stopped: bool,
    ) -> Result<bool, AgentResult> {
        let mut records = self.records.lock().await;
        let Some(record) = records.get_mut(&id) else {
            return Err(rejected("not_found", "server not found"));
        };
        if record.busy() {
            return Err(rejected(
                "busy",
                "another operation is running for this server",
            ));
        }
        if !record.installed {
            return Err(rejected("not_installed", "install the server first"));
        }
        if require_stopped && (record.desired_running || record.stop_requested_at.is_some()) {
            return Err(rejected(
                "running",
                "stop the server before restoring a backup",
            ));
        }
        let was_running = record.desired_running;
        record.operation = Some(OperationStatus {
            kind,
            state: OperationState::Running,
            message: message.into(),
            updated_at: now(),
        });
        persist(&self.layout, record).map_err(|e| rejected("storage", e.to_string()))?;
        Ok(was_running)
    }

    pub async fn create_backup(self: &Arc<Self>, id: Uuid, retention: u8) -> AgentResult {
        if !(1..=5).contains(&retention) {
            return rejected("invalid_retention", "keep between 1 and 5 backups");
        }
        let was_running = match self
            .begin(id, OperationKind::Backup, "Preparing the backup", false)
            .await
        {
            Ok(was_running) => was_running,
            Err(result) => return result,
        };
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            let outcome = runtime
                .run_backup(id, BackupKind::Manual, Some(retention))
                .await;
            // The previous running state is restored whether or not the backup worked.
            if was_running && let Err(error) = runtime.launch(id).await {
                let _ = runtime
                    .update(id, |r| {
                        r.problem = Some(format!("Could not start again after the backup: {error}"))
                    })
                    .await;
            }
            runtime
                .finish(
                    id,
                    OperationKind::Backup,
                    outcome.map(|info| {
                        format!(
                            "Backup of {} MiB created",
                            info.size_bytes.div_ceil(1024 * 1024)
                        )
                    }),
                )
                .await;
        });
        AgentResult::Accepted
    }

    // Stops a running server gracefully (forced after the grace period) without counting a crash.
    async fn stop_and_wait(&self, id: Uuid) -> io::Result<()> {
        let stop = {
            let mut records = self.records.lock().await;
            let record = records
                .get_mut(&id)
                .ok_or_else(|| io::Error::other("server vanished"))?;
            record.desired_running = false;
            record.pending_restart_at = None;
            persist(&self.layout, record)?;
            record.spec.stop.clone()
        };
        if !self.container_state(&runtime_name(id)).await?.running {
            return Ok(());
        }
        self.progress(id, "Stopping the server for a consistent backup")
            .await;
        self.send_stop(id, &stop).await?;
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(self.config.stop_grace_secs);
        while tokio::time::Instant::now() < deadline {
            if !self.container_state(&runtime_name(id)).await?.running {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        self.docker(
            vec!["kill".into(), "--".into(), runtime_name(id).into()],
            30,
            1024,
        )
        .await?;
        let grace = self.config.stop_grace_secs;
        self.update(id, |r| r.problem = Some(format!("The server did not stop within {grace} seconds and was force-stopped before the backup"))).await?;
        Ok(())
    }

    pub(super) async fn run_backup(
        &self,
        id: Uuid,
        kind: BackupKind,
        retention: Option<u8>,
    ) -> io::Result<BackupInfo> {
        let uid = self
            .records
            .lock()
            .await
            .get(&id)
            .map(|r| r.uid)
            .ok_or_else(|| io::Error::other("server vanished"))?;
        self.stop_and_wait(id).await?;
        storage::ensure_mounted(&self.layout, id).await?;
        let volume = self.layout.volume(id);
        let (used, _) = storage::volume_usage(&volume)?;
        let directory = self.prepare_backup_dir(id)?;
        let (_, available) = filesystem_space(&directory)?;
        let reserve = self.config.disk_reserve_bytes;
        if available.saturating_sub(used) < reserve {
            return Err(io::Error::other(
                "not enough free disk space for a backup without using the reserved space; delete old backups or free space",
            ));
        }
        self.progress(id, "Writing the backup").await;
        let backup = Uuid::new_v4();
        let partial = directory.join(format!("{backup}.tar.gz.partial"));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o640)
            .open(&partial)?;
        if let Some(gid) = self.config.api_gid {
            std::os::unix::fs::chown(&partial, Some(0), Some(gid))?;
        }
        let root = volume.clone();
        let written = tokio::task::spawn_blocking(move || {
            let guard_file = file.try_clone()?;
            let mut writer = backups::Guarded::new(std::io::BufWriter::new(file), move |_| {
                if fd_available(&guard_file)? < reserve {
                    return Err(io::Error::other(
                        "the backup was stopped before it could use the reserved disk space",
                    ));
                }
                Ok(())
            });
            let result = as_server(uid, || backups::write_archive(&root, &mut writer));
            let file = writer
                .into_inner()
                .into_inner()
                .map_err(|e| e.into_error())?;
            file.sync_all()?;
            result
        })
        .await
        .map_err(|_| io::Error::other("backup task failed"))?;
        let verified = async {
            written?;
            self.progress(id, "Verifying the backup").await;
            let check = partial.clone();
            tokio::task::spawn_blocking(move || -> io::Result<String> {
                backups::verify_archive(std::io::BufReader::new(std::fs::File::open(&check)?))?;
                sha256_file(&check)
            })
            .await
            .map_err(|_| io::Error::other("verification task failed"))?
        }
        .await;
        let sha256 = match verified {
            Ok(sha256) => sha256,
            Err(error) => {
                let _ = std::fs::remove_file(&partial);
                return Err(error);
            }
        };
        let info = BackupInfo {
            id: backup,
            kind,
            created_at: now(),
            size_bytes: std::fs::metadata(&partial)?.len(),
            sha256,
        };
        let meta = directory.join(format!("{backup}.json"));
        std::fs::write(&meta, serde_json::to_vec(&info).map_err(io::Error::other)?)?;
        std::fs::set_permissions(&meta, std::fs::Permissions::from_mode(0o640))?;
        if let Some(gid) = self.config.api_gid {
            std::os::unix::fs::chown(&meta, Some(0), Some(gid))?;
        }
        std::fs::rename(&partial, directory.join(format!("{backup}.tar.gz")))?;
        // Only now, with the new backup verified, are older ones beyond the limit removed.
        let existing = self.backups_of(id)?;
        let (keep_kind, limit) = match (kind, retention) {
            (BackupKind::Manual, Some(limit)) => (BackupKind::Manual, limit as usize),
            (kind, _) => (kind, 1),
        };
        for old in existing.iter().filter(|b| b.kind == keep_kind).skip(limit) {
            self.remove_backup_files(id, old.id)?;
        }
        Ok(info)
    }

    pub async fn delete_backup(&self, id: Uuid, backup: Uuid) -> AgentResult {
        if let Some(record) = self.records.lock().await.get(&id)
            && record.busy()
            && record
                .operation
                .as_ref()
                .is_some_and(|op| matches!(op.kind, OperationKind::Backup | OperationKind::Restore))
        {
            return rejected("busy", "a backup or restore is running for this server");
        }
        match self.backups_of(id) {
            Ok(backups) if backups.iter().any(|b| b.id == backup) => {
                match self.remove_backup_files(id, backup) {
                    Ok(()) => AgentResult::Accepted,
                    Err(error) => rejected("backups", error.to_string()),
                }
            }
            Ok(_) => rejected("not_found", "backup not found"),
            Err(error) => rejected("backups", error.to_string()),
        }
    }

    pub async fn restore_backup(self: &Arc<Self>, id: Uuid, backup: Uuid) -> AgentResult {
        let info = match self.backups_of(id) {
            Ok(backups) => match backups.into_iter().find(|b| b.id == backup) {
                Some(info) => info,
                None => return rejected("not_found", "backup not found"),
            },
            Err(error) => return rejected("backups", error.to_string()),
        };
        match self.container_state(&runtime_name(id)).await {
            Ok(state) if state.running => {
                return rejected("running", "stop the server before restoring a backup");
            }
            Err(error) => return rejected("docker", error.to_string()),
            _ => {}
        }
        if let Err(result) = self
            .begin(id, OperationKind::Restore, "Checking the backup", true)
            .await
        {
            return result;
        }
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            let outcome = runtime.run_restore(id, info).await;
            runtime.finish(id, OperationKind::Restore, outcome).await;
        });
        AgentResult::Accepted
    }

    async fn run_restore(&self, id: Uuid, info: BackupInfo) -> io::Result<String> {
        let archive = self.backup_dir(id).join(format!("{}.tar.gz", info.id));
        let check = archive.clone();
        let actual = tokio::task::spawn_blocking(move || sha256_file(&check))
            .await
            .map_err(|_| io::Error::other("verification task failed"))??;
        if actual != info.sha256 {
            return Err(io::Error::other(
                "the backup file is damaged (checksum mismatch); nothing was changed",
            ));
        }
        // A copy of the current data, so the restore itself can be undone.
        self.progress(id, "Saving the current data before restoring")
            .await;
        self.run_backup(id, BackupKind::PreRestore, None)
            .await
            .map_err(|error| {
                io::Error::other(format!(
                    "could not save the current data first, so nothing was changed: {error}"
                ))
            })?;
        self.progress(id, "Restoring").await;
        self.restore_files(id, &archive).await?;
        Ok("Backup restored; the previous data was saved as a pre-restore backup".into())
    }

    // Receives an uploaded archive in order. Imports never use the reserved disk space and are
    // verified (readable, every path inside the server directory) before they are listed.
    pub async fn import_chunk(
        &self,
        id: Uuid,
        upload: Uuid,
        offset: u64,
        data: Vec<u8>,
        last: bool,
    ) -> AgentResult {
        if !self.records.lock().await.contains_key(&id) {
            return rejected("not_found", "server not found");
        }
        match self.write_import(id, upload, offset, &data, last).await {
            Ok(Some(info)) => AgentResult::Backups(vec![info]),
            Ok(None) => AgentResult::Accepted,
            Err(error) => {
                let _ = std::fs::remove_file(
                    self.backup_dir(id).join(format!("{upload}.tar.gz.partial")),
                );
                let code = if error.kind() == io::ErrorKind::InvalidData {
                    "invalid"
                } else {
                    "import"
                };
                rejected(code, error.to_string())
            }
        }
    }

    async fn write_import(
        &self,
        id: Uuid,
        upload: Uuid,
        offset: u64,
        data: &[u8],
        last: bool,
    ) -> io::Result<Option<BackupInfo>> {
        use std::io::Write;
        const MAX_IMPORT_BYTES: u64 = 256 * 1024 * 1024 * 1024;
        let directory = self.prepare_backup_dir(id)?;
        let partial = directory.join(format!("{upload}.tar.gz.partial"));
        let mut file = if offset == 0 {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o640)
                .open(&partial)?
        } else {
            std::fs::OpenOptions::new().append(true).open(&partial)?
        };
        if file.metadata()?.len() != offset {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "upload chunks arrived out of order",
            ));
        }
        if offset + data.len() as u64 > MAX_IMPORT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the archive is too large",
            ));
        }
        if fd_available(&file)?.saturating_sub(data.len() as u64) < self.config.disk_reserve_bytes {
            return Err(io::Error::other(
                "not enough free disk space for the archive without using the reserved space",
            ));
        }
        file.write_all(data)?;
        if !last {
            return Ok(None);
        }
        file.sync_all()?;
        drop(file);
        if let Some(gid) = self.config.api_gid {
            std::os::unix::fs::chown(&partial, Some(0), Some(gid))?;
        }
        let check = partial.clone();
        let sha256 = tokio::task::spawn_blocking(move || -> io::Result<String> {
            backups::verify_archive(std::io::BufReader::new(std::fs::File::open(&check)?))
                .map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("not a usable .tar.gz archive of server files: {error}"),
                    )
                })?;
            sha256_file(&check)
        })
        .await
        .map_err(|_| io::Error::other("verification task failed"))??;
        let info = BackupInfo {
            id: upload,
            kind: BackupKind::Imported,
            created_at: now(),
            size_bytes: std::fs::metadata(&partial)?.len(),
            sha256,
        };
        let meta = directory.join(format!("{upload}.json"));
        std::fs::write(&meta, serde_json::to_vec(&info).map_err(io::Error::other)?)?;
        std::fs::set_permissions(&meta, std::fs::Permissions::from_mode(0o640))?;
        if let Some(gid) = self.config.api_gid {
            std::os::unix::fs::chown(&meta, Some(0), Some(gid))?;
        }
        std::fs::rename(&partial, directory.join(format!("{upload}.tar.gz")))?;
        Ok(Some(info))
    }

    // Replaces the server's files with an archive's contents through a staging directory.
    pub(super) async fn restore_files(
        &self,
        id: Uuid,
        archive: &std::path::Path,
    ) -> io::Result<()> {
        let (uid, spec) = self
            .records
            .lock()
            .await
            .get(&id)
            .map(|r| (r.uid, r.spec.clone()))
            .ok_or_else(|| io::Error::other("server vanished"))?;
        storage::ensure_mounted(&self.layout, id).await?;
        let volume = self.layout.volume(id);
        let input = std::fs::File::open(archive)?;
        tokio::task::spawn_blocking(move || {
            as_server(uid, || {
                backups::clean_restore_leftovers(&volume)?;
                let staging = volume.join(backups::STAGING);
                if let Err(error) = backups::extract_archive(std::io::BufReader::new(input), &staging) {
                    let _ = std::fs::remove_dir_all(&staging);
                    if error.raw_os_error() == Some(libc::ENOSPC) {
                        return Err(io::Error::other(
                            "the server's disk limit has too little free space to stage the restore; nothing was changed",
                        ));
                    }
                    return Err(error);
                }
                backups::swap_in(&volume)
            })
        })
        .await
        .map_err(|_| io::Error::other("restore task failed"))??;
        // The restored files may hold another game version than the record says.
        let game = self.detect_installed(id, &spec, uid).await?;
        self.update(id, |record| record.game = game).await?;
        Ok(())
    }

    pub(super) fn backup_path(&self, id: Uuid, backup: Uuid) -> PathBuf {
        self.backup_dir(id).join(format!("{backup}.tar.gz"))
    }

    pub(super) fn remove_all_backups(&self, id: Uuid) -> io::Result<()> {
        match std::fs::remove_dir_all(self.backup_dir(id)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    // Interrupted restores are resolved to the original data before the server can start.
    pub(super) async fn recover_restores(&self) -> io::Result<()> {
        let servers: Vec<(Uuid, u32)> = self
            .records
            .lock()
            .await
            .iter()
            .map(|(id, r)| (*id, r.uid))
            .collect();
        for (id, uid) in servers {
            let volume = self.layout.volume(id);
            if storage::mounted(&volume)? {
                tokio::task::spawn_blocking(move || {
                    as_server(uid, || backups::clean_restore_leftovers(&volume))
                })
                .await
                .map_err(|_| io::Error::other("restore recovery failed"))??;
            }
        }
        Ok(())
    }
}
