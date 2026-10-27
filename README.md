# nades-sandbox

A native CS2 map and grenade simulator built in Rust with wgpu. It renders any official map from the game's own files,
with its materials, baked lighting, sky, fog and post-processing. Pick a map, stand somewhere, aim, choose how to throw,
and see the grenade fly through the map, bounce and go off: the smoke filling around walls, the fire spreading over the
floor, the flash, the HE.

![de_cache rendered in nades-sandbox](docs/images/de_cache.png)

## Status

**The simulator (version 0.1.0, S31).** Tested on Apple Silicon macOS. Nothing in it depends on recorded matches:
lineup libraries, team and player statistics and demo analysis belong to other applications
([docs/simulator.md](docs/simulator.md)). The window has:

- **map selection**, every packed map kept on the GPU within a memory budget, so another map shows in the next frame;
- **the throw tool**: stand where the free camera stands (or click a spot on the map to stand there), aim with the
  camera, choose the grenade (smoke, flash, HE, molotov, incendiary, decoy), the movement (stand, walk, run,
  jump-throw, run jump-throw), crouch and the click (left, middle, right), with a live preview arc; **Throw** (T) adds
  it to the scene;
- **the scene**: every placed throw, listed, to select, re-aim, edit, duplicate or delete, each with its start time;
  play, pause and scrub, every grenade flying and going off at its own time;
- **views**: free, the thrower's eye, following the grenade, an orbit of what it left (1–4);
- **the practice command** of the selected throw (`setpos …; setang …`), to copy into CS2;
- **scene files** to save and open (or drop on the window): JSON, `"format": "nades-sandbox-scene"`, version 1 — the one
  generic port, which other applications can write to show their throws here.

