# Project progression and continuation handoff

Last updated: 2026-10-28 (Europe/Amsterdam).

**Resumed 2026-10-27** in a Claude Code cloud session on branch `claude/ecstatic-allen-u2m477`, which carries all earlier `codex/game-server-manager` commits. The user then set the goal "finish it, fix all edge cases, test it, and when done test a Fabric server with Carpet spawning bots". That cloud session built the game runner, server workspace and Modrinth installs and passed that acceptance through the real interface; see "Latest completed milestone". Keep maintaining this file continuously without treating each update as a request to stop.

## Read this first

**All planned product features are built and were exercised against real Docker** in a disposable Ubuntu 24.04 container (agent as root, API as a non-root user, browser over HTTPS):
- Fabric + Carpet bots (the acceptance goal).
- Backups, restore and the file manager.
- Startup settings, limit changes, CPU/RAM metrics and members.
- Vanilla, Paper (+ a plugin), Forge and NeoForge (+ mods), and Bedrock.
- Manual and scheduled updates with rollback, and schedules.
- Experimental Pumpkin and SteelMC servers.
- Network isolation and a Pterodactyl-style import rehearsal.
- A real Java client join.

The host-level tests also passed on Debian 13 userland (in a container). It has **still not run on the Debian target host**:
- Its legacy limits consume the safe budget, so creation will be blocked there until they are lowered (see blockers).
- A Bedrock client join could not be verified here, because this sandbox's kernel has no IPv6, which current Bedrock servers need for their sockets.

Do not tell the user it is deployment-ready for the target host.

The user requested this file so Claude or another agent can continue when Codex usage runs out. No Codex-specific tool or skill is required to understand the handoff.

All main planning documents now live together in the repository root:

- [progression.md](progression.md): actual progress, tests, blockers, next steps and decisions.
- [design.md](design.md): approved requirements and architecture.
- [implementation-plan.md](implementation-plan.md): task-by-task implementation plan.
- [FUTURE_FEATURES.md](FUTURE_FEATURES.md): deferred ideas, currently Modrinth modpacks.

The design and plan were moved from `docs/superpowers/`; those old paths are no longer canonical. Additional host evidence remains under [docs/operations](docs/operations).

## User intent and decisions

Build a web-based game manager from scratch for one existing Debian host and a small private team. Keep portability to other Linux distributions in mind, but qualify Debian first. No Panel/Wings code or runtime dependency. Pterodactyl egg JSON imports and existing third-party runtime/installer images are explicitly allowed.

- Everyone may create servers and import eggs into one shared library.
- Every server has one owner. Enabled teammates have full control, including deletion and member management. Owner access cannot be revoked; administrator always has full access.
- Without access, teammates see only server name and resource usage, plus the UI indication that they cannot open it.
- Resource presets must respect a safe CPU/RAM/disk budget, accounting for legacy workloads and host overhead. Do not use current free RAM as the only budget.
- Minecraft Java first; Bedrock also required. Vanilla, Paper, Fabric, Forge and NeoForge choices.
- Modrinth: individual mods/plugins matched to installation and dependencies. Complete modpacks are deferred.
- Experimental Pumpkin/SteelMC installation and real sourced GitHub progress. No invented completeness percentage or release ETA.
- Console, file management/text editing, IP/port allocations, local backups/restore, schedules, startup settings, resource monitoring and activity logs are all in scope.
- Backups: local host storage, default retention 3, adjustable 1–5; only delete oldest after replacement succeeds. No default backup schedule. Keep backups optionally when deleting a server; deletion requires typing its name.
- Manual updates default. Automatic updates are opt-in, within the selected Minecraft version, with a successful backup first and a warning about broken mods/plugins/worlds. Minecraft-version upgrades stay manual.
- Graceful controlled host shutdown: game stop command/signal, 120-second default grace, then force-stop/log if hung. Restart previously running servers after boot. Crash retries: up to 3 in 10 minutes with backoff.
- Invite-only accounts; single-use links and user-chosen passwords. User is administrator.
- Private network only, confirmed NordVPN Meshnet. No public Cloudflare route for the panel.
- Eventual Pterodactyl migration, rehearsed on copied data before each live cutover.
- UI should look like Codex: dark sidebar/workspace, subtle borders, compact controls, restrained typography. Browser testing on the user's PC is explicitly authorized.
- User chose implementation in this chat/current folder, on a new branch, rather than a separate worktree. Do not ask these questions again.

## Repository and development environment

- Local root: `C:\Users\Stijn\Documents\ChatGPT\Game Panel Web Based For Linux`.
- Shell: PowerShell. Windows development PC, Rust/C compiler/Node/npm available. WSL and local Docker were not installed at inspection.
- Branch: `codex/game-server-manager`.
- Rust workspace: `crates/core`, `crates/api`, `crates/protocol`, `crates/agent`.
- Pinned Windows Rust toolchain: 1.99.0. Linux test image contains 1.99.1; exact patch differs and is recorded, not hidden.
- Frontend: Svelte 5, TypeScript 6, Vite 8, Vitest 5, jsdom, Testing Library. TypeScript 7 was rejected by svelte-check's peer requirements; use the lockfile-supported version rather than forcing peers.
- SQLite is bundled via rusqlite. No Redis or external database needed for this single-host app.
- `.gitignore` excludes target, node_modules, web/dist, SQLite files, .env and scratch `.superpowers` state. `.gitattributes` preserves fixture JSON bytes and Linux-friendly text line endings.
- Secrets must never enter the repository. No production panel account/default password has been created.

## What exists and what remains

