# Private Linux game server manager

Date: 2026-10-27
Status: Approved by the user on 2026-10-27. Host deployment and migration require a concrete reviewed rollout.

User decisions after review: the proposed operational defaults, architecture, and complete design are accepted. Existing third-party runtime and installer images, including Pterodactyl-associated images referenced by eggs, are allowed.

## Purpose and scope

Build a new web-based game server manager for a small team of friends. Debian is the primary target; keep the design portable to other Docker-capable Linux distributions. Initially manage one physical host. Build our own panel, backend, and host management service: no Pterodactyl Panel or Wings runtime dependency or copied implementation. Pterodactyl egg JSON files are an import format only. Eventual migration of existing Pterodactyl servers is required.

The first release includes all features below, delivered incrementally rather than treated as a single deployment. Minecraft Java is the first end-to-end acceptance target; Bedrock must also work before the first release is considered complete. Modrinth modpacks are deferred in the root FUTURE_FEATURES.md.

## Confirmed requirements

### Accounts and access

- Private access through the user's VPN, described as NordVPN Meshnet. Verify the actual interface and peer connectivity before choosing bindings or firewall rules; the SSH fallback address alone does not identify the VPN technology.
- One administrator initially: the user. Invite-only registration using single-use links; invitees set their own passwords.
- Every member may create servers and becomes their owner.
- Every member sees every server's name and resource usage on the dashboard. Without access, no server page, console, files, allocations, configuration, or controls are available.
- Each server has exactly one owner. The owner enables or disables individual teammates' access.
- Enabled teammates have full control, including deletion and managing other teammates' access. They cannot revoke the owner's access. The administrator always retains full access.
- Proposed default: ownership transfer is administrator-only; account deletion cannot orphan servers. No fine-grained teammate roles in this release.

### Dashboard and server creation

- Main dashboard lists existing servers with an Add server action.
- Creation offers the shared egg library and Minecraft installation choices.
- Everyone may import eggs into the shared library and use them without administrator approval.
- Offer simple hardware-aware CPU, RAM, and disk presets, plus adjustable limits inside the safe budget.
- Enforce capacity admission checks instead of permitting over-allocation after a warning. Include existing Pterodactyl workloads, OS overhead, other services, images, backups, and temporary installation space.
- Proposed default: reserve RAM and disk for all created servers, including stopped ones. CPU presets use a total allocatable CPU budget; displayed percentages explain that 100% equals one logical CPU. Exact overhead reserves follow inspection and load testing, not guessed constants.
- Existing workloads must be inventoried before admitting new workloads. If the legacy limits already consume the budget, block new creation and explain why; do not silently reduce their limits.
- CPU usage is a limit, not a performance guarantee. Disk limits require enforceable storage controls; usage monitoring alone must not be presented as a hard quota. Qualify a suitable quota-capable storage layout before production provisioning. A dedicated quota-capable filesystem/volume is the proposed fallback if the existing ext4 layout cannot enforce per-server quotas safely.

### Server controls

- Start, graceful stop, and restart.
- Live console: stream logs and send commands.
- File manager: list, upload, download, create, rename, delete, and edit text files inside that server's storage.
- Allocations: change primary and additional IP/port mappings from an administrator-defined pool. Validate host IP ownership, protocol, port availability, and conflicts with other services and legacy servers. Users cannot add arbitrary host interfaces or change host networking.
- Backups: create, download, delete, and restore.
- Schedules: user-configured restarts, backups, and commands; no default scheduled backup job.
- Startup settings: edit egg variables, launch settings, appropriate runtime image, and supported game-version settings within validated limits.
- Resource monitoring: CPU, RAM, disk, and network usage. Proposed dashboard default: current CPU/RAM, disk consumption, and network rates; no performance-history database in this release.
- Activity log: actor, action, server, timestamp, outcome, and operational failures. Do not record passwords, invite tokens, or secret variable values.
- Deletion requires typing the server's name and offers keeping its backups. Preserved backups remain visible to their permitted users and count toward host storage. Proposed default: retained backups keep the deletion-time owner/member access list and administrator access, and cannot be restored into an unrelated server without an explicit restore action and capacity checks.

