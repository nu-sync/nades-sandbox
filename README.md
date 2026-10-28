# nades-sandbox

A native CS2 map and grenade simulator built in Rust with wgpu. It renders any official map from the game's own files,
with its materials, baked lighting, sky, fog and post-processing. Pick a map, stand somewhere, aim, choose how to throw,
and see the grenade fly through the map, bounce and go off: the smoke filling around walls, the fire spreading over the
floor, the flash, the HE.

![de_cache rendered in nades-sandbox](docs/images/de_cache.png)

## Status

**A lineup renderer for agents (version 0.1.0, S31, S34).** Tested on Apple Silicon macOS. Its main use: an agent
renders any grenade lineup found online, headlessly. One MCP call, `lineup_render`, takes the map, the grenade, the
lineup's console line as posted (`setpos 1296.00 32.00 -167.97; setang -28.89 -166.00 0`) and the technique in words
("jumpthrow", "run jumpthrow", "crouch", "right click"), and returns an MP4 played like jumpthrow.pro's lineup videos:

1. where to stand, wide;
2. the aim, with the crosshair;
3. a close-up of what the crosshair lines up with;
4. the throw;
5. the camera following the grenade (the game's own model) through the air;
6. the landing;
7. the effect blooming;
8. a slow 360° turn around it.

There is no crosshair after the release and no drawn paths. The defaults are 1920×1080 at 60 fps, with the timing from
the grenade's airtime and effect (a smoke's video is about 26 s and renders in about 35 s). Stills of the aim, the aim
point, the flight, the bloom and the result, and a JSON of what happened (release, bounces, landing, detonation,
effect), are written beside it. Nothing in it depends on recorded matches
([docs/simulator.md](docs/simulator.md)).

Through MCP, for more control:

- **throws**: a grenade (smoke, flash, HE, molotov, incendiary, decoy) and a spot, given as CS2's console string or
  feet, pitch and yaw, with the movement, crouch, click and a start time; `throws_simulate` reports what each does
  without rendering;
- **scenes** on any packed map, **images** (`view_render`) and **videos** with scripted shots (`video_render`: lineup,
  follow, orbit, thrower, hold moves);
- **a window**, secondary, showing only the 3D view: scenes, the clock, views (free, the thrower's eye, following the
  grenade, an orbit of the result) and cameras;
- **scene files** (JSON, `"format": "nades-sandbox-scene"`, version 1), the generic port other applications can write.

`--ui` (or F1 in the window) shows the developer shell: the throw tool with its preview arc, the scene's list, the
selected throw, the clock and an Arcs switch for the flight paths. `nades-pack --effects` packs the grenades' world
models with the smoke noise and the fire's flipbooks; without them a grenade flies as a capsule.

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

In the window: WASD, E/Q and Shift fly, the right mouse button held looks; F1 shows the developer shell (throw tool,
scene, clock: T throws, space plays, 1–4 pick the views).

A scene file, offscreen: one moment as a PNG from a view of one of its throws (or a CS2 camera), and what each throw
did as JSON lines (its release, bounces, where and when it went off, its smoke's voxels or fire's flames):

```sh
./target/release/nades-sandbox --scene smokes.json --report                    # what each throw did, no GPU
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
just report smokes.json            # what each throw did, simulated with no GPU
just video request.json out.mp4    # a scene's video from a request (a shot: follow, then orbit), with FFmpeg
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
`just verify-mcp` exercises the actual server and app (66 checks). For video, install FFmpeg with `libx264` (and
`ffprobe` for verification).

A request such as "render this smoke on Mirage: `setpos 1296.00 32.00 -167.97; setang -28.89 -166.00 0`, jumpthrow" is
one call:

```json
lineup_render {"map": "mirage", "grenade": "smoke", "technique": "jumpthrow",
               "lineup": "setpos 1296.00 32.00 -167.97; setang -28.89 -166.00 0",
               "output": "/abs/path/mirage-smoke.mp4"}
```

It waits and returns the MP4, the stills, the JSON and where and when the grenade went off. Headless tools: `lineup_render` (one lineup, end to end), `throws_simulate` (what each throw did, a second or two),
`view_render` (a map from a camera, or a scene's moment from a view of a throw), `video_render` / `video_status` /
`video_cancel`. Window tools (`app_start` returns the `instance_id` they take): `scene_get`, `scene_set` (also how to
change the map in place), `throw_add`, `throw_update`, `throw_remove`, `time_set`, `view_set`, `camera_set`,
`camera_look_at`, `frame_capture`, `map_open`, `app_stop`. `app_status` and `diagnostics_get` report state, startup
stages, CPU submission metrics, memory and logs. Disconnecting MCP hands a ready window to you; manually opened windows
are never controlled. Logs and private view state live in `~/.cache/nades/sandbox/mcp` by default; `--runs-dir`,
`--packs-dir` and `--sandbox-binary` provide explicit development overrides.

The [MCP specification](docs/mcp-spec.md) defines the tools and their success criteria.

## Layout

| Path | Role |
|---|---|
| `crates/sandbox` | the window (the 3D view; the developer shell behind `--ui`/F1) and the simulator: the scene and its simulations (`scene.rs`, `sim.rs`), scripted shots (`shot.rs`), the views, offscreen scenes, reports and videos (`offscreen.rs`), the resident maps, the startup clock |
| `crates/physics` | `nades-physics`: CS2's grenade throw — the release, the flight with its bounces, the detonation, the practice command |
| `crates/effects` | `nades-effects`: what a grenade leaves — the smoke's voxel grid and the fire's flames, from the map's collider |
| `crates/scene` | what the renderer draws of a throw: its stage (flight, cloud, fire, burst, the result shot), CS2's cameras, the collider, the smoke's cloud and light, the fire |
| `crates/control` | the scene file, throw inputs (console strings) and reports, video shots, cameras, scene state and the app-control commands, shared by the app and the MCP server |
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
