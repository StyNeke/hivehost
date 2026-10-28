# Experimental server eggs

Bundled with the API and offered to administrators from the Experimental panel. Installing
one creates a new test server; nothing converts an existing world.

- `pumpkin.json`: unmodified `egg-pumpkin.json` from
  [Pumpkin-MC/Pumpkin](https://github.com/Pumpkin-MC/Pumpkin) at commit
  `09e63cb8d8a71652f29c7fa8434bc118bf67193a` (2026-10-27), SHA256
  `ef8d3f19d8787d74fb25f0676e615890692713324dd27c174e6c5214e2062360`. MIT license in
  `LICENSE.pumpkin`. It downloads the published musl build from Pumpkin's GitHub releases.
- `steelmc.json`: written for this project, because SteelMC publishes no egg. It follows
  [Steel-Foundation/SteelMC](https://github.com/Steel-Foundation/SteelMC) at commit
  `65cd7ab416bb2bbe04d4aca3b6d04b1d939d5a8d`: the `steel-linux-musl` release asset, the
  `config/config.toml` port setting, the `Started Steel Server` log line, and stopping with
  SIGINT (`^C`): SteelMC reads console input through a terminal library, so console commands do
  not reach it in a container.

Both accept a `DOWNLOAD_URL` override. Neither implementation supports plugins or mod loaders,
so Modrinth installs stay unavailable for them.
