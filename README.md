# nades-sandbox

A native CS2 map viewer and lineup sandbox built in Rust with wgpu. It renders any official map from the game's own
files, with its materials, baked lighting, sky, fog and post-processing, and shows the grenades thrown on it, read from
CS2 demos or from `cs2-throws` files, with a fly camera and offscreen rendering from CS2 console cameras.

![de_cache rendered in nades-sandbox](docs/images/de_cache.png)

## Status

**M4 (version 0.1.0).** Tested on Apple Silicon macOS. The renderer is working. Since M3 the window
is a lineup sandbox: the lineups of the throws in your data, each seen from the thrower's eye, zoomed on the aim
point, from an opponent's eye or orbiting it, with the flight, smoke, fire, flash or HE, a time scrubber and the
practice command to copy. Since M4 every packed map stays on the GPU within a memory budget, so picking another map
shows it in the next frame, and `just bundle` makes a macOS app. The sandbox is now standalone: it reads CS2 demos
itself and packs any official map from the game's files. The workspace type-checks for Windows
(`just check-windows`) but has not been run there; Linux is not verified.

The throws come from **`cs2-throws` files**: one SQLite file of what CS2 demos record about grenade throws, named
after the game's own properties and events ([the format](docs/standalone-api.md#3-the-facts-cs2-throws-version-1)).
Any parser can write one: `tools/cs2_throws.py` is a reference writer that needs only Python's standard library, and
the sandbox's own demo reader writes one per demo (`--data match.dem`); other producers, such as the separate
`show-me-the-nades` analytics project (`nades export --cs2-throws`), are optional. Everything computed from those facts (lineup grouping, technique, practice commands,
round time, landing callouts, the opponent's viewpoints) is the sandbox's own rule, the same for every file
(`crates/model`). A producer's own labels travel as tags, shown and filterable but never interpreted.

The repository contains no game files or map packs: each machine builds its own from CS2's files. `nades-pack setup
<map>` fetches what a map needs from Steam's CS2 depot (DepotDownloader, anonymous), and `nades-pack <map>` builds its
pack with Source 2 Viewer's CLI and Bun (glTF-Transform). [Asset setup](docs/setup.md) describes the settings and the
folders.

[The specification](SPEC.md) records the milestones and historical performance measurements;
[the decision log](docs/decisions.md) records the architecture and rendering evidence.

## Build and run

Install Rust 1.95 or newer and the Xcode Command Line Tools on macOS. [just](https://just.systems/) is optional;
Python 3 is needed for the tests' fixture writer and the startup benchmarks, and [uv](https://docs.astral.sh/uv/) for
the rendering comparison script. Packing needs [Source 2 Viewer's CLI](https://github.com/ValveResourceFormat/ValveResourceFormat),
[Bun](https://bun.sh/) and, to fetch maps, [DepotDownloader](https://github.com/SteamRE/DepotDownloader). The first
build compiles bundled SQLite. The repository builds on its own.

```sh
cargo build --release
cargo test --release --workspace
cargo clippy --release --workspace --all-targets

# With the settings described in docs/setup.md (CS2's folder, the CLI, DepotDownloader):
./target/release/nades-pack setup de_cache    # fetch the map's files from Steam, once
./target/release/nades-pack de_cache          # build its pack
./target/release/nades-pack --effects         # once: the smoke noise and fire flipbooks every map shares
./target/release/nades-sandbox --map de_cache --data match.dem
```

`--data` names a CS2 demo, a cs2-throws file, or a folder of either (repeatable; a demo in two files is taken once).
A demo is read once with awpy into the cache (`throws/<sha256>.sqlite` in the platform's cache folder, about 4 s and
3.5 MB for a pro match) and from there afterwards; a demo dropped onto the window is read the same way, on a thread of
its own, with its progress in the right panel. Without `--data` the sandbox reads `NADES_SANDBOX_DATA`, then
`[data] paths` in `config.toml` in the platform's config folder
(`~/Library/Application Support/io.github.nu-sync.nades-sandbox/` on macOS), then that cache. Check what a file allows
before opening it:

```sh
./target/release/nades-sandbox check throws.sqlite    # the file's problems, and each feature: on, off, or what fact it lacks
python3 tools/cs2_throws.py fixture /tmp/fixture.sqlite   # a small worked example of the format
```

Pick a lineup on the left; the arrow keys step through them, 1–4 pick the Lineup, Aim, Result and Orbit views, space
plays the time, and dragging turns the orbit. WASD, E/Q and the right mouse button fly a free camera.

With just installed:

```sh
just                       # list commands
just pack de_cache
just run de_cache
just check                 # justfile formatting, workspace tests and Clippy
just test-local            # ignored tests against local game files, exports and packs
just startup               # startup, steady frames and memory
just bundle                # the macOS app: target/release/bundle/macos/Nades Sandbox.app
just check-windows         # type-check and lint for Windows (DX12) from a Mac
```

The app opens the other packed maps in the background once its first frame is on screen, as long as they fit an 8 GiB budget
(less on machines with less memory; `--budget-mb` sets it). `python3 bench/resident.py` measures map switches and
memory with every map in one session.

Render one frame from a CS2 console camera or a throw's view, or print the startup stages:

```sh
./target/release/nades-sandbox --render shot.png --map de_dust2 --setpos 1,2,3 --setang 0,90
./target/release/nades-sandbox --render smoke.png --throw <throw id> --view result --time 4 --data throws.sqlite
./target/release/nades-sandbox --exit-after-first-frame
```

Replay a round: in the window, **Replay** in the top bar picks a demo and a round, plays it with every player and
grenade, and follows a player through their eyes or from behind (space plays, ←/→ jump 5 s, esc goes back to the
lineups; each throw in the list opens its lineup view). Offscreen, a moment as a PNG with its facts beside it as JSON,
or the frames timed:

```sh
./target/release/nades-sandbox --data match.dem --replay match --round 16 --time 28.5 --replay-camera chase:ropz --render r16.png
./target/release/nades-sandbox --data match.dem --replay match --round 16 --time 5 --bench-replay 600 --size 1444x1420
python3 bench/replay_check.py --data <cs2-throws files>   # players and effects drawn where the facts put them
```

`--replay` names a demo by its id, the id's start or its file's name; `--replay-camera` is `overview` (the default),
`top`, a player's name (their eyes) or `chase:<player>`.

Where a show-me-the-nades checkout is at hand, the views and the effects can be checked against its web view (the
sandbox's cameras and effects are ports of it). Both read a clone of its database, the sandbox through the clone's
cs2-throws export; the checkout is named, never assumed:

```sh
python3 bench/cameras.py --nades <show-me-the-nades checkout>   # every view's camera for 84 throws, against the web view's own choice
uv run bench/effects.py --nades <show-me-the-nades checkout>    # result shots of chosen throws beside the web view's
cargo run --release -p nades-source-file --example tag_parity -- <export> nades   # the sandbox's groups, techniques and callouts against the file's nades.* tags
```

`python3 bench/startup.py` records load averages and stage marks, discards the first launch, and reports the
median of nine measured launches. Performance depends on the machine, system load and prepared assets.
`uv run bench/realism.py --nades <checkout> --note "what changed"` additionally requires that project's private local
reference set and web-view runs. Those reference images and generated comparison outputs are not published.

## Agent control

Build with `just build`, then configure an MCP client to run the absolute path to
`target/release/nades-mcp` with stdio transport. The server starts its sibling sandbox binary directly.
`just mcp` is the development entry point; `just verify-mcp` exercises the actual server and app.
For video, install FFmpeg with `libx264` and `ffprobe` for verification.

Start with `app_status` and `maps_list`, then use `app_start`, `camera_set` and `frame_capture`.
`view_render` takes a headless screenshot without changing the visible window. `diagnostics_get` exposes startup
stages, CPU submission metrics, measured memory and recent logs. Images identify the map, camera, dimensions and
animation time; live captures also identify the instance, revision and frame.
Map changes restart the managed window. Disconnecting MCP hands a ready window to you; a future session starts its
own instance. Manually opened windows are never controlled.
Logs and private view state live in `~/.cache/nades/sandbox/mcp` by default. `--runs-dir`, `--packs-dir` and
`--sandbox-binary` provide explicit development overrides. `view_render` renders base maps. Use `lineups_list` to find a recorded throw, `video_render` to render it
headlessly, and `video_status` to retrieve the completed local MP4; `video_cancel` cancels a running job.

The [MCP specification](docs/mcp-spec.md) defines each tool and its success criteria. The
[verification skill](.agents/skills/verify-mcp/SKILL.md) describes repeatable local acceptance.

## Layout

| Path | Role |
|---|---|
| `crates/sandbox` | window, the lineup shell, the views, startup clock |
| `crates/control` | shared camera, scene state and application command contracts |
| `crates/mcp` | nine agent tools, managed processes and real protocol verification |
| `crates/render` | GPU renderer, materials, lighting, post-processing and console cameras |
| `crates/pack` | per-map pack format, writer and memory-mapped reader |
| `crates/source2` | VTEX textures and VPK archives |
| `crates/packer` | `nades-pack`: fetches a map's files (`setup`) and builds its pack from the game's files: the geometry export, textures, light, sky, post-processing, collider; and the shared effects pack |
| `crates/model` | the sandbox's vocabulary and rules: CS2's facts, the `ThrowSource` port, grouping, technique, practice commands, round time, callouts, viewpoints, the smoke's voxel-log decoder, and `Library` |
| `crates/source-file` | the `cs2-throws` file adapter: version 2 SQLite files, several merged into one source, and `check` |
| `crates/source-demo` | the demo adapter: a CS2 `.dem` file, read with awpy, becomes a cs2-throws file in the cache |
| `crates/config` | where the settings come from (data, packs, CS2's files, the tools): a flag, `NADES_SANDBOX_*`, the config file, the default |
| `crates/scene` | a throw as a stage: CS2's cameras, the result shot through the map's collider, the smoke's cloud and light, the fire |
| `tools/cs2_throws.py` | the format's schema and a reference writer, Python's standard library only |
| `bench/` | benchmark camera, startup timing and rendering comparisons |
| `SPEC.md`, `docs/` | plan, setup and design evidence |

Historical engine experiments mentioned in the design documents remain local under `prototypes/` and are not
included in this preview. The production workspace contains the chosen implementation.

## License

The project's source code is [MIT licensed](LICENSE). CS2 game assets are not included or covered by that
license. Game files, exports, packs and data files stay local. The screenshot above is rendered output
from this application. This is an independent project, unaffiliated with Valve.
