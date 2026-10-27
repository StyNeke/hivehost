# External compatibility qualification

## Egg boundary

Initial target: exported PTDL_v2 JSON, not Pterodactyl/Wings implementation reuse. Source format handling must cover metadata, selected runtime images, startup templates, variables/rules, installer image/script/entrypoint, configuration transforms, readiness and stop strategy. Store immutable revisions.

Candidate official fixtures: Paper Java and Minecraft Bedrock. Download and pin exact JSON/source revision plus license before implementation tests. Their runtime behavior is **not yet qualified**. Unsupported transforms, validation rules, inherited settings or host dependencies must prevent a runnable import, not disappear silently.

Existing upstream images are approved. Image pinning and entrypoint/environment conventions require tests; matching an image name is not compatibility evidence. No game image was downloaded or run during host discovery.

Sources:

- [Egg process/configuration semantics](https://docs.pterodactyl.io/v1/guides/egg-creation/egg-config-parser)
- [Paper catalog entry](https://eggs.pterodactyl.io/egg/games-paper)
- [Docker CPU/RAM constraints](https://docs.docker.com/engine/containers/resource_constraints)
- [Classic overlay2 versus containerd snapshotter](https://docs.docker.com/engine/storage/drivers/overlayfs-driver/)

## Modrinth contract research

Official API documentation checked 2026-10-27:

- Search uses facets; loader categories and game versions filter candidates. Environment metadata distinguishes server/client suitability; old side fields are deprecated in current search documentation.
- Project-version queries accept loader and game-version filters. Version records expose dependencies and downloadable files; search hits alone are insufficient for installation decisions.
- Supply an application-specific User-Agent and obey returned rate-limit headers. Cache and retry within the API policy; a version retirement/error is not an empty successful result.

Persist resolved version IDs, file hashes and the detected installation metadata. Resolve required dependencies, reject modpacks, and keep incompatible/unknown versions visibly unavailable. Fixture/API integration tests remain pending; documentation research is not a successful install.

Sources: [API overview](https://docs.modrinth.com/api/), [search](https://docs.modrinth.com/api/operations/searchprojects/), [project versions](https://docs.modrinth.com/api/operations/getprojectversions/), [version files/dependencies](https://docs.modrinth.com/api/operations/getversion/).

## Current qualification result

Discovery is sufficient to identify deployment blockers and begin pure application work. Supported egg matrix, hard quota enforcement, TLS trust and friend-peer HTTPS access require staging evidence before release. No promise of universal egg compatibility or all-distribution support.