Legend: **built** = implemented and tested for the listed scope; **partial** = only the listed slice exists; **pending** = not implemented. Do not equate a partial task with completed product behavior.

| Plan task | Status | Implemented | Remaining |
| --- | --- | --- | --- |
| 1. Host qualification | Partial; evidence recorded | Read-only inventory, real Meshnet confirmation, CPU/RAM/storage/legacy limits, Docker/network/service inspection, capacity proposal; agent host tests and `inspect-host` pass on Debian 13 userland in a privileged container (`tests/host/debian-userland.sh`) | A run on the Debian host itself, a justified live budget (legacy limits), private TLS trust, friend-peer browser tests |
| 2. Foundation | Built | UUID models, durable SQLite leases, typed bounded RPC (4 MiB frames), peer-UID checks, agent registry, instance lock, persisted operation state; interrupted restores and updates are put back at agent start | A real agent/API run on the Debian host |
| 3. Accounts and permissions | Built for the team scope | Bootstrap admin, Argon2id, invites, sessions, origin checks, throttling; owner/member/admin rules on every route (streams re-check); members tab grants/removes access | Ownership transfer, account lifecycle, admin-wide audit view |
| 4. Shared egg library | Built | Pinned Vanilla/Paper/Fabric/Forge/NeoForge/Bedrock fixtures and bundled experimental Pumpkin/SteelMC eggs; strict parser with `properties` and `file` (line) transforms; immutable revisions; admin qualification | yaml/json/ini transforms |
| 5. Capacity/allocations | Built | Atomic admission; hard disk quotas (ext4 loop images); limit changes admitted against everyone else, disk grow-only (offline resize at next start); UDP allocations; create form warns on a transport that does not fit the game | Host presets, allocation removal; applying on the Debian host |
| 6. Java/Bedrock games | Built; accepted in the test container | Hardened installer/runtime containers, digest pinning, image re-pin when the image setting changes, game/loader detection for Vanilla, Paper (paperclip), Fabric, Forge, NeoForge and Bedrock; a real Java client (mineflayer, 26.1) joined, chatted and left | Bedrock client join (blocked here: no IPv6 in this kernel), Debian-host acceptance |
| 7. Console/lifecycle/metrics | Built | Console, readiness, graceful stop/kill, crash policy with explained causes (wrong Java, missing jar, port/IPv6, refused EULA/allowlist, Pumpkin Bedrock listener, out of memory) that stop retrying deterministic failures; boot restore; SIGTERM shutdown; CPU/RAM/network metrics every 5 s | WebSocket streaming (polling with per-request access checks is used), Debian reboot acceptance |
| 8. Browser UI | Built | Servers, library, experimental panel, create page, workspace tabs: Console, Files, Mods, Backups (incl. import), Schedules, Settings (startup + limits + update), Members, Activity, Delete | Mobile pass of the newest tabs |
| 9. Backups/deletion | Built | Verified gzip tar backups (SHA-256), retention 1–5, pre-restore/pre-update copies, staged reversible restores, streaming downloads with access re-checks, keep-on-delete with deletion-time access, imports of `.tar.gz` server folders | Off-host copies |
| 10. Schedules/settings/activity | Built | Weekly schedules (restart/backup/command/update) in IANA time zones with DST handling, once per occurrence, missed runs recorded not run late; startup settings (editable variables, egg images) with names-only activity; per-server activity | Admin-wide audit view |
| 11. Minecraft/Modrinth | Built | Modrinth for Fabric/Forge/NeoForge mods and Paper plugins (Modrinth v2 reports plugins as `mod` projects); verified live: Carpet, LuckPerms, No Chat Reports, FerriteCore | Modpacks (deferred by design) |
| 12. Updates | Built | Manual update re-runs the egg installer after a verified pre-update backup; failures, "no game afterwards" and (scheduled) version changes roll back; interrupted updates restored at boot; scheduled updates stay on the installed version, pause after a failure, refuse unreliable versions (Bedrock) | Choosing among published builds by name |
| 13. Experimental implementations | Built | Pumpkin (upstream egg) and SteelMC (egg written here, SIGINT stop) installable as test servers, labelled experimental; GitHub releases/commits/milestones cached 1 h, background refresh, 60 s refresh spacing, stale label with the reason on errors/rate limits | Live GitHub metadata is unverified here (the sandbox blocks the GitHub API; tested against a fake) |
| 14. Deployment/migration | Partial | systemd units, nginx template, runbook; network isolation chains; import + restore path rehearsed with a Pterodactyl-style archive | Install on the Debian host, private TLS, per-server migration of the real legacy servers in an agreed window |

## Current code map