The flight comes from `crates/physics` (CS2's release model, bounces, fuses), the smoke's grid and the fire's flames
from `crates/effects`, both against the map's own collider; the smoke and fire are drawn as show-me-the-nades' web view
draws them. `just bundle` makes a macOS app. The workspace type-checks for Windows (`just check-windows`) but has not
been run there; Linux is not verified.

The repository contains no game files or map packs: each machine builds its own from CS2's files. `nades-pack setup
<map>` fetches what a map needs from Steam's CS2 depot (DepotDownloader, anonymous), and `nades-pack <map>` builds its
pack with Source 2 Viewer's CLI and Bun (glTF-Transform). [Asset setup](docs/setup.md) describes the settings and the
folders.

[The specification](SPEC.md) records the milestones and performance measurements; [the decision log](docs/decisions.md)
the architecture and its evidence.

## Build and run

Install Rust 1.95 or newer and the Xcode Command Line Tools on macOS. [just](https://just.systems/) is optional;
Python 3 runs the startup benchmarks, and [uv](https://docs.astral.sh/uv/) the rendering comparison script. Packing
needs [Source 2 Viewer's CLI](https://github.com/ValveResourceFormat/ValveResourceFormat), [Bun](https://bun.sh/) and,
to fetch maps, [DepotDownloader](https://github.com/SteamRE/DepotDownloader).

```sh
cargo build --release
cargo test --release --workspace
cargo clippy --release --workspace --all-targets

# With the settings described in docs/setup.md (CS2's folder, the CLI, DepotDownloader):
./target/release/nades-pack setup de_mirage   # fetch the map's files from Steam, once
./target/release/nades-pack de_mirage         # build its pack
./target/release/nades-pack --effects         # once: the smoke noise and fire flipbooks every map shares
./target/release/nades-sandbox --map de_mirage
```

In the window: WASD, E/Q and Shift fly, the right mouse button held looks; aim and press T to throw. Space plays the
scene's clock, 1–4 pick the Free, Thrower, Follow and Orbit views, Esc goes back to the free camera, Delete removes the
selected throw, and dragging turns the orbit. Each map keeps its own scene between runs; **Save** writes it to
`~/nades-scenes/<map>.json` (or the path typed), **Open** reads one back.

A scene file, offscreen: one moment as a PNG from a view of one of its throws (or a CS2 camera), and what each throw
did as JSON lines (its release, bounces, where and when it went off, its smoke's voxels or fire's flames):

```sh
./target/release/nades-sandbox --scene smokes.json --time 4 --view orbit --throw 0 --render smoke.png
./target/release/nades-sandbox --scene smokes.json --time 1.2 --view follow --render flight.png --size 1280x720
./target/release/nades-sandbox --render shot.png --map de_dust2 --setpos 1,2,3 --setang 0,90   # the bare map
./target/release/nades-sandbox --exit-after-first-frame                                       # start-up stages
```

```json
{"format": "nades-sandbox-scene", "version": 1, "map": "de_mirage",
 "throws": [{"grenade": "smoke", "feet": [1250.0, 0.0, -166.3], "pitch": -20.0, "yaw": 180.0,
             "crouch": false, "movement": "jumpthrow", "click": "left", "start": 0.0}]}
```

With just installed, the whole workflow is a few commands (`just` lists them all):

```sh
just doctor                       # settings, packing tools, packed maps: what is missing
just maps                         # fetch and pack every pool map and the shared effects (once per machine)
just map de_vertigo de_train      # any other official map: fetch and pack in one step
just run de_mirage                # the simulator on a map; nades-sandbox flags pass through
just scene smokes.json            # open a scene file in the window
just simulate smokes.json 4 shot.png orbit 0   # a scene's moment offscreen, with what each throw did
just bundle                       # the macOS app: target/release/bundle/macos/Nades Sandbox.app
just check                        # justfile formatting, tests and Clippy (warnings as errors)
just startup                      # startup, steady frames and memory (window on screen)
just check-windows                # type-check and lint for Windows (DX12) from a Mac
```

The app opens the other packed maps in the background once its first frame is on screen, as long as they fit an 8 GiB
budget (less on machines with less memory; `--budget-mb` sets it). `python3 bench/resident.py` measures map switches and
memory with every map in one session. `python3 bench/startup.py` records load averages and stage marks, discards the
first launch, and reports the median of nine measured launches. `uv run bench/realism.py --nades <checkout> --note
"what changed"` compares the maps' look with the game's; it requires show-me-the-nades' private local reference set,
which is not published.

## Agent control

Build with `just build`, then configure an MCP client to run the absolute path to `target/release/nades-mcp` with stdio
transport. The server starts its sibling sandbox binary directly. `just mcp` is the development entry point;
`just verify-mcp` exercises the actual server and app. For video, install FFmpeg with `libx264` (and `ffprobe` for
verification).

Start with `app_status` and `maps_list`. Headless, `view_render` draws a map from a CS2 camera, or a scene at a moment
from a view of one of its throws, and reports what each throw did; `video_render` plays a scene into an MP4
(`video_status`, `video_cancel`). In a window (`app_start`), `scene_get` and `scene_set` read and replace its scene,
`throw_add`, `throw_update` and `throw_remove` edit it, `time_set` moves or plays its clock, `view_set` shows a throw's
view, `camera_set` moves the free camera and `frame_capture` shows the result. `diagnostics_get` exposes startup stages,
CPU submission metrics, measured memory and recent logs. Map changes through `map_open` restart the managed window; a
scene on another map switches it in place. Disconnecting MCP hands a ready window to you; manually opened windows are
never controlled. Logs and private view state live in `~/.cache/nades/sandbox/mcp` by default; `--runs-dir`,
`--packs-dir` and `--sandbox-binary` provide explicit development overrides.

The [MCP specification](docs/mcp-spec.md) defines the tools and their success criteria.

## Layout

| Path | Role |
|---|---|
| `crates/sandbox` | the window and the simulator: the scene and its simulations (`scene.rs`, `sim.rs`), the throw tool, the views, the shell, offscreen scenes (`offscreen.rs`), the resident maps, the startup clock |
| `crates/physics` | `nades-physics`: CS2's grenade throw — the release, the flight with its bounces, the detonation, the practice command |
| `crates/effects` | `nades-effects`: what a grenade leaves — the smoke's voxel grid and the fire's flames, from the map's collider |
| `crates/scene` | what the renderer draws of a throw: its stage (flight, cloud, fire, burst, the result shot), CS2's cameras, the collider, the smoke's cloud and light, the fire |
| `crates/control` | the scene file, cameras, scene state and the app-control commands, shared by the app and the MCP server |
| `crates/mcp` | the agent tools, managed processes and real protocol verification |
| `crates/render` | GPU renderer, materials, lighting, post-processing, the throws' layer and the smoke's composite |
| `crates/pack` | per-map pack format, writer and memory-mapped reader |
| `crates/source2` | VTEX textures and VPK archives |
| `crates/packer` | `nades-pack`: fetches a map's files (`setup`) and builds its pack from the game's files: the geometry export, textures, light, sky, post-processing, collider; and the shared effects pack |
| `crates/model` | CS2's own encodings the sandbox keeps: the smoke's voxel log, the release model's maths |
| `crates/config` | where the settings come from (packs, CS2's files, the tools): a flag, `NADES_SANDBOX_*`, the config file, the default |
| `bench/` | benchmark camera, startup timing, resident maps and the look against the game |
| `SPEC.md`, `docs/` | plan, setup and design evidence |

Historical engine experiments mentioned in the design documents remain local under `prototypes/` and are not
included. The production workspace contains the chosen implementation.

## License

The project's source code is [MIT licensed](LICENSE). CS2 game assets are not included or covered by that
license. Game files, exports, packs and data files stay local. The screenshot above is rendered output
from this application. This is an independent project, unaffiliated with Valve.
