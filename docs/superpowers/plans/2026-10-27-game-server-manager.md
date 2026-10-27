# Linux Game Server Manager Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Execution method is pending the user's choice.

**Goal:** Deliver the approved private, single-host game manager without disrupting existing Pterodactyl or CasaOS services.

**Architecture:** An unprivileged Rust web API serves static Svelte assets and stores metadata in SQLite. A separate Rust host agent controls only application-owned Docker containers and storage over a restricted local Unix socket. Durable jobs coordinate installs, backups, updates, and recovery.

**Tech Stack:** Rust, Axum, Tokio, SQLite, Svelte, TypeScript, Docker, systemd; choose supported dependency versions and commit lockfiles at implementation time.

**Spec:** `docs/superpowers/specs/2026-10-27-game-server-manager-design.md` (approved 2026-10-27).

## Global Constraints

- Initially manage one physical host. Debian is the primary target.
- No Pterodactyl Panel or Wings runtime dependency or copied implementation. Pterodactyl egg JSON files are an import format only.
- Everyone may import eggs into the shared library and use them without administrator approval.
- Without access, no server page, console, files, allocations, configuration, or controls are available.
- The administrator always retains full access. Teammates cannot revoke the owner's access.
- Enforce capacity admission checks instead of permitting over-allocation after a warning.
- Retention defaults to 3 per server; users may choose 1 through 5.
- Replace the oldest backup only after a new backup completes successfully.
- Manual updates are the default. Moving to another Minecraft version requires a manual action.
- 120 seconds grace per server with concurrent shutdown requests; at most 3 restart attempts in 10 minutes.
- No billing, public signup, multiple-host management, modpack installation, or automatic Minecraft-version migration in the first release.
- No secrets in source control. Existing runtime/installer images are allowed, subject to qualification.
- Read-only host discovery first. Do not reboot, repartition, reconfigure networking, change existing limits, or migrate live worlds as a discovery step.

## Review Focus

1. Simultaneous creation and scheduled work must not both spend the same remaining capacity (Task 5).
2. Revoked access must stop an already-open console/file download, not merely hide controls (Tasks 3, 7, 8).
3. A crash during backup/restore/update must preserve a known recovery path (Tasks 9, 12).
4. A manually stopped or repeatedly crashing server must not restart through another recovery mechanism (Task 7).
5. Missing upstream metadata must not become a fabricated compatibility or readiness claim (Tasks 11, 13).

## Repository and file responsibilities

The repository currently contains planning documents only. Create files as their tasks require them, not a speculative folder scaffold.

| Path | Responsibility |
| --- | --- |
| `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` | Rust workspace and pinned toolchain/dependencies |
| `crates/core/src/{lib,model,permissions,eggs,capacity,lifecycle,backup_policy}.rs` | Shared domain rules, no Docker or HTTP |
| `crates/api/src/{main,lib,state,auth,servers,eggs,jobs,schedules,minecraft,modrinth,experimental}.rs` | HTTP API, sessions, SQLite transactions, durable workflows |
| `crates/api/migrations/*.sql` | Incremental database schema |
| `crates/agent/src/{main,lib,rpc,docker,storage,files,backups,lifecycle}.rs` | Host operations; Linux-specific code stays here |
| `crates/protocol/src/lib.rs` | Typed local agent request/response messages |
| `web/src/{App.svelte,api.ts,app.css}` | Browser shell, API client, styles |
| `web/src/pages/*.svelte`, `web/src/components/*.svelte` | Feature screens created with their owning tasks |
| `crates/*/tests/*.rs`, `web/tests/*.test.ts` | Focused automated checks |
| `tests/fixtures/eggs/`, `tests/fixtures/upstream/` | Licensed, pinned examples and metadata fixtures |
| `tests/host/*.sh` | Explicitly invoked disposable-host smoke checks |
| `deploy/systemd/`, `deploy/nginx/` | Deployment templates, not automatically installed |
| `docs/operations/` | Discovery evidence, capacity/storage decisions, rollout and migration runbooks |

Use UUIDs for user/server/egg/job identifiers and integer bytes for storage/RAM. Model CPU in millicores. Use UTC timestamps in storage and label the displayed timezone. Errors use one shared structured code/message format; never return raw internal errors or secrets to clients.

## Task 1: Qualify the existing host and external contracts

**Files:** Create `docs/operations/host-inventory.md`, `docs/operations/compatibility.md`, `docs/operations/storage-and-network.md`.