| Location | Purpose |
| --- | --- |
| `crates/core/src/model.rs` | IDs and small domain models |
| `crates/core/src/permissions.rs` | Owner/admin/member rules |
| `crates/core/src/capacity.rs` | Positive resource validation and checked admission without overflow |
| `crates/core/src/backup_policy.rs` | Retention defaults/bounds/excess calculation only; no file I/O |
| `crates/core/src/eggs.rs` | PTDL_v2 normalization, supported rules/properties transforms, fail-closed compatibility errors |
| `crates/api/src/jobs.rs` | SQLite enqueue/claim/renew/complete; attempt fencing; not a game worker |
| `crates/api/src/auth.rs` | Accounts, Argon2id, hashed invites/sessions, session authentication |
| `crates/api/src/eggs.rs` | Shared library store, immutable revisions/original JSON/SHA256 |
| `crates/api/src/servers.rs` | Metadata list/detail authorization and member toggles |
| `crates/api/src/runtime.rs` | Install state, server specs from pinned revisions, deletion, qualification, extensions, activity |
| `crates/api/src/worker.rs` | Leased install jobs (max 2 concurrent) and durable deletions against the agent |
| `crates/api/src/modrinth.rs` | Modrinth search and dependency resolution with validation |
| `crates/api/src/http_server_routes.rs` | Server detail/power/console/reinstall/delete/activity/extensions/qualification routes |
| `crates/api/src/provisioning.rs` | Atomic creation/reservations/allocation pool against trusted host state; reservation totals and free-allocation listing |
| `crates/api/src/database.rs` | Shared atomic schema initialization, foreign keys, busy timeout and schema-version downgrade protection |
| `crates/api/src/http.rs` | Authenticated API routes, origin/cookie/body/worker limits, library/server handlers; `router` (no agent, fails closed) and `router_with_agent` |
| `crates/api/src/main.rs`, `config.rs` | Local CLI/bootstrap/startup validation; fixed loopback listener behind expected HTTPS origin |
| `crates/api/migrations/0001_jobs.sql` through `0009_experimental.sql` | Feature-owned schema (schema version 9): reservations, provisioning, runtime, backups, schedules, experimental status |
| `crates/protocol/src/lib.rs` | Version 1 request/response, 64 KiB frames; host inspection and server operations |
| `crates/agent/src/rpc.rs`, `linux.rs`, `main.rs` | Peer UID check, socket group grant to the API user, dispatch, supervisor/restore/shutdown loop, configuration |
| `crates/agent/src/commands.rs`, `docker.rs`, `host.rs` | Bounded host commands (stderr excerpt on failure), Docker inventory, hardware/interface inspection, quota readiness and disk budget |
| `crates/agent/src/runtime.rs` | Registry, install/start/stop/kill/delete, console, crash policy, extension downloads |
| `crates/agent/src/engine.rs` | Docker Engine API over the socket: stdin attach and ordered bounded logs |
| `crates/agent/src/storage.rs` | Loop-image quota volumes, mount checks, disk budget, instance lock |
| `crates/agent/src/files.rs` | setfsuid-scoped, no-symlink file operations, properties transforms, game detection, ownership fix-up |
| `crates/core/src/lifecycle.rs` | Crash restart policy and recognised exit causes |
| `crates/core/src/schedule.rs` | Weekly recurrences in IANA zones (jiff, system tzdb) |
| `crates/api/src/schedules.rs` | Schedule store, once-per-occurrence claiming, scheduler loop |
| `crates/api/src/experimental.rs`, `http_experimental_routes.rs` | Bundled experimental eggs, cached GitHub status |
| `crates/api/src/http_backup_routes.rs`, `http_file_routes.rs`, `http_settings_routes.rs` | Backups/imports, file manager, settings/limits/members/schedules/update routes |
| `crates/agent/src/runtime_backups.rs`, `runtime_files.rs`, `runtime_update.rs` | Backup/restore/import, file manager operations, updates with rollback |
| `crates/agent/src/backups.rs`, `file_manager.rs`, `metrics.rs`, `firewall.rs` | Archive write/verify/extract/swap, openat2 file manager, stats parsing, isolation rules |
| `eggs/experimental/` | Pumpkin (upstream) and SteelMC (written here) eggs with provenance |
| `crates/api/src/agent_client.rs` | Peer-UID-checked Unix-socket client: bounded frames, 36-second exchange deadline, request/version matching, snapshot freshness (`HostAdmission::is_fresh`, 30 seconds, future timestamps rejected) |
| `web/src/App.svelte`, `app.css`, `components/Icon.svelte` | Initial Codex-style responsive UI; no copied OpenAI assets |
| `web/src/api.ts` | Same-origin fetch, response/error handling and compatibility details |
| `web/src/pages/CreateServer.svelte` | Creation page driven by `/api/host` and `/api/allocations` |
| `web/src/pages/ServerWorkspace.svelte` | Server workspace |
| `deploy/`, `docs/operations/deployment.md` | systemd units, nginx template, runbook |
| `tests/host/` | Acceptance helpers: browser acceptance, real client join (`client-join.cjs`), Debian userland run (`debian-userland.sh`), agent CLI client, test-only TLS terminator and egress forwarder |
| `tests/fixtures/eggs` | MIT-licensed upstream fixtures, source revision/hashes/license |

All implemented non-trivial slices were developed with failing tests first, then observed passing tests. Avoid large new abstractions or infrastructure; use the existing boundaries and lockfiles.

## Current API surface and limits

