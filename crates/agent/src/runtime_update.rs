// Updates: the egg installer runs again on the existing files (newer builds of the chosen game
// version, or the version the settings now ask for). A verified pre-update backup comes first,
// and any failure puts the previous files back from it.
use super::{Runtime, game_summary, persist, rejected, validate_spec};
use crate::storage;
use manager_protocol::{AgentResult, BackupKind, OperationKind, ServerSpec};
use std::{io, sync::Arc};
use uuid::Uuid;

impl Runtime {
    pub async fn update_server(
        self: &Arc<Self>,
        spec: ServerSpec,
        keep_game_version: bool,
    ) -> AgentResult {
        if let Err(reason) = validate_spec(&spec) {
            return rejected("invalid_spec", reason);
        }
        let id = spec.server_id;
        let was_running = match self
            .begin(id, OperationKind::Update, "Preparing the update", false)
            .await
        {
            Ok(was_running) => was_running,
            Err(result) => return result,
        };
        {
            let mut records = self.records.lock().await;
            if let Some(record) = records.get_mut(&id) {
                if record.spec.image != spec.image {
                    record.image = None;
                }
                record.spec = spec;
                if let Err(error) = persist(&self.layout, record) {
                    return rejected("storage", error.to_string());
                }
            }
        }
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            let outcome = runtime.run_update(id, keep_game_version).await;
            let _ = runtime.update(id, |r| r.update_backup = None).await;
            if was_running && let Err(error) = runtime.launch(id).await {
                let _ = runtime
                    .update(id, |r| {
                        r.problem = Some(format!("Could not start again after the update: {error}"))
                    })
                    .await;
            }
            runtime.finish(id, OperationKind::Update, outcome).await;
        });
        AgentResult::Accepted
    }

    async fn run_update(&self, id: Uuid, keep_game_version: bool) -> io::Result<String> {
        let (spec, uid, previous) = {
            let records = self.records.lock().await;
            let record = records
                .get(&id)
                .ok_or_else(|| io::Error::other("server vanished"))?;
            (record.spec.clone(), record.uid, record.game.clone())
        };
        // Automatic updates must be able to tell the game version before and after.
        if keep_game_version
            && previous
                .as_ref()
                .is_none_or(|game| game.loader.as_deref() == Some("bedrock"))
        {
            return Err(io::Error::other(
                "automatic updates need a recognised game version, and this server's cannot be read reliably; update it manually",
            ));
        }
        self.ensure_network().await?;
        // Images first: a download failure leaves everything untouched.
        let pinned = self.prepare_images(id, &spec).await?;
        self.progress(id, "Saving the current files before updating")
            .await;
        let backup = self
            .run_backup(id, BackupKind::PreUpdate, None)
            .await
            .map_err(|error| {
                io::Error::other(format!(
                    "could not save the current files first, so nothing was changed: {error}"
                ))
            })?;
        self.update(id, |r| r.update_backup = Some(backup.id))
            .await?;
        storage::ensure_mounted(&self.layout, id).await?;
        storage::grow_volume(&self.layout, id, spec.limits.disk_bytes).await?;
        let installed = self.run_installer(id, &spec, uid, &pinned).await;
        let output = installed
            .as_ref()
            .map(|(_, output)| output.clone())
            .unwrap_or_default();
        let installed = installed.map(|(game, _)| game);
        let failure = match (&installed, &previous) {
            (Err(error), _) => Some(format!("The update failed: {error}")),
            // Installers can exit successfully yet leave no game behind (for example an unknown
            // version); keeping that would leave a server that cannot start.
            (Ok(None), Some(_)) => Some(format!(
                "The update finished but no game could be found in the files afterwards (check the version in Settings). The installer's last output:\n{output}"
            )),
            (Ok(Some(after)), Some(before))
                if keep_game_version && before.version != after.version =>
            {
                Some(format!(
                    "The update would move Minecraft {} to {}; automatic updates stay on the installed game version. Update manually to change versions",
                    before.version, after.version
                ))
            }
            _ => None,
        };
        if let Some(failure) = failure {
            self.progress(id, "Putting the previous files back").await;
            let archive = self.backup_path(id, backup.id);
            return Err(match self.restore_files(id, &archive).await {
                Ok(()) => io::Error::other(format!("{failure}. The previous files were restored.")),
                Err(error) => io::Error::other(format!(
                    "{failure}. Restoring the previous files also failed ({error}); restore the pre-update backup from the Backups tab."
                )),
            });
        }
        let game = installed?;
        let summary = match (&previous, &game) {
            (Some(before), Some(after)) if before == after => {
                format!(
                    "{} (same game version)",
                    game_summary("Updated", Some(after))
                )
            }
            _ => game_summary("Updated to", game.as_ref()),
        };
        self.update(id, |record| {
            record.image = Some(pinned);
            record.game = game;
        })
        .await?;
        Ok(summary)
    }

    // An update interrupted by an agent restart may have changed some files; the pre-update
    // backup puts all of them back before the server can start.
    pub(super) async fn recover_updates(&self) {
        let interrupted: Vec<(Uuid, Uuid)> = self
            .records
            .lock()
            .await
            .iter()
            .filter_map(|(id, r)| r.update_backup.map(|b| (*id, b)))
            .collect();
        for (id, backup) in interrupted {
            let archive = self.backup_path(id, backup);
            let message = match self.restore_files(id, &archive).await {
                Ok(()) => "The update was interrupted by an agent restart; the previous files were restored from the pre-update backup".to_string(),
                Err(error) => format!(
                    "The update was interrupted by an agent restart and restoring the previous files failed ({error}); restore the pre-update backup from the Backups tab"
                ),
            };
            let _ = self
                .update(id, |r| {
                    r.update_backup = None;
                    r.desired_running = false;
                    r.operation = Some(manager_protocol::OperationStatus {
                        kind: OperationKind::Update,
                        state: manager_protocol::OperationState::Failed,
                        message,
                        updated_at: super::now(),
                    });
                })
                .await;
        }
    }
}
