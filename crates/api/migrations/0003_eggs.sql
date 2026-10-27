CREATE TABLE IF NOT EXISTS eggs (
    id TEXT PRIMARY KEY NOT NULL,
    importer_id TEXT NOT NULL REFERENCES users(id)
);
CREATE TABLE IF NOT EXISTS egg_revisions (
    sequence INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    egg_id TEXT NOT NULL REFERENCES eggs(id),
    name TEXT NOT NULL,
    raw BLOB NOT NULL,
    normalized TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    creator_id TEXT NOT NULL REFERENCES users(id)
);
CREATE INDEX IF NOT EXISTS egg_revision_latest ON egg_revisions(egg_id, sequence DESC);
CREATE TRIGGER IF NOT EXISTS immutable_egg_revision BEFORE UPDATE ON egg_revisions
BEGIN SELECT RAISE(ABORT, 'egg revisions are immutable'); END;