- `POST /api/login`, `/api/register`, `/api/logout`, `/api/invites`; `GET /api/session`.
- `GET/POST /api/eggs`, `POST /api/eggs/{id}/revisions`, `GET /api/eggs/revisions/{id}`.
- `GET /api/servers`, `GET /api/servers/{id}`, `PUT /api/servers/{id}/members/{user_id}`.
- `GET /api/host`, `GET/POST /api/allocations` (POST admin-only), `POST /api/servers`. All authenticate first, then ask the agent for a fresh snapshot per request (never cached).
- `GET/DELETE /api/servers/{id}` (delete needs `{"confirm_name"}`), `POST /api/servers/{id}/power` (`start|stop|restart|kill`), `GET/POST /api/servers/{id}/console`, `POST /api/servers/{id}/reinstall` (failed installs only), `GET /api/servers/{id}/activity`, `GET/POST /api/servers/{id}/extensions`, `DELETE /api/servers/{id}/extensions/{project}`, `GET /api/servers/{id}/modrinth?query=`, `POST /api/eggs/revisions/{id}/qualification` (admin).
- Without `GM_AGENT_SOCKET`/`GM_AGENT_UID`, or when the agent fails/replies untrustworthily, host routes return **503 host_unavailable**. Agent blockers (quotas, unqualified runtime) return **503 host_not_ready** from creation. Capacity → 409 `capacity_exceeded` (per resource); taken/foreign allocation → 409 `allocation_unavailable`; bad variable → 400 `invalid_variable` with the egg rule text.
- Creation reserves resources and queues an install job that the API worker runs through the agent. Agent rejections reach users as 409s with the agent's message (`port_in_use`, `server_running`, …).
- `GET /api/host` returns budget/reserved/remaining/hardware/blockers but not private IPs or occupied port bindings.
- Egg `runtime_qualified` is true only after an administrator marks the revision qualified. Successful parsing alone is not runtime compatibility.
- Server list `usage` comes from the agent (CPU % with 100 % = one CPU, memory without page cache, disk used); fields are null when unknown (for example CPU/RAM of a stopped server). No generated readings.
- Also: `GET/PUT /api/servers/{id}/settings`, `PUT /api/servers/{id}/limits`, `GET /api/servers/{id}/members`, `GET /api/users`, `GET/POST /api/servers/{id}/schedules`, `PUT/DELETE /api/servers/{id}/schedules/{schedule}`, `POST /api/servers/{id}/update`, `POST /api/servers/{id}/import` (streamed `.tar.gz`), backups (`/backups`, `/retention`, `/restore`, `/download`), files (`/files`, `/files/text`, `/download`, `/upload`, `/folder`, `/rename`), `GET /api/retained-backups`, `GET /api/experimental`, `POST /api/experimental/refresh`, `POST /api/experimental/{project}/egg`.
- New invitations last 24 hours; sessions seven days. Usernames: 3–32 ASCII letters/digits/underscores/hyphens. Passwords: 12–1024 bytes.
- Browser session cookie: `__Host-gm_session`, Secure, HttpOnly, SameSite=Strict, Path=/. Mutating requests require exact configured Origin.
- Login/registration share a bounded process-wide budget of 10 attempts/minute; four concurrent blocking auth/database workers. This deliberately simple small-team limit may need per-peer refinement later.
- API body limit 16 KiB; egg upload routes allow 1 MiB. Unknown egg fields/semantics fail closed; current supported rule/parser subset is intentionally limited.
- CLI API binds loopback only; `GM_ORIGIN` must be a bare HTTPS origin. Frontend dev HTTP alone does not establish working secure-cookie browser authentication. Do not weaken cookie/TLS checks to make a preview appear complete.

## Latest completed milestone

Goal session continued, 2026-10-28 (same Ubuntu 24.04 container with Docker 29.9; the container restarted once mid-session and everything came back up). Commits `46f2521` → `1b6d0c6` → `efc8a5d` and the documentation commit after them.

- **Settings, limits, metrics, members** (`46f2521`, `426b10b`): verified live on the Carpet Bots server.
  - Memory 3 → 3.5 GiB, disk 4 → 5 GiB (offline resize to a 5.2 GB filesystem at the next start).
  - Image switch to Java 21 and back, with the image re-pinned.
  - Usage reported (CPU millicores, memory).
  - A Carpet bot joined afterwards.
  - The Java 21 switch made Minecraft 26.3 crash; that led to explained, non-retried crash causes.
- **Editions** (`1b6d0c6`): real installs and starts, then deletion:
  - Vanilla 26.3.
  - Paper 26.3 with LuckPerms (Bukkit build). Modrinth reports plugins as `mod` projects, which used to be refused.
  - Forge 26.3 with No Chat Reports.
  - NeoForge 26.3 with FerriteCore, whose loading was confirmed in the log.
  - Bedrock 1.26.60 over UDP (`list` answered).
- **Updates and schedules:**
  - Manual update of a running server: pre-update backup, installer re-run, "same game version", restarted, Carpet kept.
  - An update interrupted by restarting the agent was put back from the pre-update backup at boot.
  - A nonexistent version (the Fabric installer exits 0 but deletes `server.jar`) is now rolled back with an explanation; before the fix this left an unstartable server.
  - Restores re-detect the game version.
  - A scheduled console command fired at the next minute, and its next run moved 24 h ahead.
- **Experimental:**
  - Pumpkin 0.2.1 (for 26.3) and SteelMC nightly installed through their eggs from builds extracted from their official images (served locally; github.com downloads are blocked in this sandbox), and ran.
  - Pumpkin needs its Bedrock listener off on hosts without IPv6; this is explained when it happens.
  - SteelMC needed `^C` (SIGINT) to stop, because its console needs a terminal.
- **Network isolation:** before the rules, a container reached a host service on the bridge gateway; after, it could not, while the egress proxy and a full Vanilla install kept working.
- **Import:** a Pterodactyl-style archive (`./` entries, files owned by UID 988) was imported, verified and restored into a new Fabric server. The existing world loaded (`Done (1.281s)`) with Carpet, and an invalid upload was refused with 422.
- **Real client:** mineflayer 4.39 (protocol for 26.1) joined a Vanilla 26.1 server through the published port, spawned, chatted and was logged by the server.
- **Bedrock client: not verified.**
  - Bedrock 1.26.60 defaults to `transport=nethernet`, which opens one dual-stack IPv6 socket. `transport=raknet` also needs IPv6 (port 19133).
  - This sandbox kernel has no IPv6 (`/proc/sys/net/ipv6` is missing), so the server either listens only on its LAN discovery port (7551) or exits.
  - Check this on the Debian host with `tests/host/client-join.cjs bedrock-ping` and `bedrock`.
