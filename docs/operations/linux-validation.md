# Linux validation — 2026-10-27

Validated on the user's Debian 13 host in a disposable Docker build container, not as installed services.

- Image: `rust:1.99-slim`, digest `sha256:1a939748a7cbab172de91a014e3804a97af1394a8c844faa67fce9401c4e40fb`; included Rust 1.99.1. Windows tests use pinned Rust 1.99.0.
- Limits: one CPU, 1 GiB memory with no extra swap, 256 PIDs; unprivileged UID/GID 1000; all capabilities dropped; no-new-privileges.
- No published ports, Docker socket, privileged access, or existing game-data mounts. Source was read-only. Cache/build files were in bounded temporary memory filesystems.
- Agent Linux tests first failed on unimplemented socket behavior, then passed after implementation: real peer UID authorization, non-writable owned socket parent, preserving existing paths, and socket mode 0660. Protocol framing/authorization tests also passed.
- Full Linux workspace run: **25 tests passed**, including auth, HTTP, jobs, egg normalization/library and policies. `cargo test --locked --workspace --jobs 1` used `CARGO_PROFILE_DEV_DEBUG=0` and `CARGO_PROFILE_TEST_DEBUG=0` to fit the bounded temporary build area.
- Initial full debug-symbol build failed in the linker with SIGBUS. The compact run passed under the same limits; final build/cache filesystem usage was 446 MiB of 768 MiB, with 321 MiB in target artifacts. This supports build-space pressure as the cause; no claim of a runtime bug fix.

Test sources/logs remain in `/home/stijn/game-manager-check.pQyDcja9`; the temporary container was removed on exit. The downloaded Rust image remains in Docker's cache. No credentials were written into the test directory.

These results do not verify game installation, Docker control, disk quotas, backups/restores, live updates, private HTTPS browser access, or graceful host reboot. The agent currently exposes only a health request, not arbitrary host commands.
