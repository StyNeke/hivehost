CREATE TABLE IF NOT EXISTS backup_settings (
    server_id TEXT PRIMARY KEY NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    retention INTEGER NOT NULL CHECK (retention BETWEEN 1 AND 5)
);
CREATE TABLE IF NOT EXISTS deletion_requests (
    server_id TEXT PRIMARY KEY NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    keep_backups INTEGER NOT NULL CHECK (keep_backups IN (0, 1))
);
-- Backups kept after their server was deleted keep the deletion-time access list.
CREATE TABLE IF NOT EXISTS retained_backups (
    server_id TEXT PRIMARY KEY NOT NULL,
    server_name TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    deleted_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS retained_backup_access (
    server_id TEXT NOT NULL REFERENCES retained_backups(server_id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (server_id, user_id)
);
