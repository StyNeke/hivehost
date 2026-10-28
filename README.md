# Linux game server manager

Work in progress: a new Rust-based manager for one private Linux host. Start with [progression.md](progression.md), then the [approved design](design.md) and [implementation plan](implementation-plan.md). All three handoff documents live together in this root folder.

Implemented so far: invite-only accounts, owner/member access, a shared egg library with administrator runtime qualification, atomic capacity admission, a root host agent that installs and runs game servers in hardened Docker containers with per-server hard disk quotas, crash restarts, graceful shutdown and boot restore, Modrinth mod installs with dependency resolution, and a Codex-style web interface (creation, live console, power controls, mods, activity, deletion). A Fabric server with Carpet has been installed, started and used to spawn bots through the real interface in a disposable Linux test environment. It has not yet been deployed to the target host; backups, file manager, schedules, metrics and other Minecraft editions are still pending. See [progression.md](progression.md) and the [deployment runbook](docs/operations/deployment.md).

## Local development

Requires the pinned Rust toolchain and a native C compiler for bundled SQLite. Run:

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Create a new administrator using a hidden interactive password prompt:

```powershell
$env:GM_DATABASE = 'game-manager.sqlite'
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

Host-backed routes: `GET /api/host` (safe budget, reserved and remaining CPU/RAM/disk plus agent blockers; no private IPs), `GET /api/allocations`, admin-only `POST /api/allocations`, `POST /api/servers`, and per-server detail, power, console, reinstall, delete, activity and Modrinth extension routes. Admission always uses a fresh agent snapshot (at most 30 seconds old). Set `GM_STATIC_DIR` to serve the built interface from the API.

Deployment remains blocked on [legacy resource reservations and storage quota qualification](docs/operations/storage-and-network.md). Existing host services have not been changed. Linux-only host behavior still requires Linux acceptance tests.

The current Rust foundation passed [Linux container validation](docs/operations/linux-validation.md). Game control and recovery still require their own acceptance checks. The approved visual direction is a Codex-inspired dark sidebar/workspace, with browser testing on the user's PC.