**Interfaces:** Produces the concrete private bind address, unused ports, storage/quota mechanism, resource reserve calculation, Docker capabilities, supported egg fixture matrix, and Minecraft/Modrinth upstream API contracts for later tasks. This is an evidence gate, not application code.

- [ ] Inspect non-secret service configuration, network interfaces and VPN peers, firewall/nftables, Docker networks/storage driver, filesystem mount/quota support, legacy container limits and volumes, backup sizes, and shutdown ordering over SSH.
- [ ] Verify the actual private network: the 100.x SSH address is not proof of NordVPN Meshnet. Record peer reachability and how HTTPS trust will work on the chosen private hostname/address. Keep game allocations restricted to the same approved network by default.
- [ ] Inspect representative Java and Bedrock PTDL_v2 egg JSON and official Docker API/Modrinth documentation. Pin source URL/revision and license for each fixture; list unsupported features explicitly.
- [ ] Calculate conservative RAM, CPU, and disk budgets including stopped legacy servers. If no safe headroom remains, document the blocker and options without changing existing limits.
- [ ] Select enforceable per-server disk quotas. If the existing filesystem cannot support them safely, prepare a dedicated-volume proposal with rollback; do not create or repartition it during discovery.
- [ ] Verify the proposed paths and ports do not overlap legacy data or listeners. Record unknowns as blockers to host deployment, not guessed successful checks.
- [ ] Review the evidence against the approved spec and commit only the non-secret documents when repository metadata is writable. No application deployment in this task.

**Pass:** Every deployment-critical choice has observed evidence or an explicit blocker. Work on pure domain rules may continue while hardware/network blockers remain; provisioning may not.

## Task 2: Domain schema, durable jobs, and minimal service boundary

**Files:** Create Rust workspace manifests, `crates/core/src/{lib,model}.rs`, `crates/protocol/src/lib.rs`, `crates/api/src/{main,lib,state,jobs}.rs`, `crates/api/migrations/0001_initial.sql`, `crates/agent/src/{main,lib,rpc}.rs`, `crates/api/tests/job_recovery.rs`.

**Interfaces:** `ServerId`, `UserId`, `EggRevisionId`, `JobId` are UUID-backed IDs. `ServerState` covers stopped/installing/starting/running/stopping/failed. `JobKind` covers install, backup, restore, update, delete. `AgentRequest` carries a job/server ID and a typed operation, never arbitrary host paths or arbitrary container IDs. `ApiState` owns the SQLite pool and local agent client. `claim_job(worker: &str) -> Result<Option<Job>>` leases a durable job transactionally.

- [ ] Write `job_recovery.rs`: enqueue one job, race two claimers, assert exactly one claim; expire a lease and assert it becomes recoverable without duplicating the operation ID.
- [ ] Run `cargo test -p manager-api --test job_recovery`; expect failure before the implementation.
- [ ] Implement the workspace and SQLite schema for users, hashed invites/sessions, servers/owners/members, eggs/revisions, reservations, allocations, jobs/operation checkpoints, schedules, backups/retained ACLs, installation metadata, and audit events. Add indexes/foreign keys and uniqueness for active resource assignments.
- [ ] Implement bounded agent RPC over a Unix socket with OS peer credential checks, restricted socket permissions, protocol versioning, message size limits, and idempotent operation IDs. API never accepts a browser-supplied agent operation directly.
- [ ] Run the focused test and `cargo check --workspace`; expect successful recovery assertions and compilation. Keep Linux RPC checks on Linux rather than weakening the boundary for Windows development.
- [ ] Commit this independently testable foundation.

## Task 3: Invite authentication and enforce ownership

**Files:** Create `crates/core/src/permissions.rs`, `crates/api/src/auth.rs`, `crates/api/tests/access.rs`; modify `model.rs`, API router/state.

**Interfaces:** `can_manage(actor: &User, server: &Server, member_enabled: bool) -> bool`; `require_server_access(user: UserId, server: ServerId) -> Result<ServerAccess>`. Routes: `POST /api/invites` (admin), `POST /api/register`, `/api/login`, `/api/logout`; `PUT /api/servers/{id}/members/{user_id}`. Account ownership transfer is admin-only. The initial administrator is created via local CLI, never a public bootstrap endpoint.

