# Project progression and continuation handoff

Last updated: 2026-10-27 (Europe/Amsterdam).

**Resumed 2026-10-27** in a Claude Code cloud session ("look at the whole repo and continue") on branch `claude/ecstatic-allen-u2m477`, which carries all earlier `codex/game-server-manager` commits. The WIP checkpoint `f03c916` was finished and three milestones were added; see "Latest completed milestone". Keep maintaining this file continuously without treating each update as a request to stop.

## Read this first

This project is **at the foundation and initial UI stage**. It cannot install, start, stop, or manage real game servers yet. Passing tests below cover implemented pieces, not the whole approved product. Do not tell the user it is deployment-ready.

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
| 1. Host qualification | Partial; evidence recorded | Read-only inventory, real Meshnet confirmation, CPU/RAM/storage/legacy limits, Docker/network/service inspection, external contract research, capacity proposal | Enforceable disk quota layout, justified live resource budget, private TLS trust and friend-peer browser tests, runtime egg compatibility |
| 2. Foundation | Built for current operations | UUID models, durable SQLite leases, exclusive job claiming, renewal, stale-worker fencing, recovery with same operation ID; typed bounded RPC; Linux UID/socket checks on both agent and API client sides; API-to-agent InspectHost integration | Host-action idempotency/checkpoints when mutating Docker operations are added; remaining schema with owning features; a real agent-binary/API-client roundtrip on the Debian host |
| 3. Accounts and permissions | Partial, substantial | Bootstrap admin CLI, Argon2id hashes, single-use expiring invites, sessions/logout, origin checks/cookies/throttling; persisted owner/member authorization and dashboard/detail separation | Ownership transfer/account lifecycle, complete endpoint coverage, stream revocation once streams exist, full audit integration |
| 4. Shared egg library | Partial, substantial | Pinned real Java/Bedrock fixtures; strict parser and variable checks; shared HTTP import/list/details; immutable revisions, original JSON/hash retained; importer/admin edit rules | More rule/parser adapters, unused-revision deletion and server revision FK integration, runtime qualification, script/image execution |
| 5. Capacity/allocations | Partial; HTTP path connected | Checked CPU/RAM/disk rules; atomic server/owner/reservation/port/install-job creation; pinned egg revision and validated settings; admin-only private-IP allocation pool; stale/unqualified host rejection; race/rollback tests; HTTP creation, allocation pool and capacity overview backed by a fresh agent snapshot per request | Host presets, actual quota enforcement and qualification (agent still reports `disk_quotas_ready: false`, `disk_bytes` budget 0 and no qualified revisions), allocation edits/removal and limit changes |
| 6. First Java game | Pending | No Docker game operations | Installer/runtime isolation, image digest pinning, game license acceptance, installation, start/readiness, operation journal, first real game |
| 7. Console/lifecycle/metrics | Pending | Transport boundary only; resource usage explicitly unknown | Live console/logs/commands, actual metrics, state reconciliation, crash limits, boot restoration, graceful game/host shutdown and systemd ordering |
| 8. Browser UI | Partial | Codex-style login/invite registration, sidebar, server list/access presentation, shared library import/details, admin invite page; host-backed creation page with capacity explanation, blockers, egg/image/variable/allocation/limit/license form and admin address form; UI tests/type checks/build; headless-Chromium layout check of the creation page against mocked API data | Signed-in real-browser QA against a running API, production static serving/private TLS, server workspace (settings/console/files/other feature views) |
| 9. Backups/deletion | Partial | Pure retention policy: default 3, range 1–5, calculate excess only after success | Archive creation/verification/download/restore, durable recovery, space accounting, rotation, confirmed deletion/retained backup ACLs |
| 10. Schedules/settings/activity | Pending | No scheduler or audit log | Durable scheduled occurrences, timezone handling, validated startup edits, redacted audit events and UI |
| 11. Minecraft/Modrinth | Research + parser fixtures only | Java/Paper and Bedrock egg parsing tests; Modrinth API research | Actual edition/loader installs, version detection, dependency resolution, stopped-server extension install, client connection checks |
| 12. Updates | Pending | Policy agreed only | Version selection, safe manual/automatic workflows, backups/checkpoints/readiness, failed-release handling and recovery UI |
| 13. Experimental implementations | Research only | Official Pumpkin/Steel sources identified | Install adapters/build selection, cached GitHub metadata, sourced feature/milestone display, stale/rate-limit behavior, no fabricated readiness |
| 14. Deployment/migration | Pending | Host evidence and Linux test run documented | Debian install templates/runbooks, private HTTPS, full staging acceptance, disposable migration rehearsal, per-server reviewed cutover/rollback |

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
| `crates/api/src/servers.rs` | Metadata list/detail authorization and member toggles; no provisioning |
| `crates/api/src/provisioning.rs` | Atomic creation/reservations/allocation pool against trusted host state; reservation totals and free-allocation listing |
| `crates/api/src/database.rs` | Shared atomic schema initialization, foreign keys, busy timeout and schema-version downgrade protection |
| `crates/api/src/http.rs` | Authenticated API routes, origin/cookie/body/worker limits, library/server handlers; `router` (no agent, fails closed) and `router_with_agent` |
| `crates/api/src/main.rs`, `config.rs` | Local CLI/bootstrap/startup validation; fixed loopback listener behind expected HTTPS origin |
| `crates/api/migrations/0001_jobs.sql` through `0005_admission.sql` | Current feature-owned schema, including resource reservations, pinned provisioning records and port pool |
| `crates/protocol/src/lib.rs` | Version 1 request/response, 64 KiB frames, Ping and read-only InspectHost |
| `crates/agent/src/rpc.rs`, `linux.rs`, `main.rs` | Peer UID/socket protection; read-only host inspection dispatch; new configuration/dispatch changes still need fresh Linux socket acceptance |
| `crates/agent/src/commands.rs`, `docker.rs`, `host.rs` | Bounded host commands, Docker inventory, hardware/private-interface inspection; no mutating game operations |
| `crates/api/src/agent_client.rs` | Peer-UID-checked Unix-socket client: bounded frames, 36-second exchange deadline, request/version matching, snapshot freshness (`HostAdmission::is_fresh`, 30 seconds, future timestamps rejected) |
| `web/src/App.svelte`, `app.css`, `components/Icon.svelte` | Initial Codex-style responsive UI; no copied OpenAI assets |
| `web/src/api.ts` | Same-origin fetch, response/error handling and compatibility details |
| `web/src/pages/CreateServer.svelte` | Creation page driven by `/api/host` and `/api/allocations` |
| `tests/fixtures/eggs` | MIT-licensed upstream fixtures, source revision/hashes/license |

