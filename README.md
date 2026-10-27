# Linux game server manager

Work in progress: a new Rust-based manager for one private Linux host. See the [approved design](docs/superpowers/specs/2026-10-27-game-server-manager-design.md) and [implementation plan](docs/superpowers/plans/2026-10-27-game-server-manager.md).

Implemented so far: durable SQLite job leases/recovery, core permissions/resource/retention rules, invite-only account storage, and authentication HTTP endpoints. The browser interface, egg runner and host agent are not ready. This is not yet a usable game-server panel.

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

`GM_BIND` defaults to `127.0.0.1:19090` and must remain loopback. `GM_ORIGIN` is the exact browser HTTPS origin, with no trailing slash. Secure cookies intentionally require HTTPS. No TLS certificate or proxy is automatically installed. Database files contain credentials and must be stored in a directory accessible only to the service user; do not commit or share them.

API routes: `POST /api/login`, `/api/logout`, `/api/register`, `/api/invites` (admin), and `GET /api/session`. Mutating browser requests require an exact matching Origin header. JSON credentials use `username` and `password`; registration also takes `invite`. Invitations expire in 24 hours, sessions in seven days. Initial usernames are 3–32 ASCII letters/digits/underscores/hyphens, passwords 12–1024 bytes. Login/registration share a bounded process-wide attempt budget (10/minute) and password work is limited to four concurrent tasks.

Deployment remains blocked on [legacy resource reservations and storage quota qualification](docs/operations/storage-and-network.md). Existing host services have not been changed. Linux-only host behavior still requires Linux acceptance tests.