- [ ] Write `access.rs`: owner/admin/enabled teammate can manage; disabled teammate can get only dashboard fields; teammate cannot disable owner/admin; expired/used invite fails; racing invite redemption creates one account. Revoking membership invalidates long-lived authorization.
- [ ] Run `cargo test -p manager-api --test access`; expect failure.
- [ ] Implement password hashing, hashed expiring invite/session tokens, secure cookies, CSRF/Origin checks, login limits, and shared access enforcement. Use established Rust security crates, not custom cryptography. Prevent account deletion until owned servers are reassigned.
- [ ] Run the test; expect all assertions to pass. Test console/download revocation again in their owning tasks.
- [ ] Commit authentication and permissions.

## Task 4: Shared, versioned egg library

**Files:** Create `crates/core/src/eggs.rs`, `crates/api/src/eggs.rs`, `crates/core/tests/eggs.rs`, `tests/fixtures/eggs/{java,bedrock,unsupported}.json`; modify API routes.

**Interfaces:** `normalize_egg(raw: &[u8]) -> Result<EggRevision, Vec<CompatibilityIssue>>`; normalized revisions contain image choices, variable rules, startup template, stop strategy, readiness matcher, install specification, and config transforms. Routes: `GET/POST /api/eggs`, `POST /api/eggs/{id}/revisions`.

- [ ] Write fixture tests: Java and Bedrock normalize; unsupported rules/parser/inheritance are rejected with field paths; invalid JSON and oversized input fail; import never executes code. A new revision leaves an existing server pinned to its old one.
- [ ] Run `cargo test -p manager-core --test eggs`; expect failure.
- [ ] Implement the Task 1 compatibility matrix, not a permissive parser that drops unfamiliar fields. Validate user variables before interpolation and reserve system-controlled values. Only importer/admin can edit unused revisions; referenced ones cannot be deleted.
- [ ] Run the focused tests and one API test proving any invited member can import and everyone can list the shared result; expect pass.
- [ ] Commit library and fixtures with attribution.

## Task 5: Atomic capacity admission and allocation pools

**Files:** Create `crates/core/src/capacity.rs`, `crates/api/src/servers.rs`, `crates/api/tests/admission.rs`; modify migration schema if needed, agent storage module.

**Interfaces:** `Resources { memory_bytes: u64, disk_bytes: u64, cpu_millis: u32 }`; `admit(requested: Resources, budget: Resources, reserved: Resources) -> Result<()>`. `POST /api/servers` transactionally creates owner, reservation, allocation, and install job. `PATCH /api/servers/{id}/allocations` stages a validated change; admin pool management is separate.

- [ ] Write admission tests: two concurrent requests cannot overspend; stopped servers retain reservations; legacy limits reduce budget; unavailable/foreign IP and occupied TCP/UDP mapping fail; larger limits require fresh admission. Reject zero, overflow, and unsafe disk headroom.
- [ ] Run `cargo test -p manager-api --test admission`; expect failure.
- [ ] Implement atomic reservation and hardware-aware presets. Agent rechecks actual port binding and quota readiness before provisioning; runtime conflicts fail cleanly and release only this job's reservations. Resource changes apply Docker limits and storage quotas with recoverable failure handling.
- [ ] Run focused tests and a disposable Linux quota-exceed check; expect excess writes refused without consuming OS reserve. Do not declare hard quotas working from unit tests alone.
- [ ] Commit capacity/allocation behavior.

## Task 6: Isolated installation and first Java container

**Files:** Create `crates/agent/src/{docker,storage}.rs`, `crates/agent/tests/isolation.rs`, `tests/host/java-smoke.sh`; modify RPC, jobs, servers.

**Interfaces:** `install(server: ServerId, revision: EggRevision, resources: Resources, operation: JobId) -> Result<InstallResult>`; `start(server: ServerId) -> Result<()>`. Agent derives directories and container labels from its own registry. Store resolved image digest and installation metadata.

- [ ] Write isolation tests: arbitrary host mount/socket/privileged/network request fails; installer sees only its server volume; another application's container cannot be acted on; interrupted install is marked recoverable and never auto-started.
- [ ] Run `cargo test -p manager-agent --test isolation`; expect failure.
- [ ] Implement bounded install containers, temporary staging, required egg environment/config adapters, non-root runtime, dropped capabilities, restricted network access, and image resolution. Enforce game license acceptance before install/start where required.
- [ ] Run focused tests, then `bash tests/host/java-smoke.sh` on a disposable Linux host. Expect install, readiness detection, console command round-trip, graceful stop, and saved data. Do not run this against legacy IDs.
- [ ] Commit installer and first-game path.

## Task 7: Console, metrics, crash recovery, and host lifecycle