All implemented non-trivial slices were developed with failing tests first, then observed passing tests. Avoid large new abstractions or infrastructure; use the existing boundaries and lockfiles.

## Current API surface and limits

- `POST /api/login`, `/api/register`, `/api/logout`, `/api/invites`; `GET /api/session`.
- `GET/POST /api/eggs`, `POST /api/eggs/{id}/revisions`, `GET /api/eggs/revisions/{id}`.
- `GET /api/servers`, `GET /api/servers/{id}`, `PUT /api/servers/{id}/members/{user_id}`.
- `GET /api/host`, `GET/POST /api/allocations` (POST admin-only), `POST /api/servers`. All authenticate first, then ask the agent for a fresh snapshot per request (never cached).
- Without `GM_AGENT_SOCKET`/`GM_AGENT_UID`, or when the agent fails/replies untrustworthily, host routes return **503 host_unavailable**. Agent blockers (quotas, unqualified runtime) return **503 host_not_ready** from creation. Capacity → 409 `capacity_exceeded` (per resource); taken/foreign allocation → 409 `allocation_unavailable`; bad variable → 400 `invalid_variable` with the egg rule text.
- Successful creation reserves resources and queues an `install` job; **no worker runs it yet**, so nothing is installed or started. On the real host the agent still reports quotas/runtimes as unqualified, so creation remains refused there.
- `GET /api/host` returns budget/reserved/remaining/hardware/blockers but not private IPs or occupied port bindings.
- Egg `runtime_qualified` is **false**. Successful parsing is not runtime compatibility.
- Server list `usage` is **null** until the collector exists. No generated CPU/RAM/disk readings or sample servers are shipped.
- New invitations last 24 hours; sessions seven days. Usernames: 3–32 ASCII letters/digits/underscores/hyphens. Passwords: 12–1024 bytes.
- Browser session cookie: `__Host-gm_session`, Secure, HttpOnly, SameSite=Strict, Path=/. Mutating requests require exact configured Origin.
- Login/registration share a bounded process-wide budget of 10 attempts/minute; four concurrent blocking auth/database workers. This deliberately simple small-team limit may need per-peer refinement later.
- API body limit 16 KiB; egg upload routes allow 1 MiB. Unknown egg fields/semantics fail closed; current supported rule/parser subset is intentionally limited.
- CLI API binds loopback only; `GM_ORIGIN` must be a bare HTTPS origin. Frontend dev HTTP alone does not establish working secure-cookie browser authentication. Do not weaken cookie/TLS checks to make a preview appear complete.

