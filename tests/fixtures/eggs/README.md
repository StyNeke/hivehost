# Compatibility fixtures

Unmodified JSON data from `pterodactyl/game-eggs`, revision `be654a201bf3b90446569167d4864edec1b33009`, retrieved 2026-10-27. MIT license reproduced in LICENSE.upstream. These contain executable script text; tests parse it as data and never execute it.

- `java.json`: `minecraft/java/paper/egg-paper.json`, SHA256 `fa76a2409a1f97848dd06ffd7c1c59049e38837f4a9eb1c3b27241f613b27bb9`.
- `bedrock.json`: `minecraft/bedrock/bedrock/egg-vanilla-bedrock.json`, SHA256 `7723befb387894afeccc042f25560550a22785bc867e12a2d0254f0259f75e6b`.
- `fabric.json`: `minecraft/java/fabric/egg-fabric.json`, SHA256 `0869ad80cd0be6bc778571506182e23db0b10941e529262a566e72252bab7d23` (same revision, retrieved 2026-10-27).

- `forge.json`: `minecraft/java/forge/egg-forge-enhanced.json`, SHA256 `5bdb3c6c5a551510b793af8418ebf99905ebc472491d47b02db0c91f4d20832d` (same revision, retrieved 2026-10-28).
- `neoforge.json`: `minecraft/java/neoforge/egg-neoforge.json`, SHA256 `1b029c455741bb4df11a022b97a8e769c8b851c88b9af3e6e74f7737fb0a09f7` (same revision, retrieved 2026-10-28).

Source: https://github.com/pterodactyl/game-eggs/tree/be654a201bf3b90446569167d4864edec1b33009

`vanilla.json` is the egg data (not code) shipped with the upstream panel, which game-eggs does not
carry: `database/Seeders/eggs/minecraft/egg-vanilla-minecraft.json` from `pterodactyl/panel`
revision `eed61bf0725e105520012fefdded522abc25774e`, SHA256
`20dc827bb5f692e3b0a0315cce8feefbb32d44d0a658bafe9c100384d9904c8a`, retrieved 2026-10-28; MIT
license in LICENSE.panel. Only this JSON is used.

Passing normalization tests does not establish runtime installation or game compatibility. No Panel or Wings implementation is included.