**Files:** Create `crates/core/src/lifecycle.rs`, `crates/agent/src/lifecycle.rs`, `crates/api/tests/lifecycle.rs`, `deploy/systemd/{manager-api,manager-agent}.service`, `tests/host/lifecycle-smoke.sh`; modify API server routes/RPC.

**Interfaces:** `next_action(state: LifecycleSnapshot, event: LifecycleEvent, now: DateTime<Utc>) -> LifecycleDecision`. Routes: `/api/servers/{id}/power`, `/console` (WebSocket), `/metrics`; dashboard projection contains only ID/name and resource measurements, with no hidden server metadata. Audit every lifecycle operation.

- [ ] Write deterministic-clock tests: 120-second graceful timeout; 3 attempts in 10 minutes; manual stop suppresses all restarts; circuit breaker survives agent/API restart; stale intent cannot resurrect deleted/install-failed servers. Revoke access during an open console and assert commands/log delivery stop.
- [ ] Run `cargo test -p manager-api --test lifecycle`; expect failure.
- [ ] Implement container-event reconciliation, bounded logs, owner-only management routes, resource-only list projection, CPU/RAM/disk/network sampling, desired-running persistence, staggered boot, retry delays, force-stop auditing, and concurrent controlled shutdown.
- [ ] Set Docker restart policy to no automatic restart for managed containers: the agent owns retries and host restoration. Establish systemd ordering so the agent stops before Docker/storage/network; configured service timeout exceeds concurrent grace plus cleanup.
- [ ] Run tests and disposable-host lifecycle smoke: clean reboot preserves saves and only desired-running servers return; process crash reaches retry limit. Legacy host reboot remains a separately scheduled acceptance check.
- [ ] Commit lifecycle/console/metrics.

## Task 8: Accessible dashboard, creation, and server workspace

UI direction added by the user during implementation: Codex-inspired dark workspace/sidebar, subtle borders, compact controls, and restrained typography. Use the user's PC for browser testing; keep tests isolated from the existing Pterodactyl panel. This requirement applies to login, library, dashboard and all server-management views.

**Files:** Create `web/package.json`, lockfile, TypeScript/Vite config, `web/src/{App.svelte,api.ts,app.css}`, pages `Login.svelte`, `Dashboard.svelte`, `CreateServer.svelte`, `Server.svelte`, `Library.svelte`, `Admin.svelte`; create `web/tests/access.test.ts`, `crates/agent/src/files.rs`, `crates/agent/tests/files.rs`.

**Interfaces:** Browser consumes only `/api` routes above. File routes under `/api/servers/{id}/files` expose relative paths with list/read/upload/download/rename/delete/create operations; text edits accept a version/hash for conflict detection. Agent uses directory-relative no-follow operations rather than string prefix checks.

- [ ] Write UI tests for locked cards exposing name/resources only, owner/member controls, invite registration, capacity explanation, and full keyboard access. Write file tests for `../`, absolute paths, archive paths, symlink replacement races, stale text edits, oversized/binary text edits, and access revoked during download.
- [ ] Run `npm --prefix web test -- --run` and `cargo test -p manager-agent --test files`; expect failure.
- [ ] Implement responsive screens with visible status/error/loading states, labeled forms, confirmation dialogs, and usable console. Add file uploads/downloads and text editing without loading unbounded files into memory; bounded staging plus atomic file replacement.
- [ ] Run focused tests and `npm --prefix web run build`; inspect login, dashboard, creation, server console/files, and small-screen layouts in a browser. A frontend-hidden button is never the permission boundary.
- [ ] Commit usable panel and files.

## Task 9: Backups, restore, and deletion with retained backups

**Files:** Create `crates/core/src/backup_policy.rs`, `crates/agent/src/backups.rs`, `crates/api/tests/backups.rs`, `web/src/pages/Backups.svelte`, `tests/host/backup-smoke.sh`; modify jobs/server deletion routes.

**Interfaces:** `BackupPolicy::new(retention: u8) -> Result<BackupPolicy>` defaults to 3, permits 1..=5. Agent operations create/verify/stage/restore archives by owned backup ID. Routes under `/api/servers/{id}/backups`; retained backups have independent ACL records and storage accounting.