## Latest completed milestone

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
- Main ext4 filesystem had ~72 GiB free; no quota/project feature or quota mount flags were observed. **No enforceable per-server storage path is qualified.** Do not call usage monitoring a hard quota.
- Docker 29.7.2, containerd `overlayfs`, cgroup v2. Do not switch the existing storage driver: that can make legacy images/containers inaccessible.
- Confirmed Meshnet address `100.116.183.201` on `nordlynx`; LAN address `192.168.178.90`. Global IPv6 exists; verify private-only exposure for both address families.
- Existing game uses LAN TCP/UDP 25567; listeners include 80/8080/8085/2022. Preserve existing rules, including TCPShield. No public panel tunnel added.
- Proposed app locations/ports are in storage-and-network.md, not applied settings. Recheck availability before deployment.
- User said "do what you want" when asked about preparing lower limits. Decision made: document proposals and continue locally, leaving live limits unchanged pending workload evidence and migration rehearsal.
- No application service was installed and no existing game/service/firewall setting was changed. The existing game was confirmed still running after Linux tests.

## Test evidence and how to rerun

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

1. Read this file, design.md and implementation-plan.md, then check `git status` and commits. Do not redo completed foundation work or host discovery unnecessarily.
2. On the Debian host, run the real agent (`GM_API_UID`, `GM_AGENT_SOCKET`, `GM_PRIVATE_IPS`, `GM_DATA_ROOT`) and the API with `GM_AGENT_SOCKET`/`GM_AGENT_UID`, then confirm `GET /api/host` shows the real budget and blockers through a signed-in browser over private HTTPS. Creation is expected to be refused there until quotas/runtimes are qualified.
3. Continue the Docker game runner behind the restricted agent and remaining allocation/limit-edit integration. Do not let browsers send arbitrary host paths, Docker IDs, mounts or commands. No privileged/host-network game or installer containers and no Docker socket in them. Restart the local preview for the pending desktop/mobile/browser checks once useful.
4. Prepare a concrete, reversible storage and capacity rollout. Existing memory limits over-reserve this host; quota enforcement needs a real supported layout. The capacity proposal is arithmetic, not proof that the existing games tolerate lower limits.
5. Prove one isolated Java installation/start/console/graceful-stop cycle before expanding to the rest. Add durable host-operation checkpoints and lifecycle recovery as actual operations are introduced.
6. Continue remaining planned tasks, with focused failure-first tests, safe restore checks and real Linux game acceptance. Keep progression.md current at each verified milestone.
7. Before any live migration/reboot or disruptive legacy change, present the concrete operation/recovery procedure and use an agreed window. Never point two managers at the same live world.

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

Use `git log` and `git status` for the final current commit; documentation-only follow-ups may be newer than the implementation checkpoints above.

## Suggested continuation prompt

> Continue this repository from progression.md. Read design.md and implementation-plan.md, inspect git status, and preserve existing tested work. Build the approved game manager in Rust/Svelte with a Codex-style UI. Start with the next incomplete integration task; do not install or migrate over existing Pterodactyl/CasaOS services. Treat host resource/quota issues and missing game/backup acceptance as real blockers to deployment. Keep progression.md updated with verified evidence and remaining work.
