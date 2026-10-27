CREATE TABLE IF NOT EXISTS jobs (
    sequence INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK (kind IN ('install', 'backup', 'restore', 'update', 'delete')),
    state TEXT NOT NULL DEFAULT 'queued' CHECK (state IN ('queued', 'running', 'completed')),
    worker TEXT,
    lease_until INTEGER,
    attempt INTEGER NOT NULL DEFAULT 0 CHECK (attempt >= 0),
    CHECK ((state = 'running' AND worker IS NOT NULL AND lease_until IS NOT NULL)
        OR (state != 'running' AND worker IS NULL AND lease_until IS NULL))
);
CREATE INDEX IF NOT EXISTS jobs_claim ON jobs(state, lease_until, sequence);
