-- Last known upstream status of experimental implementations, kept through outages.
CREATE TABLE IF NOT EXISTS experimental_status (
    project TEXT PRIMARY KEY NOT NULL,
    data TEXT,
    fetched_at INTEGER,
    attempted_at INTEGER NOT NULL,
    error TEXT
);
-- Library eggs that came from the experimental panel and must stay labelled as such.
CREATE TABLE IF NOT EXISTS experimental_eggs (
    egg_id TEXT PRIMARY KEY NOT NULL REFERENCES eggs(id) ON DELETE CASCADE,
    project TEXT NOT NULL
);
