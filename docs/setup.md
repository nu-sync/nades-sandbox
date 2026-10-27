# Local asset setup

The viewer reads a prepared map pack for each map. The packer builds it from a Source 2 Viewer GLB export and local CS2
files. Neither game assets nor prepared packs are distributed with this repository.

## Building

The workspace builds on its own (`cargo build --release`): no path dependency leaves it, and a test says so
(`crates/sandbox/tests/standalone.rs`). Its tests need `python3` for the fixture `tools/cs2_throws.py` writes.

## Preparation pipeline

`nades-pack` builds a map's pack from CS2's own files alone, with three public tools: Source 2 Viewer's CLI (the
export, the decompiles), Bun's `bunx` (glTF-Transform 4.5.1, which compresses the geometry export) and, to fetch a map
from Steam, DepotDownloader. `nades-pack setup <map>` fetches what a map needs into the game folder (anonymously, from
the CS2 depot, pinned to the build the folder already holds; a pool map is a few hundred MB, the first one more for the
shared archives); `nades-pack <map>` then builds its pack. show-me-the-nades' `export_map.py` is no longer needed.

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
(decisions S26), and the exports it makes are byte for byte `export_map.py`'s (S28).

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

## The lineups' inputs (M3)

The map draws from its pack alone. The lineup list, the views and the effects read more, all optional (without
them the map still opens, with no lineups or with the stand-ins named below):

| Input | Default | What reads it |
|---|---|---|
| the throws: cs2-throws files | `--data <file or folder>` (repeatable), else `NADES_SANDBOX_DATA` (paths separated as in PATH), else `[data] paths` in `config.toml` in the platform's config folder, else the demo reader's cache (`throws/` in the platform's cache folder) | the lineups and each throw's flight, smoke, fire and viewpoints, read-only; a file written again is read again |
| the collider | the map pack's Collider section (`nades-pack <map>` with the CLI) | the result shot's line of sight and the smoke's light; without it, an overview and an unlit smoke |
| the game's smoke noise and fire flipbooks | `<packs>/shared/effects.pack` (`nades-pack --effects`) | the smoke and the fire; without them, the web view's stand-ins |
| the remembered view | `~/.cache/nades/sandbox/state.json`, or `NADES_SANDBOX_STATE` | each map's last lineup, view and camera, written on exit |

A flag beats the environment, which beats the config file, which beats the default. On macOS the config file is
`~/Library/Application Support/io.github.nu-sync.nades-sandbox/config.toml`:

```toml
[data]
paths = ["~/cs2/pro.sqlite", "throws"]   # a relative path is the config folder's
```

The packs folder follows the same order (`--packs`, `NADES_SANDBOX_PACKS`, `[assets] packs`, the platform's cache).

### Writing cs2-throws files

`tools/cs2_throws.py` holds the schema (docs/standalone-api.md §3) and a `Writer` that needs only Python's standard
library; its `fixture` command is a worked example that fills every table. show-me-the-nades writes its database as one
file with `nades export --cs2-throws <file>`. Either way, `nades-sandbox check <file>` names the file's problems and,
feature by feature, what its facts allow and which fact a feature lacks.

## The macOS app (M4)

```sh
just bundle      # target/release/bundle/macos/Nades Sandbox.app, signed for this machine
```

The app reads the same inputs as the binary: the packs, maps, effects and remembered view under `~/.cache/nades`, and
the throws its config file names. Opened from Finder it takes no environment of its own;
`open --env NADES_SANDBOX_DATA=/path/to/throws.sqlite "target/release/bundle/macos/Nades Sandbox.app"` points it
elsewhere, as the variables above do for the binary. Its first launch compiles the shaders (about half a
second): macOS keeps their cache per app. It keeps the other packed maps on the GPU within a budget of 8 GiB, or half
of what the GPU recommends where that is less; `NADES_SANDBOX_BUDGET_MB` or `--budget-mb` sets it.

## Data and verification inputs

```sh
./target/release/nades-sandbox check /path/to/throws.sqlite
cargo run --release -p nades-source-file --example tag_parity -- /path/to/throws.sqlite nades
```

The second compares the sandbox's groups, techniques and landing callouts with a producer's own labels for them (here
show-me-the-nades' `nades.lineup`, `nades.technique` and `nades.land_place` tags) and counts every difference.

`bench/cameras.py` and `bench/effects.py` compare the sandbox with show-me-the-nades' web view. They take that
checkout as `--nades <dir>` (or `SHOW_ME_THE_NADES`), clone its database (copy-on-write) into their run folder, start
its web server (and, for the effects, its development server) on the clone, and export the clone with its
`nades export --cs2-throws` for the sandbox (`--data` takes an export made earlier instead). They need Bun and that
checkout's installed `web/` packages. Never point them at the live `nades.duckdb` without the clone.

`cargo test --release --workspace` uses synthetic fixtures, among them the one `tools/cs2_throws.py` writes. The
ignored tests require local game files, exports and packs. `bench/realism.py --nades <dir>` also requires that
checkout's `verify-scene-realism` skill, reference shots and latest web rendering run; it reads those in place. Its
generated comparisons stay local.
