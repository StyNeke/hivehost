CREATE TABLE IF NOT EXISTS server_runtime (
    server_id TEXT PRIMARY KEY NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    license_accepted INTEGER NOT NULL CHECK (license_accepted IN (0, 1)),
    install_state TEXT NOT NULL DEFAULT 'pending'
        CHECK (install_state IN ('pending', 'installing', 'installed', 'failed')),
    install_message TEXT,
    deleting INTEGER NOT NULL DEFAULT 0 CHECK (deleting IN (0, 1))
);
CREATE TABLE IF NOT EXISTS egg_qualifications (
    revision_id TEXT PRIMARY KEY NOT NULL REFERENCES egg_revisions(id),
    qualified_by TEXT NOT NULL REFERENCES users(id),
    qualified_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS server_extensions (
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL,
    version_id TEXT NOT NULL,
    title TEXT NOT NULL,
    version_number TEXT NOT NULL,
    filename TEXT NOT NULL,
    dependency INTEGER NOT NULL CHECK (dependency IN (0, 1)),
    PRIMARY KEY (server_id, project_id),
    UNIQUE (server_id, filename)
);
CREATE TABLE IF NOT EXISTS activity (
    sequence INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    actor_id TEXT,
    server_id TEXT,
    server_name TEXT,
    action TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('succeeded', 'failed', 'requested')),
    detail TEXT
);
CREATE INDEX IF NOT EXISTS activity_by_server ON activity(server_id, sequence DESC);