- **Debian 13 userland** (`tests/host/debian-userland.sh`): 28 agent host tests passed in a privileged `debian:trixie` container, using Debian's e2fsprogs 1.47.2, util-linux and iptables 1.8.11. They covered:
  - Quota volume, ENOSPC and offline resize.
  - Archives, file manager, file identity, socket permissions and firewall rules.

  `manager-agent inspect-host` ran there. This is not the target host: its kernel, Docker and data are this sandbox's.
- Checks on the final tree are listed under "Test evidence".

### Earlier in this goal session

Goal session, 2026-10-27/28 (Ubuntu 24.04 container with Docker 29.9 and loop devices, Rust 1.99.0, Node 22):

- Agent game runner, per-server quota volumes and file identity isolation (`99927fd`); console/stdin, instance lock and egress trust fixes found by running the real Fabric egg (`8261147`); API worker, runtime routes and Modrinth (`fcf4bed`); server workspace and static serving (`3081be7`); browser acceptance and the UI fixes it found (`2465aab`); socket access for the non-root API (`40345b4`).
- **Acceptance passed** with `tests/host/fabric-carpet-acceptance.cjs`: agent as root, API as a dedicated non-root user, browser (headless Chromium) over HTTPS through `tests/host/tls-proxy.py`. Install of the unmodified upstream Fabric egg (Minecraft 26.3, detected from files) took ~30 s; Carpet 26.3 installed from Modrinth with SHA-512 verification; readiness detected; `player Bot_One/Two/Three spawn` sent from the console; `list` reported `There are 3 of a max of 20 players online`; graceful stop saved the worlds and exited 0. Carpet logs Mojang profile-lookup warnings for bot names without accounts; that is expected.
- Also verified directly against the agent: graceful host shutdown (SIGTERM) stopped the running server and kept it desired-running; the next agent start restored it; a second agent refuses the same data root; forced stop after the grace period is recorded; hard quota refuses writes past the limit.
- Edge cases found by real runs and fixed: `docker create -i` closed stdin after the first command (now `run --detach`); two agents on one data root; Netty needing exec on `/tmp`; TLS-inspecting egress; quick console commands dropped by the browser's implicit-submit rule; unreadable root-owned socket for a non-root API; API ignoring SIGTERM.
- Final checks on that tree: `cargo test --workspace` **74 passed, 0 failed, 3 ignored** (opt-in host tests); opt-in real-Docker agent tests (`docker_engine`, `runtime_docker`) **2 passed**; fmt and clippy clean; `npm --prefix web test -- --run` **8 passed**; `npm --prefix web run check` 0 errors/0 warnings; build succeeded.
- Environment notes (not product behaviour): this sandbox needed a Docker Hub mirror (anonymous pulls hit 429), `iproute2`, a CONNECT-only egress proxy reached through `tests/host/egress-forwarder.py`, and a readable copy of its proxy CA for the non-root API.

### Earlier cloud milestone (2026-10-27)

Cloud session continuation, 2026-10-27 (Ubuntu 24.04 container, Rust 1.99.0 exactly as pinned, Node 22):