### Backup policy

- Store backups on this host.
- Retention defaults to 3 per server; users may choose 1 through 5.
- At the selected limit, replace the oldest backup only after a new backup completes successfully. If space is insufficient for safe replacement, fail without deleting a good backup.
- Proposed default: a temporary extra backup is allowed only during replacement, bounded by disk budget. Lowering retention requires confirming removal of the oldest excess backups.
- Required pre-update backups also use these retention rules. If a pre-update backup fails, do not update or start the update operation; preserve the prior running/stopped state.
- Proposed default: stop the server gracefully before backup to guarantee a consistent archive across games; restart it afterward only if it was running. Display the downtime before manual backups and explain it in schedule setup.
- Restore requires explicit confirmation, a stopped server, archive integrity/path validation, capacity admission, and a recovery copy of the pre-restore data. Stage data before switching directories so a failed restore preserves recoverable data.
- Local backups do not protect against host disk failure. Retained backup growth must never consume the OS reserve.

### Minecraft and Modrinth

- Java first; also support Bedrock.
- Java installation choices: Vanilla, Paper, Fabric, Forge, and NeoForge. Track game version and loader/build version separately. Accept required game licenses explicitly; do not silently accept the Minecraft EULA.
- Built-in Modrinth browsing and installation of individual mods and plugins, filtered using recorded/detected game version, loader/server type, and server-side compatibility. Inspect existing files/metadata where possible; show unknown status rather than assume compatibility.
- Resolve required dependencies; show intended file changes before installation; reject incompatible or ambiguous choices rather than silently force them. Warn when API compatibility metadata cannot establish runtime compatibility. Avoid duplicate versions and changes to a running server's loaded extensions.
- Proposed default: server must be stopped before changing mods/plugins. Use staged downloads and validated filenames/hashes when available; a failed install must not leave a partial active set.
- Complete Modrinth modpacks are future work.
- Experimental section includes Pumpkin and SteelMC with direct installation of published builds in containers. Installation is of new test servers initially, not an in-place conversion of an existing world.
- Experimental entries show supported game versions, published releases/builds, recent GitHub activity, documented feature coverage, roadmap/milestone information, source links, and last successful refresh time.
- Never invent a completeness percentage or ETA. Show upstream milestone/checklist counts only with their scope and source; missing data is unknown. GitHub issue counts or commit frequency are not gameplay completeness measures.
- Proposed default: cache upstream metadata for one hour, offer a rate-limited manual refresh, retain last-known data during outages with a stale label. No GitHub credential required for initial public-repository support; handle API rate limits explicitly.
- Keep experimental builds clearly identified. Do not claim Bukkit/Paper/mod-loader compatibility without evidence for the exact implementation/build. Modrinth install controls are disabled where a supported extension mechanism cannot be established.

### Updates

- Manual updates are the default; optional automatic updates are configured per server.
- Automatic updates remain within the selected Minecraft version. Moving to another Minecraft version requires a manual action.
- Warn at automatic-update setup that builds can break mods, plugins, or worlds, especially modded and experimental servers.
- Create a successful backup before an automatic update; stop safely, stage the update, validate startup, and log results.
- Proposed default: automatic updates update the server implementation/build only, never automatically update user-installed mods/plugins. Loader and experimental builds qualify only when there is reliable version mapping and a supported update procedure; otherwise show manual-only with the reason.
- On failed update, stop retries and present recovery choices. Do not silently roll a potentially upgraded world back or repeatedly apply the same failing release.
- Users choose automatic-update timing. No surprise automatic updates on initial install.

### Host lifecycle and crash handling

- On controlled host shutdown/restart, request each running game's proper stop command or signal and allow saves to complete.
- Use a configurable grace period, then force-stop a hung server. Record force-stops in the activity log.
- Proposed default: 120 seconds grace per server with concurrent shutdown requests; coordinate the service's systemd stop budget so Docker/network/storage remain available until this process completes.
- Persist which servers were running before shutdown. After boot, restart those only, with capacity rechecks and staggered launches. Previously stopped or administratively disabled servers remain stopped.
- Automatically restart crashed game servers with backoff and bounded retries. Proposed default: at most 3 restart attempts in 10 minutes, then leave stopped with a visible reason. A requested stop, install failure, update failure, or shutdown is not a crash restart trigger.
- Proposed default: recover previously desired-running servers after an unexpected host crash, but retain crash-loop circuit breaker state across service restarts.
- A power outage or kernel failure cannot guarantee graceful saves.

