# Local asset setup

The M2 viewer reads a prepared map pack. The packer builds it from a Source 2 Viewer GLB export and local CS2
files. Neither game assets nor prepared packs are distributed with this repository.

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

## Database and verification inputs

The viewer does not need a database at M2. To exercise the data crate, use a copy of a database produced by
`show-me-the-nades`, never the live file:

```sh
cargo run --release -p nades-data --example lineups -- /path/to/copy.duckdb de_cache
```

`cargo test --release --workspace` uses synthetic fixtures. The ignored tests require local game files,
exports and packs. `bench/realism.py` also requires the sibling checkout's `verify-scene-realism` skill,
reference shots and latest web rendering run; it reads those in place. Its generated comparisons stay local.