- [ ] Write tests: reject 0/6 retention; at five backups failed sixth preserves all five; successful sixth removes oldest; failure to restart after archive still preserves backup; insufficient space fails before destructive work. Wrong delete-name fails. Retained backup access matches deletion-time ACL.
- [ ] Add interrupted archive, corrupt/traversing restore, interrupted directory switch, failed recovery-copy, and canceled-job tests. Assert original data or a named recovery directory survives every restore checkpoint.
- [ ] Run `cargo test -p manager-api --test backups`; expect failure.
- [ ] Implement stop-before-backup/restart-afterward, verified archive publication before rotation, explicit retention reduction confirmation, temporary space reservations, local downloads/deletion, and recovery-copy restore. Block concurrent mutating jobs per server.
- [ ] Implement typed-name server deletion; preserve selected archives and ACLs without orphaning reservations or leaving a container restart path.
- [ ] Run focused tests and disposable backup smoke with checksummed content before/after restore; expect exact recovery and no deletion on failed replacement.
- [ ] Commit backup/restore/deletion.

## Task 10: Schedules, startup settings, and activity log

**Files:** Create `crates/api/src/schedules.rs`, `crates/api/tests/schedules.rs`, `web/src/pages/{Schedules,Settings,Activity}.svelte`; modify jobs, egg validation, and API routes.

**Interfaces:** `POST /api/servers/{id}/schedules` records command/backup/restart, next execution, timezone and enabled state. `PATCH /api/servers/{id}/startup` validates a typed setting map against the pinned revision. `GET /api/servers/{id}/activity` returns paginated redacted audit events.

- [ ] Write tests: no schedules on creation; duplicate scheduler ticks enqueue one occurrence; revoked schedule author cannot bypass changed server access; missed occurrences do not create an unbounded catch-up queue; daylight-saving change is handled once per intended occurrence. Invalid egg/system variables are rejected and secret values never enter logs.
- [ ] Run `cargo test -p manager-api --test schedules`; expect failure.
- [ ] Implement explicit user schedules with timezone display, disabling/deleting, durable occurrence IDs, and per-server serialization. Record automated actions as service actions attributed to configuration author. Validate settings and apply changes only at a safe restart boundary; show restart required.
- [ ] Run focused tests and browser forms for schedules/settings/activity; expect correct errors and no default backup schedule.
- [ ] Commit automation/settings/audit views.

## Task 11: Minecraft editions/loaders and Modrinth extensions

**Files:** Create `crates/api/src/{minecraft,modrinth}.rs`, `crates/api/tests/minecraft.rs`, `tests/fixtures/upstream/modrinth.json`, `web/src/pages/Mods.svelte`, `tests/host/minecraft-matrix.sh`; modify creation/settings pages.

**Interfaces:** `Installation { edition, implementation, game_version, build_version, loader_version, image_digest }`; `resolve_extensions(installation: &Installation, requested: &[VersionId]) -> Result<ExtensionPlan>`. Routes expose installation options, Modrinth search, a dependency/file-change preview, and a confirmed install job.

- [ ] Write fixture tests: filter wrong loader/version/client-only project; include required dependencies; detect conflicting versions/dependency cycles; unknown installation stays unknown; failed staged download or hash mismatch leaves active files unchanged. Pack project types are rejected.
- [ ] Run `cargo test -p manager-api --test minecraft`; expect failure.
- [ ] Implement Vanilla, Paper, Fabric, Forge, NeoForge and Bedrock adapters with exact upstream version metadata and runtime requirements. Enforce explicit EULA/license acceptance. Reuse qualified egg/installer machinery rather than parallel provisioning code.
- [ ] Implement Modrinth search/filter/details and stopped-server transactional extension install with verified downloads and sanitized destinations. Resolve existing-file ambiguity visibly instead of overwriting unknown jars.
- [ ] Run focused tests and disposable matrix smoke for each listed implementation; record installation/startup and a real Java plus Bedrock client connection. API metadata matching alone is not proof that a mod works.
- [ ] Commit Minecraft variants and extension browser.

## Task 12: Manual updates and opt-in automatic build updates

**Files:** Create `crates/api/tests/updates.rs`, `web/src/pages/Updates.svelte`; modify Minecraft adapters/jobs/schedules/backups.

**Interfaces:** `plan_update(installation: &Installation, target: &Installation, mode: UpdateMode) -> Result<UpdatePlan>`. Manual may change game version with explicit confirmation; automatic must retain it. Update jobs checkpoint backup/staging/switch/start/readiness results.

- [ ] Write tests: automatic disabled by default; automatic different game version rejected; mods/plugins are not silently updated; backup failure preserves running intent and prevents update; interrupted switch remains recoverable; startup failure stops retries and offers recovery instead of silently rolling a world back.
- [ ] Run `cargo test -p manager-api --test updates`; expect failure.
- [ ] Implement manual target choice and per-server automatic build schedule with the warning about broken mods/plugins/worlds. Require a successful consistent backup and a safe stopped boundary before changing files. No restart when the server was intentionally stopped. Unsupported update adapters show manual-only with the reason.
- [ ] Run tests and disposable successful/failed update checks; verify restore from the pre-update archive. Maintain a failed-release marker to prevent repeated automatic installation.
- [ ] Commit update behavior.