## Approved architecture (originally proposed by the assistant)

- Rust for the API and host management service, using Axum and Tokio. The goal is efficient orchestration with low idle overhead, not a claim that the language accelerates the games.
- Svelte with TypeScript for the browser interface; compile static assets served by the API. No Node runtime required on the host for normal operation.
- SQLite for accounts, permissions, invites, server metadata, immutable egg revisions, schedules, task state, and audit events. No Redis or additional database service needed for this single-host workload.
- Two local service processes: an unprivileged web API and a narrowly exposed host agent. Communication through a restricted Unix socket; only the agent gets Docker/storage authority. The browser never gets Docker access. Jobs and actual container state are reconciled after restarts.
- Docker containers for each game and separate temporary installation containers. Manage only containers/storage marked as belonging to this application. Never operate on arbitrary container IDs supplied by a client.
- Serve the panel only on a verified private interface or through a privately bound HTTPS endpoint. Do not add it to the existing public Cloudflare Tunnel. Verify actual reachability from a friend peer before release. The browser-facing API and console require authentication even on the VPN.
- Separate data directory, database, containers, Docker network, and unused web/game ports from Pterodactyl and CasaOS. Final paths and ports follow inspection.
- Agent independently validates resource limits, file paths, container identities, and allocations. Installer eggs cannot request Docker socket access, privileged mode, arbitrary host mounts, host network/PID namespaces, or extra capabilities. Containers cannot reach host control services or legacy workloads through an unfiltered shared bridge; qualify network isolation during the host inspection.
- File/archive operations must enforce server-root containment, including symlinks, traversal, archive entries, and races. Apply size limits and bounded background jobs to uploads, extracts, log streaming, and imports.
- Password hashing, expiring hashed single-use invite tokens, secure sessions, CSRF protection, login rate limits, TLS, and server-side permission checks apply to HTTP and WebSocket traffic. Revoked access closes existing streams. Resource-only visibility must also hold at the API layer.
- Installer scripts execute only inside bounded temporary containers, with access to their own server directory and permitted outbound downloads. Importing stores data; it does not execute a script. Everyone may import/use eggs, but no egg may extend its authority beyond those boundaries. Container isolation reduces risk; arbitrary third-party code is not rendered trustworthy by JSON validation.

## Egg compatibility contract

Use a newly implemented importer and execution adapter. Start with exported Pterodactyl 1.x PTDL_v2 eggs; inspect actual fixtures to confirm schema. Version 2.x formats require explicit qualification rather than claiming support from their existence.

Map images, environment variables and validation, startup templates, install container/script/entrypoint, stop commands/signals, startup readiness detection, and required configuration-file transforms. Implement the needed compatibility surface in our own code. Store the original JSON and a normalized immutable revision; existing servers stay pinned to their revision when someone imports a replacement.

Do not silently ignore unsupported validation rules, inheritance, config parsers, special network behavior, or host dependencies. Mark such imports unsupported with actionable reasons; do not offer them as runnable until qualified. Shared-library edits create revisions and do not mutate other people's running servers. Proposed default: all users can import, while only the importer and administrator can edit or remove an unused revision; referenced revisions cannot be deleted.

The user confirmed that existing third-party/community runtime and installer images, including Pterodactyl-associated images referenced by eggs, are acceptable. The request for 'nothing from Pterodactyl' means no Panel/Wings code or runtime dependency. Image compatibility and isolation still require qualification; acceptance of an image source does not establish compatibility.

## Host inspection evidence and deployment constraints

Read-only SSH inspection on 2026-10-27 found:

