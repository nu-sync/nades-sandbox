# Local asset setup

The viewer reads a prepared map pack for each map. The packer builds it from a Source 2 Viewer GLB export and local CS2
files. Neither game assets nor prepared packs are distributed with this repository.

## Building

The workspace builds on its own (`cargo build --release`): no path dependency leaves it, and a test says so
(`crates/sandbox/tests/standalone.rs`). Its tests need `python3` for the fixture `tools/cs2_throws.py` writes.

## Preparation pipeline

The current exporter is `scripts/export_map.py` in the separate `show-me-the-nades` project. That project is
not part of this public preview. If you already have that checkout, run these commands there:

```sh
uv run scripts/export_map.py de_cache
```

The exporter obtains the map's dependencies from Steam, runs Source 2 Viewer, and prepares the compressed GLB,
material metadata, skybox and post-processing inputs. It uses uv, Bun, Source 2 Viewer and DepotDownloader;
the first export downloads several GB. Use its default cache paths for the commands below.

For readers without the exporter, standalone asset preparation is a known limitation of this source preview.
A generic GLB export may omit the companion metadata and compression expected by this packer.

## Input layout

The default cache root is `~/.cache/nades`:

| Path under the cache root | Contents |
|---|---|
| `maps/de_cache.glb` | compressed Source 2 Viewer map export and its texture buffers |
| `maps/de_cache.post.json`, `maps/de_cache.lut.bin` | post-processing metadata and colour lookup table, when exported |
| `maps/de_cache.sky.json`, `maps/de_cache.skybox.json`, `maps/de_cache.skybox.glb` | sky and 3D skybox inputs, when exported |
| `cs2/game/csgo/maps/de_cache.vpk` | the map's VPK; skybox VPKs belong beside it |
| `cs2/depot/game/csgo/pak01_dir.vpk` and numbered archives | shared materials and textures |
| `cs2/export` | Source 2 Viewer's decompiled entities and sky materials |
| `tools/s2v/Source2Viewer-CLI` | Source 2 Viewer CLI used to identify lightmapped draws |
| `packs/de_cache.pack` | generated pack loaded by the viewer |

Return to this checkout, then build and open the map:

```sh
cargo build --release
./target/release/nades-pack de_cache
./target/release/nades-sandbox --map de_cache
```

The packer prints which game textures and optional inputs it found. Without the CLI it falls back to the
web viewer's lightmap rule; without decompiled entities it omits light probes and sky. These fallbacks do not
produce the full look shown in the README. If the exporter placed the CLI in `tools/s2v-master`, pass that
location explicitly with `--cli`.

## Custom input locations

```sh
./target/release/nades-pack de_cache \
  --export /path/to/maps/de_cache.glb \
  --game /path/to/cs2/game/csgo \
  --depot /path/to/cs2/depot \
  --decompiled /path/to/cs2/export \
  --cli /path/to/Source2Viewer-CLI \
  --out "$HOME/.cache/nades/packs/de_cache.pack"
```

Companion metadata is read beside the GLB. The viewer uses `~/.cache/nades/packs/<map>.pack`.
The packer does not inherit the exporter's `NADES_CACHE` override; use the explicit flags above.

## The lineups' inputs (M3)

The map draws from its pack alone. The lineup list, the views and the effects read more, all optional (without
them the map still opens, with no lineups or with the stand-ins named below):

| Input | Default | What reads it |
|---|---|---|
| the throws: cs2-throws files | `--data <file or folder>` (repeatable), else `NADES_SANDBOX_DATA` (paths separated as in PATH), else `[data] paths` in `config.toml` in the platform's config folder, else the demo reader's cache (`throws/` in the platform's cache folder) | the lineups and each throw's flight, smoke, fire and viewpoints, read-only; a file written again is read again |
| awpy's collision mesh | `~/.cache/nades/maps/<map>.mesh` (show-me-the-nades' `just map-meshes`), or `NADES_MAPS_DIR` | the result shot's line of sight and the smoke's light; without it, an overview and an unlit smoke |
| the game's smoke noise and fire flipbooks | `~/.cache/nades/cs2/effects/web` (its `uv run scripts/cs2_effects.py web`), or `NADES_CS2_EFFECTS_DIR` | the smoke and the fire; without them, the web view's stand-ins |
| the remembered view | `~/.cache/nades/sandbox/state.json`, or `NADES_SANDBOX_STATE` | each map's last lineup, view and camera, written on exit |

A flag beats the environment, which beats the config file, which beats the default. On macOS the config file is
`~/Library/Application Support/io.github.nu-sync.nades-sandbox/config.toml`:

```toml
[data]
paths = ["~/cs2/pro.sqlite", "throws"]   # a relative path is the config folder's
```

The packs, the collider and the effects keep their `~/.cache/nades` paths until the sandbox reads maps from the game's
own files (docs/standalone-api.md §7).

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