## Task 13: Experimental servers and sourced upstream progress

**Files:** Create `crates/api/src/experimental.rs`, `crates/api/tests/experimental.rs`, `tests/fixtures/upstream/{pumpkin,steel}.json`, `web/src/pages/Experimental.svelte`; modify Minecraft installation registry.

**Interfaces:** `ProjectStatus { source_url, fetched_at, latest_release, supported_versions, features, milestone_counts, stale }`. Sources are curated official Pumpkin/Steel repositories, not arbitrary user-supplied URLs. Installation consumes the same `Installation`/job path.

- [ ] Write tests: one-hour cache; timeout/rate limit returns stale last-known data; absent roadmap is unknown; issue closure counts cannot become completeness; no compatible binary disables install rather than choosing a wrong platform; unsupported extension systems hide Modrinth installation actions.
- [ ] Run `cargo test -p manager-api --test experimental`; expect failure.
- [ ] Implement official release selection and sourced feature/milestone descriptions. Preserve source timestamps and upstream disclaimers; do not calculate an overall readiness percentage or ETA. Install into new test servers only and pin selected artifacts/digests.
- [ ] Run focused tests, inspect the experimental page, and smoke-test available compatible Pumpkin/Steel releases on disposable data. If an artifact is unavailable, show that limitation rather than claim installation was tested.
- [ ] Commit experimental catalog.

## Task 14: Debian deployment and migration rehearsal

**Files:** Create `docs/operations/{deployment,migration,recovery,release-checklist}.md`, `deploy/nginx/manager.conf`, `tests/host/release-smoke.sh`; modify systemd units/configuration documentation as qualification requires.

**Interfaces:** Packaged API assets and two binaries, documented restricted service users/directories/socket, private HTTPS binding, storage quotas, environment file references with no secrets. Migration input is explicit source server metadata plus a consistent copied volume; never a shared live volume.

- [ ] Prepare a deployment dry-run/runbook showing exact new paths, interfaces/ports, units, quotas, isolation rules, health checks and rollback. Confirm existing CasaOS/Pterodactyl/tunnel mappings remain intact. Do not expose the panel publicly.
- [ ] Rehearse migration using a disposable copy: map owner/access, egg revision, game/loader/build, extension files, limits/ports/startup settings; generate checksums and compare saved world data. Recreate schedules intentionally; never import credentials or broad database dumps.
- [ ] Require an explicit scheduled live cutover/reboot window before interrupting existing games. Stop source cleanly, make final copy, start destination, verify client play, keep source stopped and untouched for rollback. Never run two writers on one world.
- [ ] Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm --prefix web test -- --run`, `npm --prefix web run build`; expect all checks to pass on supported targets.
- [ ] Run release smoke on staging: Java/Bedrock joins, permission revocation, capacity/quota refusal, file boundary, archive recovery, automatic-update failure, private-peer HTTPS, controlled reboot, crash circuit breaker, and legacy-service health. Record evidence and any user-dependent acceptance checks still pending.
- [ ] Commit deployment/runbooks; deploy or cut over only after presenting the concrete rollout. Debian release acceptance does not establish support for untested distributions.

## Self-review and execution handoff

All approved sections map to tasks: accounts (3), shared eggs (4), capacity/allocations (1/5), controls/metrics/lifecycle (6/7), UI/files (8), backups/deletion (9), schedules/settings/audit (10), Minecraft/Modrinth (11), updates (12), experiments (13), deployment/migration (14). Host uncertainty is isolated behind Task 1 and release checks rather than hidden in assumptions. The future modpack list remains separate.

Review Focus cases each have an owning test above. Shared identifiers and installation/job interfaces are defined before consumers. The core/API/agent/web split is sufficient; do not introduce a plugin framework, Kubernetes, microservice fleet, or distributed job broker.

Suggested execution: native implementation in this chat, with focused checks after each task and independent review before deployment. This avoids repeated full-context handoffs while the first build establishes closely related interfaces. Subagent-driven execution is available if the user prefers independent task-by-task review.

This plan is documentation, not evidence that any runtime, backup, update, or migration check has passed. No task is marked complete until its stated checks have run.