| Item | Observed |
| --- | --- |
| OS | Debian GNU/Linux 13.6, kernel 6.12 |
| CPU | Intel i5-9600K, 6 cores / 6 threads |
| RAM | About 23 GiB reported by free; about 19 GiB available at inspection |
| Main filesystem | ext4, about 103 GiB usable, 72 GiB available |
| Existing services | CasaOS, Docker/containerd, Pterodactyl Wings and queue worker, Nginx, PHP-FPM, MariaDB, Redis, Cloudflare Tunnel, NordVPN, Samba, rclone and others |
| Containers | One running Java 21 container, three stopped Java containers, one stopped installer |
| Running game | About 2.5 GiB resident RAM, about 20.5 GiB configured memory limit; TCP/UDP port 25567 on the LAN interface |
| Other observed listeners | SSH 22, web 80/8085, Wings-related listeners 8080/2022, plus internal service ports |

The hostname did not resolve from the current workstation; the user-supplied fallback IP worked. The SSH host key matched the existing known_hosts entry for the hostname. Do not put passwords, SSH private keys, tunnel tokens, or application secrets in this repository.

These observations are a snapshot, not a complete capacity or security audit. Before deployment, inspect firewall rules, actual VPN interfaces/peers, Nginx bindings, Docker networks/version/storage quota support, service ordering, existing game configuration, backup sizes, storage paths, and migration prerequisites. Read targeted non-secret fields; do not dump environment files or tunnel credentials. Make no changes to legacy services as part of discovery.

## Delivery stages and completion evidence

1. **Host qualification and compatibility spike:** verify networking and capacity; test representative Java and Bedrock egg imports in isolated staging. Qualify hard disk quotas and shutdown ordering. Resolve runtime image policy before implementation commitment.
2. **Foundation:** invite authentication, visibility/ownership rules, shared egg revisions, capacity admission, allocations, API and restricted agent. Prove denied users cannot read server details or invoke actions.
3. **First running game:** Java creation/install/start, live console, metrics, graceful stop/restart, persisted state, crash handling. Demonstrate clean host reboot without affecting legacy services, in an agreed test window.
4. **Full server management:** file manager, startup settings, local backups/restore/rotation, schedules, audit log, deletion with retained backups. Test restore using disposable data, interrupted archive creation, full-disk refusal, and port conflicts.
5. **Minecraft coverage:** Bedrock, Vanilla/Paper/Fabric/Forge/NeoForge paths, compatible Modrinth extensions, manual updates and opt-in automatic build updates. Demonstrate at least one client connection for both Java and Bedrock; extension compatibility checks are not a substitute for a runtime test.
6. **Experimental implementations:** Pumpkin/SteelMC install choices and sourced GitHub status. Test stale/rate-limited data behavior and clearly unavailable builds.
7. **Migration and release:** rehearsed migration of a disposable copy of a legacy server, backup verification, explicit per-server cutover and rollback procedure. Original Pterodactyl containers/data remain available until each migration is verified. Never let two managers write the same live world. Owner mapping, extensions, ports, resource limits, permissions and schedules must be reviewed; import only the existing metadata actually needed.

Use focused tests for non-trivial behavior, especially permissions, capacity races, quota enforcement, path/archive containment, egg normalization, backup rotation, state transitions, update failure, and restart loops. Real host smoke checks are required for Docker, graceful saves, networking, and systemd lifecycle behavior. Broader Linux support is qualified distribution by distribution; initial release acceptance is Debian 13, not an untested promise for every distribution.

No billing, public signup, multiple-host management, modpack installation, or automatic Minecraft-version migration in the first release. Future ideas belong in FUTURE_FEATURES.md.

## Sources checked during planning

- Rust language overview: https://rust-lang.org/
- Tokio networking/runtime overview: https://tokio.rs/
- Egg process/configuration semantics: https://docs.pterodactyl.io/v1/guides/egg-creation/egg-config-parser
- Egg imports and fields: https://docs.pterodactyl.io/v2/guides/egg-creation/creating-custom-egg
- Pumpkin upstream repository and readiness statement: https://github.com/Pumpkin-MC/Pumpkin
- Pumpkin quick start: https://docs.pumpkinmc.org/
- SteelMC upstream repository and readiness statement: https://github.com/Steel-Foundation/SteelMC

Upstream status changes; refresh it at implementation time. Modrinth API details and host-specific enforcement are still research tasks, not verified capabilities.
