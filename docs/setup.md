# Local asset setup

The viewer reads a pack for each map, built on this machine by `nades-pack` from CS2's own files. Neither game
assets nor packs are distributed with this repository.

## Building

The workspace builds on its own (`cargo build --release`): no path dependency leaves it, and a test says so
(`crates/sandbox/tests/standalone.rs`).

## Preparation pipeline

`nades-pack` builds a map's pack from CS2's own files alone, with three public tools: Source 2 Viewer's CLI (the
export, the decompiles), Bun's `bunx` (glTF-Transform 4.5.1, which compresses the geometry export) and, to fetch a map
from Steam, DepotDownloader. `nades-pack setup <map>` fetches what a map needs into the game folder (anonymously, from
the CS2 depot, pinned to the build the folder already holds; a pool map is a few hundred MB, the first one more for the
shared archives); `nades-pack <map>` then builds its pack.

## Input layout

The packer's settings (docs/standalone-api.md §8) each come from a flag, else the environment, else `config.toml` in
the platform's config folder (on macOS `~/Library/Application Support/io.github.nu-sync.nades-sandbox/config.toml`):

| Setting | Flag | Environment | Config file | What it is |
|---|---|---|---|---|
| packs | `--packs <dir>` | `NADES_SANDBOX_PACKS` | `[assets] packs` | where packs go and the viewer reads them; default `packs/` in the platform's cache folder |
| CS2's files | `nades-pack --game <dir>` | `NADES_SANDBOX_GAME` | `[assets] game` | the folder holding `game/csgo`: an install, or DepotDownloader's folder (its manifests name the build); no default |
| Source 2 Viewer's CLI | `nades-pack --s2v <path>` | `NADES_SANDBOX_S2V` | `[tools] s2v` | exports the map's geometry and physics, decompiles its entities, skies, post-processing, layer textures and the fire's flipbooks, and says which meshes are lightmapped; no default |
| Bun's `bunx` | `nades-pack --bunx <path>` | `NADES_SANDBOX_BUNX` | `[tools] bunx` | runs glTF-Transform on the geometry export; default the PATH's |
| DepotDownloader | `nades-pack setup --depotdownloader <path>` | `NADES_SANDBOX_DEPOTDOWNLOADER` | `[tools] depotdownloader` | fetches a map's files from Steam; no default |

```toml
[assets]
packs = "~/.cache/nades/packs"
game = "~/.cache/nades/cs2/depot"

[tools]
s2v = "~/.cache/nades/tools/s2v-master/Source2Viewer-CLI"
depotdownloader = "~/.cache/nades/tools/depotdownloader/DepotDownloader"
```

From the game folder the packer reads `game/csgo/maps/<map>.vpk` (and the 3D skybox prefab's VPK beside it) and
`game/csgo/pak01_dir.vpk` with its numbered archives; with the CLI it decompiles what it needs into a scratch folder of
its own. The geometry export (`<map>.glb`, `<map>.skybox.glb`) is made in `exports/` in the platform's cache folder
(`--work` moves it; `--export <map>.glb` takes one made elsewhere instead). Use Source 2 Viewer's master build
(`s2v-master`, as `export_map.py` did): its decompiled flipbook frames are the ones the web view's fire was made from
(decisions S26), and the exports it makes are byte for byte `export_map.py`'s (S29).

Return to this checkout, then build the packs and open a map:

```sh
cargo build --release
./target/release/nades-pack setup de_vertigo # once a map: fetch what it needs from Steam (the pool maps are there)
./target/release/nades-pack de_cache        # every map you want
./target/release/nades-pack --effects       # once: the smoke noise and fire flipbooks, <packs>/shared/effects.pack
./target/release/nades-sandbox --map de_cache
```

The packer prints what it found and what it left out. Without the CLI a pack has no collider (the result shot falls
back to an overview, the smoke is unlit), no light probes, sky or post-processing, and lightmapping follows the web
view's rule; without the effects pack the smoke and fire draw the web view's stand-ins.

## Custom input locations

```sh
./target/release/nades-pack de_cache \
  --game /path/to/cs2 \
  --s2v /path/to/Source2Viewer-CLI \
  --export /path/to/maps/de_cache.glb \
  --out /path/to/de_cache.pack
```

`--unpacked <game/csgo>` adds game files unpacked from `pak01`, for a depot whose archives were dropped once unpacked.
The viewer reads `<packs>/<map>.pack` (`NADES_PACK` names one file).

## The simulator's inputs

The map draws from its pack alone. The simulation and the effects read more, all optional (without them the map still
opens, with the stand-ins named below):

| Input | Default | What reads it |
|---|---|---|
| the collider | the map pack's Collider section (`nades-pack <map>` with the CLI) | the grenades' bounces, the smoke's grid and light, the fire's spread, clicking a spot to stand on, the result shot's line of sight; without it the grenades fly through walls and leave no smoke or fire |
| the game's smoke noise, fire flipbooks and grenade models | `<packs>/shared/effects.pack` (`nades-pack --effects`; run it again for the grenade models) | the smoke, the fire and the grenades in flight; without them, the web view's stand-ins and a capsule |
| scene files | `~/nades-scenes/<map>.json` in the shell (any path typed, a file dropped on the window, or `--scene <file>`) | the throws placed on a map ([docs/simulator.md](simulator.md), `crates/control/src/scene.rs`) |
| the remembered view | `~/.cache/nades/sandbox/state.json`, or `NADES_SANDBOX_STATE` | each map's last camera and its scene, written on exit |

The packs folder follows the usual order: `--packs`, `NADES_SANDBOX_PACKS`, `[assets] packs` in `config.toml` in the
platform's config folder (`~/Library/Application Support/io.github.nu-sync.nades-sandbox/config.toml` on macOS), the
platform's cache. A config file from before the simulator may still hold a `[data]` section; it is ignored.

## The macOS app (M4)

```sh
just bundle      # target/release/bundle/macos/Nades Sandbox.app, signed for this machine
```

The app reads the same inputs as the binary: the packs and effects in the packs folder and the remembered view. Opened
from Finder it takes no environment of its own; `open --env NADES_SANDBOX_PACKS=/path/to/packs "target/release/bundle/macos/Nades Sandbox.app"`
points it elsewhere, as the variables above do for the binary. Its first launch compiles the shaders (about half a
second): macOS keeps their cache per app. It keeps the other packed maps on the GPU within a budget of 8 GiB, or half
of what the GPU recommends where that is less; `NADES_SANDBOX_BUDGET_MB` or `--budget-mb` sets it.

## Verification inputs

`cargo test --release --workspace` uses synthetic fixtures. The ignored tests require local game files and packs
(`NADES_PACK_EXPORT=<glb>` names the export `tests/export.rs` checks). `bench/realism.py --nades <dir>` requires a
show-me-the-nades checkout's `verify-scene-realism` skill, reference shots and latest web rendering run; it reads those
in place. Its generated comparisons stay local.
