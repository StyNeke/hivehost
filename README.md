# HiveHost

Work in progress: a new Rust-based manager for one private Linux host. Start with [progression.md](progression.md), then the [approved design](design.md) and [implementation plan](implementation-plan.md). All three handoff documents live together in this root folder.

Implemented:
- Invite-only accounts, owner/member access with a members tab.
- A shared egg library with administrator runtime qualification.
- Atomic capacity admission, and limit changes (disk grows only).
- A root host agent that runs game servers in hardened Docker containers. It provides:
  - Per-server hard disk quotas and network isolation from the host and private networks.
  - Crash restarts with explained causes, graceful shutdown and boot restore.
  - CPU/RAM/network metrics.
- Minecraft Vanilla, Paper, Fabric, Forge, NeoForge and Bedrock, with Modrinth mods and plugins resolved with their dependencies.
- Experimental Pumpkin and SteelMC test servers with cached upstream GitHub status.
- Verified local backups with restore, retention, downloads and imports of other panels' server folders.
- A file manager, startup settings, manual and scheduled updates with automatic rollback, and weekly schedules in any time zone.
- A Codex-style web interface for all of it.

Everything was exercised against real Docker in a disposable Linux test environment:
- Including a Fabric server with Carpet spawning bots, and a real Java client joining.
- The agent's host tests also pass on Debian 13 userland.

It has not yet been deployed to the target host. See [progression.md](progression.md) for evidence and open points, and the [deployment runbook](docs/operations/deployment.md).

## Local development

Requires the pinned Rust toolchain and a native C compiler for bundled SQLite. Run:

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Create a new administrator using a hidden interactive password prompt:

```powershell
$env:GM_DATABASE = 'hivehost.sqlite'
cargo run -p manager-api -- init-admin stijn
```

Then run the API behind a privately bound HTTPS proxy:

```powershell
$env:GM_ORIGIN = 'https://panel.example.test:8443'
cargo run -p manager-api
```

To let the API admit servers against live host state, also set `GM_AGENT_SOCKET` (the agent's Unix socket) and `GM_AGENT_UID` (the agent service user's UID; the API refuses replies from any other peer). Without them, host-dependent routes report `host_unavailable`.

`GM_BIND` defaults to `127.0.0.1:19090` and must remain loopback. `GM_ORIGIN` is the exact browser HTTPS origin, with no trailing slash. Secure cookies intentionally require HTTPS. No TLS certificate or proxy is automatically installed. Database files contain credentials and must be stored in a directory accessible only to the service user; do not commit or share them.

API routes: `POST /api/login`, `/api/logout`, `/api/register`, `/api/invites` (admin), and `GET /api/session`. Mutating browser requests require an exact matching Origin header. JSON credentials use `username` and `password`; registration also takes `invite`. Invitations expire in 24 hours, sessions in seven days. Initial usernames are 3–32 ASCII letters/digits/underscores/hyphens, passwords 12–1024 bytes. Login/registration share a bounded process-wide attempt budget (10/minute) and password work is limited to four concurrent tasks.

Host-backed routes: `GET /api/host` (safe budget, reserved and remaining CPU/RAM/disk plus agent blockers; no private IPs), `GET /api/allocations`, admin-only `POST /api/allocations`, `POST /api/servers`, and per-server detail, power, console, reinstall, update, delete, activity, Modrinth, backups/imports, files, settings, limits, members and schedules routes. The full list is in [progression.md](progression.md). Admission always uses a fresh agent snapshot (at most 30 seconds old). Set `GM_STATIC_DIR` to serve the built interface from the API.

Deployment remains blocked on the [legacy resource reservations](docs/operations/storage-and-network.md) on the target host and on running the acceptance there. Existing host services have not been changed. The approved visual direction is a Codex-inspired dark sidebar/workspace, with browser testing on the user's PC.