- `8b0cd49`: implemented `AgentClient::inspect_host()` (the former `todo!()`). The three Linux transport tests were first observed failing on the stub, then passing. The 30-second freshness rule moved to `HostAdmission::is_fresh` and is shared with provisioning.
- `0760c84`: host-backed HTTP routes listed above, `GM_AGENT_SOCKET`/`GM_AGENT_UID` configuration in `manager-api`. New Linux test `crates/api/tests/http_provisioning.rs` (fake agent on a real Unix socket) failed first, then passed: unconfigured agent, capacity overview, admin-only allocation add/duplicate, capacity rejection, successful creation and owner access, allocation reuse rejection, agent blockers.
- `9e8e98b`: creation page replacing the static "prepare your host" panel. Three new UI tests failed first, then passed.
- Verified on the final tree in that container: `cargo test --workspace` **45 passed, 0 failed, 1 ignored** (the explicit real-host probe); `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean; `npm --prefix web test -- --run` **5 passed**; `npm --prefix web run check` 0 errors/0 warnings; `npm --prefix web run build` succeeded.
- Headless Chromium (Playwright) rendered the built creation page at 1280px and 390px against **mocked** API responses: no horizontal overflow, blockers/capacity/form/admin form legible. This is a layout check only, not a signed-in roundtrip with the real API/agent.
- Not done in that session: no access to the Debian host, so the real agent binary was not run against the new client, and no Linux host or Windows run was repeated. Docker was not available in the container.

### Earlier milestone (Codex session)

After the user clarified continuous work, implementation continued beyond the first handoff checkpoint:

- All stores now use one schema-versioned database initializer. Initialization is transactional, concurrent first connections are tested, existing accounts survive migration, and unknown newer schema versions fail closed.
- The creation transaction now validates the pinned egg/image/editable variables and required license acceptance, validates an available private-IP/transport/port allocation, totals all existing reservations plus legacy reservations, and commits server ownership, resource reservation, allocation claim and install job atomically.
- Concurrent creation tests prove only one request succeeds when two would exceed the budget. Stale snapshots, missing disk qualification, unqualified runtimes, invalid settings, foreign/public IPs and used ports are rejected. There are no production readiness overrides from browser input.
- Host snapshots must be no older than 30 seconds. Live socket/port and disk enforcement still require agent-side rechecks; database success alone is not a successful game install.
- **Current Windows Rust suite: 30 tests passed**; formatting and Clippy checks passed. The 2 existing frontend tests/build remain the previous verified UI snapshot, unchanged by this backend milestone. Current Linux integration has not yet been rerun.
- Next work is trusted agent host-status integration and the Docker game runner. Browser `POST /api/servers` remains gated until that trusted path is connected; no fake successful creation was added.

### Host-side integration — paused WIP

- Added bounded host command execution: stdout/stderr size limits, checked exit status, deadlines, child termination/reaping on failure, and hidden Windows helper processes. Three focused tests passed after failing first.
- Added Docker inventory parsing: preserves stopped-container allocations, counts uncapped legacy RAM/CPU conservatively, rounds CPU quotas upward, rejects malformed/negative data and invalid IDs, and excludes only explicitly owned container IDs from legacy reservations. Three focused tests passed after failing first.
- Shared host/admission/port types now live in `crates/core/src/host.rs`; browser creation input still cannot supply host readiness or budget overrides.
- Hardware parsing and configured-private-interface matching tests passed (2 tests). The read-only native Linux host qualification test was then observed failing on the stub and passing after implementation. It correctly detected hardware, Meshnet IP, legacy limits and stopped-container allocations. This does not establish game-runtime readiness.
- Implementation choice: use the host's installed Docker CLI as the Docker API client, with argument arrays (no host shell interpolation), fixed local socket and a separate empty CLI config path. This avoids another SDK dependency. The only new commands implemented so far are inspection commands; no game-control operations are enabled.
- The dedicated Linux source archive was refreshed for that qualification. Build containers had no Docker socket or legacy game-data mounts. The built test binary was then run natively with sudo to read hardware/Docker state. No legacy limits, ports, firewall rules, game files or services were changed.

### Former stopping point (resolved by the cloud continuation above)

- Last fully verified whole-workspace checkpoint: **244e969**, atomic admission/allocation milestone, 30 passing Windows Rust tests plus formatting/Clippy.
- Later focused checks passed: 3 command-bound tests, 3 Docker inventory tests, 2 hardware/interface parser tests, 1 reply-decoder test, existing 3 RPC tests, and 1 native read-only Linux host qualification.
- The newer complete source tree has **not** had a final full Windows/Linux/Clippy run. Do not combine focused counts into a claimed full-suite pass.
- **Known stub:** `AgentClient::inspect_host()` in `crates/api/src/agent_client.rs` still contains `todo!()`. It is not connected to HTTP routes. Finish it before enabling host-backed creation or running its new Linux transport tests.
- New Linux-only tests in `crates/api/tests/agent_transport.rs` are written but **not yet run**. They cover peer UID validation, request/reply transport, oversized responses and stale/future timestamps. First run should expose the missing client implementation.
- Agent dispatch now supports async InspectHost and the agent CLI has `inspect-host`; configuration uses `GM_DATA_ROOT` and `GM_PRIVATE_IPS`. These latest socket/configuration changes need fresh Linux acceptance beyond the earlier direct probe test.
- Shared host models moved to `crates/core/src/host.rs`; the API re-exports admission/port types for existing callers.
- The native qualification used a disposable data root under `/tmp`, which is tmpfs on this Debian host. Its disk figures therefore described that configured temporary filesystem, not the main root disk. Disk quota enforcement and persistent storage remain unqualified.
- Local Vite preview was stopped and the SSH session logged out during wrap-up. No API/agent service or running test container is intentionally left behind. The dedicated source/log/output directory and downloaded Rust image cache remain for continuation.
- Current changes are being saved as an explicit WIP checkpoint. Preserve them and consult `git log`; do not assume this WIP is a deployment-ready release.

## Host facts and blockers

See [host inventory](docs/operations/host-inventory.md), [storage/network qualification](docs/operations/storage-and-network.md), [capacity proposal](docs/operations/capacity-proposal.md), [compatibility](docs/operations/compatibility.md), and [Linux validation](docs/operations/linux-validation.md).

- SSH: `ssh stijn@server`; hostname failed here, fallback `ssh stijn@100.116.183.201` works. Credentials were supplied privately in the conversation; retrieve them securely if needed, never add them to this file. Known host key matched the existing hostname entry.
- Debian 13.6; Intel i5-9600K, 6 cores/threads; 25,075,331,072 memory bytes (~23.35 GiB).
- Existing services: CasaOS, Docker/containerd, Pterodactyl Panel/Wings/queue, Nginx/PHP, MariaDB, Redis, Cloudflare Tunnel, NordVPN, Samba, rclone and others.
- Four legacy game limits total **48.39 GiB**. Running game has ~20.5 GiB limit and observed ~2.5 GiB resident usage. Available RAM does not justify ignoring those reservations.
- Main ext4 filesystem had ~72 GiB free; no quota/project feature or quota mount flags were observed. The agent now enforces disk limits with fully allocated per-server ext4 loop images (no project quotas needed); this still needs a run on the host, and every server's full disk limit is allocated up front.
- Docker 29.7.2, containerd `overlayfs`, cgroup v2. Do not switch the existing storage driver: that can make legacy images/containers inaccessible.
- Confirmed Meshnet address `100.116.183.201` on `nordlynx`; LAN address `192.168.178.90`. Global IPv6 exists; verify private-only exposure for both address families.
- Existing game uses LAN TCP/UDP 25567; listeners include 80/8080/8085/2022. Preserve existing rules, including TCPShield. No public panel tunnel added.
- Proposed app locations/ports are in storage-and-network.md, not applied settings. Recheck availability before deployment.
- User said "do what you want" when asked about preparing lower limits. Decision made: document proposals and continue locally, leaving live limits unchanged pending workload evidence and migration rehearsal.
- No application service was installed and no existing game/service/firewall setting was changed. The existing game was confirmed still running after Linux tests.

## Test evidence and how to rerun

### Final tree of the 2026-10-28 session

Run in the Ubuntu 24.04 test container (Rust 1.99.0, Node 22, Docker 29.9):

| Check | Result |
| --- | --- |
| `cargo test --workspace` | 97 passed, 0 failed, 3 ignored (opt-in host tests) |
| `cargo test -p manager-agent --test runtime_docker --test docker_engine -- --ignored` (real Docker) | 2 passed |
| `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `npm --prefix web test -- --run` | 15 passed |
| `npm --prefix web run check` / `build` | 0 errors, 0 warnings / built |
| Agent host tests in `debian:trixie` (privileged; `tests/host/debian-userland.sh`) | 28 passed |
| Live runs (see the milestone) | Carpet bots; editions; updates and rollback; schedules; experimental servers; isolation; import; Java client join |

