CREATE TABLE IF NOT EXISTS allocation_pool (
    id TEXT PRIMARY KEY NOT NULL,
    host_ip TEXT NOT NULL,
    port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    transport TEXT NOT NULL CHECK (transport IN ('tcp', 'udp')),
    UNIQUE(host_ip, port, transport)
);
CREATE TABLE IF NOT EXISTS server_provisioning (
    server_id TEXT PRIMARY KEY NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    egg_revision_id TEXT NOT NULL REFERENCES egg_revisions(id),
    job_id TEXT UNIQUE NOT NULL REFERENCES jobs(id),
    image TEXT NOT NULL,
    variables TEXT NOT NULL,
    memory_bytes INTEGER NOT NULL CHECK (memory_bytes > 0),
    disk_bytes INTEGER NOT NULL CHECK (disk_bytes > 0),
    cpu_millis INTEGER NOT NULL CHECK (cpu_millis > 0)
);
CREATE TABLE IF NOT EXISTS server_allocations (
    server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    allocation_id TEXT UNIQUE NOT NULL REFERENCES allocation_pool(id),
    is_primary INTEGER NOT NULL CHECK (is_primary IN (0, 1)),
    PRIMARY KEY(server_id, allocation_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS one_primary_allocation ON server_allocations(server_id) WHERE is_primary = 1;
