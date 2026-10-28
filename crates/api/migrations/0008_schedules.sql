-- Weekly scheduled actions per server. next_run_at is claimed atomically by the scheduler, so
-- each occurrence runs at most once even with several API processes.
CREATE TABLE IF NOT EXISTS schedules (
    id TEXT PRIMARY KEY NOT NULL,
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    action TEXT NOT NULL CHECK (action IN ('restart', 'backup', 'command', 'update')),
    command TEXT,
    hour INTEGER NOT NULL CHECK (hour BETWEEN 0 AND 23),
    minute INTEGER NOT NULL CHECK (minute BETWEEN 0 AND 59),
    weekdays INTEGER NOT NULL CHECK (weekdays BETWEEN 1 AND 127),
    timezone TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    next_run_at INTEGER NOT NULL,
    last_run_at INTEGER,
    last_outcome TEXT,
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS schedules_due ON schedules (enabled, next_run_at);