### Previously verified Linux snapshot

At the agent-foundation snapshot (commit `c25fc91` content), **25 Rust tests passed** on Debian in a disposable resource-capped build container. This snapshot predates the newest server metadata/UI work. Do not claim that the newest full tree has passed Linux tests until it is rerun.

Linux build container: official Rust image, one CPU, 1 GiB RAM/no extra swap, 256 PIDs, UID/GID 1000, capabilities dropped, no-new-privileges, no published ports/Docker socket/legacy data. Read-only project bind; bounded tmpfs cache/build. Temporary container removed on exit.

Full debug-symbol build initially hit a linker SIGBUS. The compact run passed with unchanged caps using `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`, `CARGO_INCREMENTAL=0`, and `--jobs 1`; final /build use 446 MiB of 768 MiB. Logs and source archive remain under `/home/stijn/game-manager-check.pQyDcja9`. The source archive is an older snapshot: **recopy current source before reusing it**. The downloaded Rust image remains cached.

### Current local checks

Use these commands from the root. Read the actual output; do not infer success from old results:

```powershell
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
npm --prefix web ci
npm --prefix web test -- --run
npm --prefix web run check
npm --prefix web run build
```

Earlier UI/server-metadata checkpoint verification on 2026-10-27 (`3d0eadd`; historical evidence, not a final test of the paused WIP):

| Check | Result |
| --- | --- |
| `cargo test --workspace` | PASS: 25 current Windows tests; two Linux-only tests correctly skipped |
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `npm --prefix web test -- --run` | PASS: 2 UI component tests |
| `npm --prefix web run check` | PASS: 0 errors, 0 warnings |
| `npm --prefix web run build` | PASS: production assets built |

Sandbox hard-link cache warnings cause copying rather than compilation failure. The earlier Linux snapshot also passed 25 tests, but it is a different source snapshot; do not confuse the identical totals. Current server metadata/HTTP/UI changes still need a fresh Linux/browser integration run.

### Local startup / browser status

```powershell
$env:GM_DATABASE = 'game-manager.sqlite'
cargo run -p manager-api -- init-admin YOUR_USERNAME
$env:GM_ORIGIN = 'https://YOUR_PRIVATE_NAME:8443'
cargo run -p manager-api
npm --prefix web run dev -- --port 5173 --strictPort
```

Admin bootstrap uses a hidden password prompt; do not pass passwords in command arguments. `GM_BIND` defaults to loopback `127.0.0.1:19090`.

The Vite preview had been started on **http://127.0.0.1:5173**. Both the original sandboxed instance and its replacement were stopped; the replacement was stopped during the user's requested wrap-up. Restart it explicitly when resuming browser testing. It was only a local frontend dev server, not deployment.

Brave test tab initially displayed a connection timeout before the restart; it has not yet been reloaded and visually verified after the restart. Actual browser QA is **pending**. There is no live signed-in browser session or seeded real server list. A new agent can open a fresh tab at the URL; ephemeral old tab identifiers must not be assumed valid.

The development proxy targets API port 19090. No API process/private HTTPS proxy was left running. Production static serving/private TLS and a real-browser signed-in roundtrip are still to be wired. Do not bypass certificate warnings or change Windows security settings for testing; use a proper private certificate/trust setup or clearly separate test-only visual fixtures.

## Next actions for Claude / the next agent

1. Read this file, design.md and implementation-plan.md, then check `git status` and commits.
2. Deploy to the Debian host with [deployment.md](docs/operations/deployment.md) (no legacy changes):
   - Run `tests/host/debian-userland.sh`'s checks natively.
   - Run `tests/host/fabric-carpet-acceptance.cjs`.
   - Join with `tests/host/client-join.cjs` for Java and Bedrock (Bedrock needs IPv6 in the kernel), over Meshnet.
   - Expect creation to be blocked until the legacy limits leave budget (the agent reports it).
3. Agree lower legacy limits or a migration window with the user. Migrate one legacy server at a time with the import flow in the runbook. Keep the Pterodactyl original stopped but intact until the new one is verified, and never run both on the same world.
4. Arrange an independent security review of the whole branch before exposing it to teammates (agent runs as root; eggs run untrusted scripts in hardened containers).
5. Smaller follow-ups:
   - Bedrock version detection (the version is only in the console).
   - Choosing a specific published build for updates.
   - An admin-wide audit view.
   - yaml/json/ini egg transforms.
   - WebSocket console (optional).

