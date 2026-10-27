# Project progression and continuation handoff

Last updated: 2026-10-27 (Europe/Berlin).

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
| 2. Foundation | Built for current operations | UUID models, durable SQLite leases, exclusive job claiming, renewal, stale-worker fencing, recovery with same operation ID; typed bounded RPC; Linux UID/socket checks | Host-action idempotency/checkpoints when mutating Docker operations are added; remaining schema with owning features; API-to-agent integration |
| 3. Accounts and permissions | Partial, substantial | Bootstrap admin CLI, Argon2id hashes, single-use expiring invites, sessions/logout, origin checks/cookies/throttling; persisted owner/member authorization and dashboard/detail separation | Ownership transfer/account lifecycle, complete endpoint coverage, stream revocation once streams exist, full audit integration |
| 4. Shared egg library | Partial, substantial | Pinned real Java/Bedrock fixtures; strict parser and variable checks; shared HTTP import/list/details; immutable revisions, original JSON/hash retained; importer/admin edit rules | More rule/parser adapters, unused-revision deletion and server revision FK integration, runtime qualification, script/image execution |
| 5. Capacity/allocations | Partial | Pure checked CPU/RAM/disk admission rules and tests; safe host capacity proposal | Transactional real reservations, host presets, quota enforcement, IP/port pools/conflicts, creation admission and limit changes |
| 6. First Java game | Pending | No Docker game operations | Installer/runtime isolation, image digest pinning, game license acceptance, installation, start/readiness, operation journal, first real game |
| 7. Console/lifecycle/metrics | Pending | Transport boundary only; resource usage explicitly unknown | Live console/logs/commands, actual metrics, state reconciliation, crash limits, boot restoration, graceful game/host shutdown and systemd ordering |
| 8. Browser UI | Partial | Codex-style login/invite registration, sidebar, server list/access presentation, shared library import/details, admin invite page; UI tests/type checks/build | Real-browser QA, mobile inspection, production static serving/private TLS, real creation/settings/console/files/other feature views |
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
| `crates/api/src/http.rs` | Authenticated API routes, origin/cookie/body/worker limits, library/server handlers |
| `crates/api/src/main.rs`, `config.rs` | Local CLI/bootstrap/startup validation; fixed loopback listener behind expected HTTPS origin |
| `crates/api/migrations/0001_jobs.sql` through `0004_servers.sql` | Current feature-owned schema; no complete deployment/migration engine yet |
| `crates/protocol/src/lib.rs` | Version 1 request/response, 64 KiB frames, currently Ping only |
| `crates/agent/src/rpc.rs`, `linux.rs`, `main.rs` | Protocol, 5-second request bound, peer UID, owned non-writable socket parent, mode 0660, bounded listener; Linux agent has no Docker commands |
| `web/src/App.svelte`, `app.css`, `components/Icon.svelte` | Initial Codex-style responsive UI; no copied OpenAI assets |
| `web/src/api.ts` | Same-origin fetch, response/error handling and compatibility details |
| `tests/fixtures/eggs` | MIT-licensed upstream fixtures, source revision/hashes/license |

All implemented non-trivial slices were developed with failing tests first, then observed passing tests. Avoid large new abstractions or infrastructure; use the existing boundaries and lockfiles.

## Current API surface and limits

- `POST /api/login`, `/api/register`, `/api/logout`, `/api/invites`; `GET /api/session`.
- `GET/POST /api/eggs`, `POST /api/eggs/{id}/revisions`, `GET /api/eggs/revisions/{id}`.
- `GET /api/servers`, `GET /api/servers/{id}`, `PUT /api/servers/{id}/members/{user_id}`.
- `POST /api/servers` intentionally returns **503 host_not_ready** after authentication. It does not create metadata or pretend to install a game.
- Egg `runtime_qualified` is **false**. Successful parsing is not runtime compatibility.
- Server list `usage` is **null** until the collector exists. No generated CPU/RAM/disk readings or sample servers are shipped.
- New invitations last 24 hours; sessions seven days. Usernames: 3–32 ASCII letters/digits/underscores/hyphens. Passwords: 12–1024 bytes.
- Browser session cookie: `__Host-gm_session`, Secure, HttpOnly, SameSite=Strict, Path=/. Mutating requests require exact configured Origin.
- Login/registration share a bounded process-wide budget of 10 attempts/minute; four concurrent blocking auth/database workers. This deliberately simple small-team limit may need per-peer refinement later.
- API body limit 16 KiB; egg upload routes allow 1 MiB. Unknown egg fields/semantics fail closed; current supported rule/parser subset is intentionally limited.
- CLI API binds loopback only; `GM_ORIGIN` must be a bare HTTPS origin. Frontend dev HTTP alone does not establish working secure-cookie browser authentication. Do not weaken cookie/TLS checks to make a preview appear complete.

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

Fresh handoff verification on 2026-10-27:

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

The Vite preview was started on **http://127.0.0.1:5173**. The sandboxed instance could not be reached from Brave and was stopped. A replacement outside that network sandbox is currently running (Codex exec session 53095 at this handoff). It is only a local frontend dev server, not deployment.

Brave test tab initially displayed a connection timeout before the restart; it has not yet been reloaded and visually verified after the restart. Actual browser QA is **pending**. There is no live signed-in browser session or seeded real server list. A new agent can open a fresh tab at the URL; ephemeral old tab identifiers must not be assumed valid.

The development proxy targets API port 19090. No API process/private HTTPS proxy was left running. Production static serving/private TLS and a real-browser signed-in roundtrip are still to be wired. Do not bypass certificate warnings or change Windows security settings for testing; use a proper private certificate/trust setup or clearly separate test-only visual fixtures.

## Next actions for Claude / the next agent

1. Read this file, design.md and implementation-plan.md. Check `git status`, current commits and any newer edits before modifying files. Do not restart completed foundation work or rerun all discovery unnecessarily.
2. Finish/checkpoint current server metadata/UI work. Run fresh local checks. Reload/open the preview, inspect desktop and narrow layouts, and test real-browser flows when a correctly configured local/private HTTPS path exists. Keep sample visual fixtures explicitly test-only, if used.
3. Complete real capacity/reservation and allocation handling, then the Docker game runner behind the restricted agent. Do not let browsers send arbitrary host paths, Docker IDs, mounts or commands. No privileged/host-network game or installer containers and no Docker socket in them.
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

Use `git log` and `git status` for the final current commit; documentation-only follow-ups may be newer than the implementation checkpoints above.

## Suggested continuation prompt

> Continue this repository from progression.md. Read design.md and implementation-plan.md, inspect git status, and preserve existing tested work. Build the approved game manager in Rust/Svelte with a Codex-style UI. Start with the next incomplete integration task; do not install or migrate over existing Pterodactyl/CasaOS services. Treat host resource/quota issues and missing game/backup acceptance as real blockers to deployment. Keep progression.md updated with verified evidence and remaining work.