## Decisions/deviations carried forward

- Work in current folder on `codex/game-server-manager`, as explicitly selected by the user. No worktree required.
- Schema/modules are created with their owning features instead of scaffolding every future table up front. Remaining schema is not missing by accident.
- rusqlite with bundled SQLite fits the single-host workload. Keep blocking DB/password work off Tokio's async worker threads; HTTP does this with bounded spawn_blocking tasks.
- Operation leases do not provide exactly-once Docker effects. Add agent operation checkpoints before running mutating jobs.
- Native Windows checks cover domain/database/UI behavior; Linux checks cover actual socket behavior. They do not replace Docker/game/backup/reboot acceptance.
- Initial egg compatibility is deliberately a subset: properties transforms and required/nullable/string/max/in/terminal regex rules; supported fixture feature flags only. Unknown/repeated/nonterminal validation semantics are rejected rather than silently ignored.
- Preserve JSON image ordering because the first image is the default. Keep fixture provenance/license/hash records when expanding adapters.
- Authentication defaults and simple global throttling are assistant choices recorded above. Refine only if a real need emerges; do not silently remove security for development convenience.
- Browser shell work began before game-runtime integration to give the approved Codex UI a concrete implementation against already-tested endpoints. It must not imply working game control.
- Manual PowerShell bookkeeping was used because skill Bash helpers were unavailable. The ignored `.superpowers/sdd/2026-10-27-game-server-manager/progress.md` contains historical notes, but this **tracked file is the portable handoff authority**. No next agent should depend on that scratch directory.
- No whole-branch independent code review has run yet. Arrange one before deployment; current tests are not a substitute for security/recovery review.
- Docker CLI with argument arrays for container lifecycle; the Engine API over the socket only where the CLI cannot work safely (stdin attach, bounded ordered logs). Game containers are started with `docker run --detach --interactive` because `create -i` sets StdinOnce.
- Disk quotas are fully allocated ext4 loop images per server (works without project quotas, contains hard links). Each server has its own UID from `GM_GAME_UID_BASE`; agent file work uses `setfsuid` to that UID and refuses symlinks.
- Runtime qualification is an administrator attestation stored per immutable revision; administrators may test-install unqualified revisions.
- Console uses polling (2 s) instead of WebSocket; every poll re-checks access, which satisfies stream revocation.
- Network isolation is on by default: chains `GM-ISOLATE-IN` (INPUT) and `GM-ISOLATE-FWD` (DOCKER-USER) apply only to the game subnet. They are rebuilt at agent start and before installs/starts; `GM_NETWORK_ISOLATION=off` disables them. This changes the host firewall only for the managed subnet; mention it when agreeing the Debian deployment.
- Updates are "run the egg installer again" (Pterodactyl's reinstall semantics) behind a verified pre-update backup. Rolling back is automatic only before the updated game has run. After it has run, a crash is explained and the pre-update backup is offered, but the world is never rolled back silently (design: no silent rollback of an upgraded world).
- Scheduled updates keep the detected game version (checked before and after), pause the schedule after a failed update, and are refused where the version cannot be read reliably (Bedrock).
- Imports reuse backups: an uploaded `.tar.gz` becomes an `imported` backup, so restoring uses the same checksum, pre-restore copy and staged swap. Metadata (owners, members, schedules, limits) is not imported; set it by hand during each reviewed migration.
- Experimental eggs are bundled with the API: Pumpkin's unmodified upstream egg and a SteelMC egg written here. They are added to the library only on request and stay unqualified until an administrator test-installs them.
- Time zones use `jiff` with the host's tz database (`/usr/share/zoneinfo`, standard on Debian).

## Commit checkpoints

- `3625d81`: approved design and host findings.
- `298ea46`: job leases/recovery and core policies.
- `a13153f`: invite-only authentication and private-origin API.
- `957b89b`: strict shared egg imports, immutable revisions, pinned fixtures.
- `c25fc91`: Linux peer-authenticated agent health protocol and Linux validation evidence.
- `3d0eadd`: server metadata/access APIs, initial Codex-style UI, and root-level handoff documents.
- `244e969`: shared versioned database initialization and atomic resource/allocation/server/install-job creation.
- `f03c916`: WIP host inspection (bounded commands, Docker inventory, hardware/interface parsing) with the client stub.
- `8b0cd49`: agent client transport. `0760c84`: host-backed HTTP creation/allocations/overview. `9e8e98b`: creation page.
- `d2415bc`: Fabric egg support. `99927fd`: agent game runner and quota storage. `8261147`: stdin/lock/egress fixes. `fcf4bed`: API runtime and Modrinth. `3081be7`: workspace and static serving. `2465aab`: browser acceptance. Later: socket group grant, SIGTERM handling, deployment templates.

- `46f2521`: settings, limits, usage metrics, members. `426b10b`: explained crash causes, Vanilla/Forge/NeoForge fixtures. `1b6d0c6`: editions, schedules, updates. `efc8a5d`: experimental servers, network isolation, imports, update hardening.

Use `git log` and `git status` for the final current commit; documentation-only follow-ups may be newer than the implementation checkpoints above.

## Suggested continuation prompt

> Continue this repository from progression.md. Read design.md and implementation-plan.md, inspect git status, and preserve existing tested work. Build the approved game manager in Rust/Svelte with a Codex-style UI. Start with the next incomplete integration task; do not install or migrate over existing Pterodactyl/CasaOS services. Treat host resource/quota issues and missing game/backup acceptance as real blockers to deployment. Keep progression.md updated with verified evidence and remaining work.
