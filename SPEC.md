# nades-sandbox: specification

A native CS2 map and grenade simulator: pick a map, stand somewhere, aim, choose how to throw, and see the grenade fly
through the map, bounce and go off, instantly ([docs/simulator.md](docs/simulator.md)). Its maps come from the game's own
files, packed on this machine. Nothing in it depends on recorded matches; lineup libraries, statistics and demo analysis
belong to other applications (`show-me-the-nades`, a separate project, keeps its own).

Status: **MCP-first** (2026-10-27, S34). The person talks to an agent, and the agent drives the simulator through
`nades-mcp`; the window shows only the 3D view (`--ui` or F1 for the developer shell).
- Throws are given as CS2's console string (`setpos …; setang …`) or feet/pitch/yaw; every tool that places throws
  returns what each did (release, bounces, landing, detonation, effect). `throws_simulate` answers that with no window
  and no GPU in a second or two; `camera_look_at` points the camera; `video_render` plays a scripted shot (follow the
  flight, a 360° orbit round what it left at a radius and height, the thrower's eye, a fixed camera) to an MP4 at a
  path of the agent's choosing.
- Acting as the agent over stdio, the user's first case (mirage-smoke-0002: a jump-throw smoke from `setpos 1296.00
  32.00 -167.97; setang -28.89 -166.00 0`) went off at (−970, −587, −286) 6.16 s after the release, against the game's
  (−1164, −630, −166) at 6.73 s: 232 units apart (199 across, 120 down), 0.57 s early (S32's smoke tail). Its video,
  following the flight and circling the smoke 360°, is 18.3 s of 1280×720 H.264.
- Flashes and HEs go off as a white-hot pop the bloom spreads; the "flat grey disk" was the arcs' release mark at the
  thrower's hand, gone.

Before it, **the simulator** (2026-10-27, S31).

- The recorded-data features are gone: the lineup library and its filters, the data adapters (cs2-throws files, the
  demo reader, `check`, `--data`), the replay, the viewpoints and callouts from recorded players, `crates/model`'s data
  modules, the MCP tools about recorded throws and the benches that compared recorded throws with the web view. The
  decisions behind them (S24, S25, S28, S30) and docs/standalone-api.md stay as history.
- The throw tool, the scene (list, select, re-aim, edit, duplicate, delete, start times), play, pause and scrub with
  every grenade flying and going off at its own time, the four views (free, the thrower's eye, following the grenade,
  an orbit of the result), the practice command, and scene files (JSON, `nades-sandbox-scene` version 1) to save and
  open. Each throw is simulated once per spot, angles and technique on the thread pool, after the first frame: its
  flight by `crates/physics` (S32), its smoke's grid and fire's flames by `crates/effects` (S33), drawn by the scene's
  ports of the web view's smoke and fire.
- `--scene f.json --time s --render out.png` draws a scene's moment offscreen and reports what each throw did; MCP tools
  place and edit throws, play time and render images and videos of scenes.
- The first frame is unchanged: de_cache on screen 139.3 ms from process start against 139.0 before the simulator
  (medians of 7 alternated launches, load ≈18).

Before it, **M5** (2026-10-26):
Status: **M5 built** (2026-10-26; its windowed checks wait for a screen, S30).
- M5, the replay: a demo's round on the map in 3D with every player (where they stand and look, alive or dead, in
  their side's colour, faint through walls) and every grenade in flight, then its smoke, fire, flash or HE at its own
  time; play, pause, scrub and speed; a free camera, a player's eyes or a camera behind them; a lineup view opened from
  any throw in the round. Players move between points 16 a second (cs2-throws 2.1's `player_tracks`, which the demo
  reader writes), else between their samples. Offscreen, `--replay` renders a moment and `--bench-replay` times the
  frames: 3.9–4.5 ms median at 1444×1420 with up to 8 throws live, the moment's set-up 0.5–0.6 ms. Checked against the
  facts in 11 moments on four maps: every living player drawn where its track puts it, in its side's colour
  (`bench/replay_check.py`).
- Every pool map stays on the GPU. Once the first frame is on screen the other six open in the background, one at a
  time, with their lineups read and their first stages built, all within 1.6–1.75 s of start-up. The seven hold 6.2 GB
  of an 8 GiB budget; a map that does not fit lets go of the one shown least recently (S19).
- Picking a map shows it at its lineup in the next frame: 42 of 42 picks, the switching frame 5.3 ms (median, worst
  8.9) and 5.7 ms on a map's first visit in a session (worst 8.0), where M3 took 1.55–1.72 s.
- de_cache's 2.0 GB peak is accounted for, and it is not the upload's staging (S20). de_cache is on screen 168.8 ms
  after the process starts and de_inferno 185.1 ms (medians of 9; 167–170 and 178–194 over three runs each); the first
  frame no longer waits on the GPU (S21).
- `just bundle` makes a macOS app that runs from Finder (S22). The workspace checks and lints clean for Windows
  (x86_64-pc-windows-msvc, DX12); it has not run on Windows yet (S23).
- Standalone, step 1 (S24): the throws come from cs2-throws files through a `ThrowSource`, and grouping, technique,
  practice commands, round time, landing callouts and viewpoints are the sandbox's own rules (`crates/model`); the
  workspace builds with no sibling checkout.
- Standalone, step 2 (S28): CS2 demos are read directly (`crates/source-demo`, awpy): `--data` takes `.dem` files and
  folders, a demo dropped onto the window is imported with its progress shown, and each is parsed once into the cache.
  On six ESL Pro League demos its files match show-me-the-nades' export row for row.
- Standalone, step 3 (S26, S29): packs come from the game's files alone. `nades-pack setup <map>` fetches a map from
  Steam's CS2 depot; `nades-pack <map>` makes its geometry export, collider, light, sky and post-processing records
  itself, with Source 2 Viewer's CLI and glTF-Transform; `nades-pack --effects` packs the smoke's noise and the fire's
  flipbooks once. Every pool map's pack rebuilds byte for byte, and de_vertigo, outside the pool, packs and renders.
- M3 made it the lineup sandbox: a map's lineups from the nades database, cut by side, grenade, team, player and phase;
  each seen from the thrower's eye, zoomed on the aim point, from an opponent's eye or orbiting it, with the flight, the
  smoke, fire, flash or HE as the round had it, a time scrubber and the practice command; every view of 84 throws opens
  from exactly the web view's camera, and a lineup switch costs the window 0.5 ms (S16–S18).
- M2 gave the map CS2's look: materials, baked light and sun shadows, light probes, sky, 3D skybox, fog, bloom, tone
  curve and colour lookup table, each pool map differing from the game no more than the web view does (S12). M1 put
  the textured de_cache on screen in 111 ms; it was built twice, as our own renderer and with Bevy as the shell
  ([docs/m1-bakeoff.md](docs/m1-bakeoff.md)), and the user chose our own renderer (S7).

## 1. What it is like to use

You open it, and the map you last looked at is on screen in about a seventh of a second, with the throws you left on it.

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│ de_mirage ▾   Free  Thrower  Follow  Orbit   ☑ Arcs                                  │
├────────────────────┬───────────────────────────────────────────────┬─────────────────┤
│ THROW              │                                               │ Throw 1         │
│ smoke flash HE …   │                                               │ smoke · jump    │
│ stand walk run …   │        the map from where you stand,          │ feet, pitch/yaw │
│ ☐ crouch  L M R    │        the preview arc as you aim,            │ Re-aim Dup Del  │
│ starts at 0.0 s    │        the grenades flying and going off      │ goes off 1.9 s  │
│ [Stand on a spot]  │                                               │ setpos … setang │
│ [Throw (T)]        │                                               │ [copy]          │
│ SCENE  3 throws    │                                               │                 │
│ 1. smoke · 0.0 s   │                                               │                 │
├────────────────────┴───────────────────────────────────────────────┴─────────────────┤
│ Play  ────●───────────────────  2.40 / 21.3 s                         first frame 128 ms │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

- **The throw tool** stands where the free camera stands (its eye 64 units over the feet, 46 crouched), or on a spot
  clicked on the map, and aims where the camera looks; the preview arc follows the aim. T throws.
- **The scene** lists every throw: select it to change its grenade, movement, crouch, click, start, feet or angles,
  re-aim it from the camera, duplicate or delete it. Space plays the scene's clock; each grenade is thrown at its own
  start, flies, bounces and goes off.
- **Views** (1–4): free; the thrower's eye as it lets go; behind the grenade in flight, then over what it left; an orbit
  of the result, opening on a shot that sees it.
- **Copy** puts the selected throw's practice command (`setpos`, `setang`) on the clipboard.
- **Scene files** save and open a map's throws as JSON; a file dropped on the window opens.
- **Switching maps** shows the other map in the next frame: every pool map stays on the GPU (0.37–1.43 GB each), and
  each keeps its own scene.

## 2. Why native, measured

The web view is fast now: a map that is already loaded opens in about 0.1 s. The native app wins where the browser
cannot follow (show-me-the-nades' `docs/research/native-sandbox.md` and the measurements below):

| | Browser (today's web app) | Native (measured here) |
|---|---|---|
| First frame of a map, cold | Cache orbit: 1.5 s to the map's first frame shown, 2.2 s settled; bare three.js 1.0 s | on screen 172 ms from process start in CS2's look (M2), 111 ms lit by its lightmap alone (M1) |
| Texture memory | RGBA8 decoded from WebP: 0.5 GB (Mirage) to 1.7 GB (Inferno) of pixels per map | the game's own BC blocks, 4× smaller |
| A smoke's light (≈34,000 rays against the map) | 200–270 ms on the main thread | 2.4–2.6 ms on 18 cores, built before the lineup is picked (M3, S16) |
| Memory ceiling | a tab's renderer dies at ≈16 GiB (D26) | the machine's memory |
| Frame of the whole map | 15–18 ms at 1440×900 with CS2's shading and a smoke | 4.2 ms at 2880×1800 with CS2's shading and post-processing; 5.6–7.1 ms (median) at 1444×1420 with a smoke in view (M3) |

## 3. Scope

**Now:** any official map, packed from the game's files (the seven pool maps checked against the game and the web view,
de_vertigo packed beside them), every packed map resident; grenades placed, simulated and played (docs/simulator.md);
scene files; agent control.

**Not here:**
- lineup libraries, team and player statistics, demo analysis and replays: other applications, which can write a scene
  file to show their throws here;
- any distribution of Valve's assets: packs are built on this machine and stay there.

## 4. Decisions

The stack, with what each choice rests on. [docs/decisions.md](docs/decisions.md) has the evidence and the rejected options.

| Concern | Choice | Version |
|---|---|---|
| Language | Rust 2024 edition, the same toolchain as nades | 1.95 |
| GPU | wgpu: Metal on macOS, DX12 or Vulkan on Windows | 30.0.1 |
| Window and input | winit (stable line; 0.31 is in beta) | 0.30.13 |
| UI | egui, drawn by egui-wgpu into the frame's last pass, input from egui-winit | 0.37 |
| Engine | none: our own renderer over wgpu | |
| Shaders | WGSL, checked by naga at build time, composed with naga_oil, a few uber-shaders with overridable constants | naga 30.0.1, naga_oil 0.23 |
| Data | scene files in JSON (`crates/control`, serde); the config file in TOML, its folder from `directories` (until S31: cs2-throws files in SQLite, rusqlite) | serde_json 1, toml 1, directories 6 |
| Assets | packs memory-mapped (memmap2), tables in postcard, LZ4 (lz4_flex), our own VTEX and VPK readers; meshopt for the export's geometry; when packing, Source 2 Viewer's CLI (export and decompiles, S9, S26), glTF-Transform through Bun (S29), DepotDownloader for `setup` | 0.9, 1.1, 0.14, 0.7; glTF-Transform 4.5.1 |
| Maths, threads | glam, rayon | 0.34, 1.12 |
| Profiling | start-up marks (JSON), wgpu-profiler (GPU timestamps; built on the same wgpu 30), tracing with Tracy | 0.28, 0.1 |
| Collision | the map's physics, reduced as awpy reduces it (byte for byte awpy's published mesh), in the pack's Collider section, and a BVH over it, as the web view does since D26 (S26) | |

Why not an engine, measured (`prototypes/engine-bakeoff`, decision S2): Bevy 0.20, on Rust 1.99.1 and the same
wgpu and winit as ours, configured at its leanest. To the empty window's first frame it costs a median +58 ms per
alternated pair (two sets of five), twice the memory (205 MB against 100), ten times the binary (96 MB against 9.2) and
four times the clean build (88 s against 21). With content, its asset path cannot take the pack's pipelined upload, and
we would bypass its PBR and glTF loader for CS2's materials and our packs anyway.

- **Fyrox 1.0** draws through OpenGL only: macOS stops at OpenGL 4.1, with no compute shaders and no BC6H or BC7.
- **three-d** is OpenGL too.
- **Raw Metal:** its extras (direct storage loads, binary archives, MetalFX) would mostly hide behind AppKit's ≈76 ms
  on the first frame, at the cost of Windows. wgpu's `as_hal` reaches Metal for one path if a measurement ever calls
  for it.

The toolchain decides nothing: every library above builds on Rust 1.95 and newer, and 1.99.1 is installed beside it.

## 5. Architecture

```
crates/sandbox   the app: winit window, egui shell, views, input, start-up clock, the maps kept on the GPU
                 (resident.rs)                                                          (bin: nades-sandbox)
crates/render    wgpu device, passes, pipelines, uploads from packs, frame graph
crates/pack      the per-map pack: format, writer, memory-mapped reader                   (format 2)
crates/source2   CS2's files: VTEX textures (BC blocks, mips, cubes, volumes), VPK archives; later KV3
crates/packer    `nades-pack <map>`: builds a map's pack from the export, the game's files and their entities
crates/model     CS2's own encodings it keeps: the smoke's voxel log, the release model's maths
crates/physics   CS2's grenade throw: the release, the flight with its bounces, the detonation, the practice command (S32)
crates/effects   what a grenade leaves: the smoke's voxel grid and the fire's flames, from the map's collider (S33)
crates/config    where the settings come from (§8): flag, environment, config file, default
crates/scene     what the renderer draws of a throw: its stage (flight, cloud, fire, burst, result shot), CS2's cameras,
                 the collider, the smoke's cloud, volume and light, the fire (ported number for number from the web view)
crates/control   the scene file and app control's protocol; crates/mcp its MCP server
                 the simulator: crates/sandbox's `scene.rs` (each map's scene, the clock, the tool, the views) and
                 `sim.rs` (a throw through the physics and the effects into a stage; a moment of the scene on the GPU),
                 crates/render's `set_arcs`, `set_moment` and the composite's clouds
```

### Start-up, on two threads, then three more

```
main      main ─ EventLoop (AppKit, ~30 ms) ─ launch (~14) ─ window + CAMetalLayer (~32) ─ surface ─┐
loader    main ─ mmap pack ─ device (~3–14) ─ buffers and textures, pipelined (~40–60) ─ pipelines ─┴─ first frame ─ on screen
                                                                                                                    │
collider  the pack's collider and its BVH (6–39 ms) ─────────────────────────────────────────────────────────────────────────┤
effects   the game's smoke noise and fire flipbooks ─────────────────────────────────────────────────────────────────┘
throws    each throw simulated on every core (rayon), once the collider is there: its flight, its effect, its stage
maps      once the window shows: the other packed maps, one at a time, each drawn offscreen (S19)
```

- The first frame needs no simulation: it is drawn from the camera the last run left on that map (S16), and the
  scene's throws are simulated after it (S31).

- The window and the map arrive together. Measured on M4's de_cache (medians of 9 from process start, load ≈10,
  `.runs/startup-20261026-163255/`):
  - the window exists at 114 ms (AppKit made it 43 ms after `resumed`; 38 in M3's run, 56–62 under more load);
  - the map is ready at 131 ms, its 1 GB of textures uploaded and two frames drawn offscreen from the first frame's
    camera;
  - the first frame is presented at 139 ms, after waiting 4.5 ms for the GPU (21 ms in M3: a map's second frame still
    costs the GPU several times a steady one, so the loader now draws two, S21);
  - macOS shows the window at 169 ms.
  Of what follows the map being ready, ours is ≈9 ms (configure, the frame, its GPU); the window server takes the
  rest, 24–31 ms from the present, with or without the window's opening animation.
- Adapter selection does not wait for the window: on macOS every Metal adapter presents, and the surface is checked
  when it exists.

### The frame

1. The sky (the map's `env_sky` cube) and the 3D skybox, from its own camera at its own scale, with its own lightmaps
   and fog.
2. The map in CS2's material families (S8). It is lit by its irradiance and directional lightmaps and its baked sun
   shadows, or by its light probes where the game does not lightmap (S9, S10), and fogged.
   The throws' arcs (tubes and marks), the grenades in flight and the flashes' and HEs' bursts draw in the same pass; the
   strongest effect's point light (fire, flash, HE) adds to the map's diffuse light.
3. The effects pass: additive, depth-feathered surfaces, and the fire's sprites and ground (`throw.wgsl`).
4. The composite: the scene into the HDR target, with the smoke raymarched by a port of CS2's smoke shader against the
   depth (`smoke.wgsl`), one pass per smoke, the farthest first; a still picture averages 16 jittered frames (S17).
5. Bloom (four levels), then present: exposure (a GPU reduction, never read back), the map's tone curve and its
   colour lookup table, into the 3D view's rectangle between the shell's panels.
6. egui.

- **The static map** is recorded once as render bundles.
- **Uploads** come straight from the pack's mapping: on Metal the GPU copies the textures out of the pack's own pages
  (a buffer over the mapping, which macOS charges to the file's cache, not the process); elsewhere a chunk (≈32 MiB)
  at a time through staging buffers, at most 256 MB of them in flight (S20).
- **Every map stays resident** within a budget (8 GiB, or half of what the GPU recommends a process keep, whichever is
  less; `--budget-mb`): 0.37 (Mirage) to 1.43 GB (Inferno) of textures and geometry each, the least recently shown let
  go first (S19).

## 6. The pack (format 2)

`<packs>/<map>.pack`, built by `nades-pack <map>` (`crates/packer`) in 2.5–6.5 s from:
- the geometry export the packer makes (S29): Source 2 Viewer's glTF of the map and of its 3D skybox's prefab
  (geometry, and materials with their `vmat` parameters), `export_map.py`'s layer step, glTF-Transform's compression;
- the game's own files (the `[assets] game` folder, docs/standalone-api.md §8):
  - textures as BC blocks, from `pak01` (or files unpacked from it);
  - each map's lightmaps and light probes, from its VPK;
- through Source 2 Viewer's CLI (`[tools] s2v`), run by the packer itself (S26):
  - the map's physics, reduced as awpy reduces it, into the Collider section (byte for byte awpy's mesh);
  - the entity lumps (the map's, its 3D skybox prefab's): the sun's shadow channel, the light probe volumes, the sky,
    the fog, the 3D skybox's placement;
  - the sky materials and the post-processing resource with its lookup table;
  - which meshes the game lightmaps (S9).

The smoke's noise and the fire's flipbooks, the same on every map, are one shared pack, `<packs>/shared/effects.pack`
(`nades-pack --effects`), from `pak01`.

It is keyed by the game build and the export's hash, rebuilt when either changes, and never committed or shared.

- **Layout** (`crates/pack`, implemented and tested):
  - a 16-byte header: magic, format version (2), the table's length;
  - a postcard table of contents;
  - sections on 16 KiB boundaries;
  - written to a temporary name and renamed into place.
  A guard test fails when the Scene's encoding changes without a version bump.
- **Sections:**
  - **Scene:**
    - draws: positions, normal and tangent frames, UV sets, lightmap coordinates, vertex colour and paint, instances;
    - materials by family, with their CS2 parameters and texture slots;
    - the lightmaps, light probes, sun, sky and 3D skybox;
    - post-processing: tone curve, exposure, bloom, the colour lookup table, gradient and cubemap fog;
    - the build's counts.
  - **Geometry:** vertex and index buffers in the shaders' layouts (quantized as exported).
  - **Textures:** mips as GPU blocks (BC1/BC4/BC5/BC6H/BC7; RGBA8 for the few images with no game file), rows padded
    to 256 bytes for buffer-to-texture copies, grouped in chunks.
  - **Collider:** the map's occluding triangles, from its physics (AWMH layout); the BVH over them builds in 6–39 ms
    when a map opens (S15, S26).
- **Texture sources:**
  - Base colours map to `.vtex_c` files in `pak01` by name, or else through each material's `TextureParams` (Cache:
    296 of 518 images by name, 69% of the pixels; Mirage: 353 of 407). The rest are the export's WebP, decoded at pack
    time.
  - The other material textures come from the material's `vmat` parameters. Normal maps and the upper layers' colours
    are capped at 1024 texels, data textures at 512 (S11).
  - Normal maps stay hemi-octahedral, as the game stores them, and are decoded in the shader.
  - The lightmaps and light probes are `.vtex_c` files inside each map's VPK
    (`game/csgo/maps/<map>.vpk` in the game folder; the 3D skybox's are in its prefab's). On de_cache, stored:
    irradiance 78.5 MB, directional irradiance 7.6 MB, direct light shadows 2.7 MB, the light probe atlas 82 MB.
- **Size:** 347 MB (Mirage) to 1,385 MB (Inferno). de_cache's is 1,078 MB: 299 MB of colour, 185 MB of normals,
  143 MB of lightmaps and 175 MB of light probes (S11). Format 1's was 640 MB.

## 7. Porting the look

From show-me-the-nades' `web/app/src/scene/`, GLSL to WGSL by hand (naga cannot take three.js chunks):

| Web module (lines) | Native | Done |
|---|---|---|
| `render/csgoMaterials.ts` (391) | materials uber-shader: layer blending, tint masks, colour correction | M2 (`map.wgsl`, S8) |
| `render/fog.ts` (194) | gradient and cubemap fog | M2 |
| `render/composite.ts` (543), `render/volumes.ts` (253), `smoke.ts` (284), `smokeNoise.ts` (145) | smoke: the cloud rebuilt from its voxel log, its volume, CS2's smoke shader, its noise; the voxels' light computed in Rust against the collider | M3 (`crates/scene` `smoke.rs`, `noise.rs`; `smoke.wgsl`) |
| `fire.ts` (570), `render/fire.ts` (409) | fire as CS2's particle systems draw it | M3 (`fire.rs`; `throw.wgsl`) |
| `render/bloom.ts` (208), the present pass in `composite.ts` | bloom, exposure, tone curve, lookup table | M2 (`post.wgsl`) |
| `render/csgoEffects.ts` (297), `render/effects.ts` (133) | the effects pass, the flight, flash and HE bursts | the effects pass in M2; the rest in M3 (`stage.rs`; `throw.rs`) |
| `camera.ts` (165), `stage.ts` (198) | CS2 cameras (vertical field of view 73.74°, aim 28°), the result camera | M3 (`camera.rs`, `stage.rs`, `collider.rs`): the same cameras as the web view's, to the bit (S18) |
| `flight.ts` (379) | the orbit view's scripted chase and turn | not ported: the orbit is free from the thrower's view on (S18) |
| `render/engine.ts` (978) | the frame graph above | M2 |

The web-only half (posters, resident maps, prefetch, sRGB upload workarounds, worker image decoding, the loading
panel) has no native counterpart: it exists to work around the browser.

**Then, beyond the web view's look,** what the game files hold and the web view leaves out, all in M2:
- the directional lightmap, so normal maps answer the baked light;
- the baked sun shadows, which replace the real-time shadow map;
- the light probe volumes, which light the meshes the game does not lightmap (S9, S10) and will light moving things
  in a replay;
- facing masks on blend layers, which keep moss and dirt to the faces that point up (S10).

## 8. Scenes

- **The scene file** (`crates/control/src/scene.rs`): `{"format": "nades-sandbox-scene", "version": 1, "map": …,
  "throws": [{"grenade", "feet", "pitch", "yaw", "crouch", "movement", "click", "start", "label"}]}`. Feet in Hammer
  units, angles as CS2's `setang` takes them, `start` in seconds on the scene's clock. A reader refuses a newer version;
  a change of meaning is a new version.
- **A throw's simulation** depends on everything but its start and label, and is kept per map for the session. The
  scene's clock places each: its grenade leaves the hand at `start + release.t`, goes off at `start + detonation.t`, and
  its effect runs on for the stage's duration.
- **Where scenes live:** each map's scene in the remembered state (`state.json`, written on exit), and files wherever the
  person saves them (`~/nades-scenes/<map>.json` by default).

## 9. Measured, and the targets

| Step | Measured | Target |
|---|---|---|
| Process start → `main` | 4–7 ms (a newly built binary's first launch: 0.3–14 s once, while macOS checks the executable) | |
| `main` → window (AppKit event loop, launch, window) | 75–78 ms | overlap the load with it |
| Window's first frame on screen, empty shell (M0) | 120–124 ms from process start (warm, n=6) | ≤ 150 ms |
| Map's first frame, GPU-complete, offscreen, BC pack (spike) | 99 ms median from `main`, 84 best (load 23–29); 91–111 at load 47 | |
| Map's first frame on screen, windowed, lit by its lightmap (M1) | 111 ms median from process start (presented at 88 ms, the window shown at 111) | ≤ 200 ms; stretch ≤ 130: met |
| Map's first frame on screen, windowed, in CS2's look (M2) | 172 ms median from process start, 9 runs, 157–216 ms, load ≈10 (window 110, map ready 117, presented 146, shown 172) | ≤ 200 ms: met |
| **The same with M3's shell and threads** | **172.4 ms** median, 9 runs, 158–189 ms, load 8–11 (window 109.8, map ready 115.2, presented 139.2, shown 172.4): `.runs/startup-20261026-151018/` | ≤ 200 ms: met |
| **Lineup switch (M3)** | **0.5 ms** of the window's time (median; max 1.1), shown in the next frame, a smoke's light included: its stage is built ahead (S16) | ≤ one frame, with a smoke's light: met |
| Frame with a lineup's effects (M3) | 4.9 ms (Lineup view) and 5.6–7.1 ms (Result, the smoke drawn) median at 1444×1420, each finished on the GPU; p95 7.1–8.9 | |
| A throw's stage (M3) | a smoke's 2.8–3.0 ms, 2.4–2.6 of them its light on 18 cores; the result shot 0.18 ms (median of 84) | off the window's thread |
| Collider (M3) | 5.6 ms (mirage) to 39 ms (inferno), the mesh's read included, after the first frame | |
| Map switch, loaded (M3) | 1.55–1.72 s from the pick to the new map shown at its first lineup, its stage built (`--bench-maps de_mirage,de_inferno,de_cache`) | |
| **Map switch, resident (M4)** | **the next frame**, 42 of 42 picks; that frame 5.3 ms (median, finished on the GPU, vsync off; GPU 3.7; worst 8.9), 5.7 ms on a map's first visit in a session (GPU 4.1; worst 8.0); the window's own share 1.6 ms; load ≈12.3: `.runs/resident-20261026-163237/` | ≤ one frame: met |
| **Every map in one session (M4)** | all seven open in the background 49–97 ms each, lineups and first stages warm 1.6–1.75 s after start; 6,221.8 MB on the GPU of an 8,589.9 MB budget; footprint 6.32–6.56 GB at the end, 6.32–6.62 after each pick, peak 6.46–6.74 GB; the shown map's frames meanwhile median 4.7–4.9 ms, p95 15–17, worst 40–56 | bounded and stated: met |
| Under a 3 GB budget (M4) | two maps kept beside de_cache; the other five load when picked (≈1.5 s each) and the least recently shown go: held ≤ 3.05 GB throughout, peak footprint 4.3 GB | |
| **First frame on screen, de_cache (M4)** | **168.8 ms** median of 9, 160–180, load ≈10 (window 114.2, map ready 130.5, presented 139.4, shown 168.8; the first frame's GPU wait 4.5): `.runs/startup-20261026-163255/`; 167.4 and 169.5 in two earlier runs | ≤ 200 ms: met |
| **First frame on screen, de_inferno (M4)** | **185.1 ms** median of 9, 163–201, load ≈8.4 (window 112.9, map ready 144.1, presented 153.4, shown 185.1; GPU wait 5.9); ≈239 ms before: `.runs/startup-20261026-163315/`; 177.7 and 194.1 in two earlier runs | reported |
| Opening the other maps against the first frame (M4) | they start once the window shows: on screen 169.5 ms with them against 167.4 without (medians of 7 alternated pairs, per pair −2.1 ms) | no slower: met |
| Frame, whole map with CS2 materials and a smoke | 4.18 ms at 2880×1800 offscreen without a throw (M3, the composite pass included; M2 4.08), each frame finished on the GPU (median of 3 runs of 600 frames, 95th percentile 5.0); in the window 3.3–3.6 ms | ≤ 8.3 ms (120 Hz): met; with a smoke in view, at 1444×1420, p95 7.1–8.9 ms |
| Pipelines at start-up | warm: none waited for (macOS's shader cache); ≈360–440 ms on the first launch after a shader change (M2's pipelines, in parallel), and 497–630 ms on the first launch of a binary at a new path or a new app bundle: the cache is the program's | |
| Peak memory, de_cache | 2.07–2.19 GB footprint (M4; M3 2.07, M2 2.0; M1's 640 MB pack 0.72–0.75 GB): the textures as Metal allocates them 983 MB, the driver's first submission 119 MB and first render pass ≈200 MB, what drawing the map's triangles holds 170–360 MB, the window ≈150 MB; the upload's staging none (S20) | |
| Pack build | 1.5–3.9 s a map (M2) | |
| **Replay frame (M5)** | **3.9–4.5 ms** median at 1444×1420 offscreen, finished on the GPU, p95 4.2–5.2, worst 5.3–7.6 (600 frames of de_cache round 16, up to 8 throws live, 10 players; overview, behind a player, through their eyes); the moment's set-up on the CPU 0.50–0.58 ms: S30 | ≤ 8.3 ms: met |
| Data: list | a map's first: 0.25–0.37 s on 247 demos (reading 180–275 ms, the rules 66–110 ms); each filter change after it 6–12 ms in memory (S24). M3's DuckDB read: 93–114 ms per change on ≈96 demos | off the first frame's path: met |

Timing runs: `nades-sandbox --exit-after-first-frame` prints every stage as JSON. `python3 bench/startup.py` runs the
first frame, both steady-frame benches and peak memory with the load average beside each, into
`.runs/startup-<time>/` (M2's: `.runs/startup-20261026-133719/`). Alternate builds within a run when comparing two.

## 10. Milestones

| | Scope | Done when |
|---|---|---|
| **M0** ✓ | workspace; window, egui shell, GPU; start-up clock; pack format, VTEX reader, read-only data (tested) | builds clean (clippy); 7 tests pass; first present 120 ms from process start |
| **M1** ✓ | a VPK reader for the map's lightmaps; `nades-sandbox pack de_cache` (geometry from the export, BC textures and the BC6H irradiance from the game's files); the textured map in the window, lit by its lightmap, loaded on its own thread while AppKit makes the window; a fly camera | met: the map on screen 111 ms from process start (median of 7 alternated runs, the later of `frame:presented` and `window:visible`); 2.4 ms steady frame; the map's frame is ready at 88 ms, and the window's appearance (AppKit) is the limit |
| **M2** ✓ | CS2 materials, directional lightmap and baked shadows, light probes, sky and skybox, fog, bloom, tone curve, lookup table, GPU exposure; offscreen renders from the in-game lineup shots' cameras | met: every pool map's render differs from the game's shot no more than the web view's, by blurred \|Δ\| whole and top half (S12; the skill's references read in place); de_cache on screen 172 ms from process start (median of 9); a 4.1 ms frame at 2880×1800 |
| **M3** ✓ | the database: lineup list and filters; the four views; flight path; smoke from its voxel log; fire, flash, HE; time scrubber; result camera through the collider; practice command; a map selector; each map opening at one of its lineups | met: every view of 84 throws on the seven maps opens from the web view's camera, 0.0000 units apart (`bench/cameras.py`); the effects beside the web view's for 12 throws (`bench/effects.py`); a lineup switch shown in the next frame for 0.5 ms of the window's time; de_cache on screen 172 ms from process start (S16–S18) |
| **M4** ✓ | every pool map packed and resident; map switch; macOS app bundle; a Windows build | met: a resident map shows in the next frame (42 of 42 picks; that frame 5.3 ms, 5.7 on a first visit, worst 8.9); all seven maps in one session, 6.2 GB of an 8 GiB budget, least recently shown let go beyond it; de_cache on screen 168.8 ms from process start, de_inferno 185.1 (medians of 9); `just bundle` runs from Finder; the workspace checks and lints clean for Windows (S19–S23) |
| **M7** (built) | MCP-first (S34): the window the 3D view alone; console-string throws; reports of what each throw did in every tool that places them; `throws_simulate`, `camera_look_at`; video shots (follow, orbit, thrower, hold) to an MP4 at a chosen path; the flash's burst | met: acting as the agent over stdio, the user's smoke placed from its console string on de_mirage, its detonation reported against the game's, and its MP4 (follow, then a 360° pan round the smoke) rendered and checked frame by frame; `verify-mcp` 66 checks, all 21 tools positive and negative; tests debug and release; clippy clean |
| **M6** (built) | the simulator (S31): the recorded-data features removed; the throw tool with a live preview arc; the scene (list, select, re-aim, edit, duplicate, delete, start times); play, pause and scrub with every grenade flying and going off (smoke through `smoke_grid` and `fill_cloud`, fire through `fire_flames` and `build_fire`, flash, HE, decoy); the views (free, thrower, follow, orbit); the practice command; scene files; MCP tools for scenes, throws, time, views, images and videos; `--scene … --render` offscreen | met: offscreen renders of placed throws, each grenade kind, on three maps; the window driven over app control (scene in, each view, captures with the shell); the first frame unchanged (139.3 ms against 139.0, medians of 7 alternated); tests in debug and release and clippy `-D warnings` clean. How right a flight or an effect is, is the physics' (S32) and the effects' (S33) |
| **M5** (built, removed in M6) | demo replay: pick a demo and a round; play, pause, scrub and speed (¼–4×); every player's position, view, life and side, every grenade in flight and its smoke, fire, flash or HE at its time, several smokes at once; a free camera, a player's eyes, a camera behind them; a lineup view from any throw of the round; players 16 a second (cs2-throws 2.1 `player_tracks`), else from the samples; `--replay` and `--bench-replay` offscreen | met offscreen: 11 moments on four maps rendered at chosen times, every living player (72 of 72) and the dead in view (4 of 4) drawn where the facts put them in their side's colour, each live effect seen at its point unless a wall hides it (`bench/replay_check.py`); a replay frame 3.9–4.5 ms median at 1444×1420 (p95 ≤ 5.2) with up to 8 throws live; the map's frame and every lineup view unchanged (3.45 ms either way; renders identical but GPU noise); no change on the first frame's path. Not yet: driving it in the window (the screen was locked, S30) |

## 11. Rules

- **Valve's assets stay on this machine.** Packs, exports and the game files live in the folders the settings name
  (docs/standalone-api.md §8: the packs folder, CS2's folder, the exports in the platform's cache). They are never
  committed, uploaded or shared; rendered images at most.
- **Nothing reads recorded matches** (S31): throws come from the throw tool, scene files and the MCP tools; tests write
  their own files.
- **Nothing waits for a human:** a missing pack is built.

## 12. Risks and unknowns

| Risk or unknown | What settles it |
|---|---|
| A window macOS paces to its display's refresh: the windowed bench then reads 8.3 ms (120 Hz) or 16.7 ms (60 Hz), not the frame | GPU timestamps per pass (wgpu-profiler) beside the wall clock: not done in M3; the switch bench finishes each frame on the GPU instead |
| Peak memory: 2.0 GB for de_cache's 1.08 GB pack, while M4 keeps every map resident (≈5.9 GB of packs) | settled (S20): it is the textures as Metal allocates them, the driver's first submission and render pass, the window, and what drawing holds; not staging. Seven maps: footprint 6.3–6.6 GB within an 8 GiB budget |
| The first windowed frame takes ≈55 ms from the map being ready to the window shown | settled (S21): ours is ≈9 ms now (the GPU wait 21 → 4.5 ms); the window server takes 24–31 ms after the present, and AppKit's window is the rest |
| A map's first frames cost the GPU 30–100 ms more than the rest (the driver's work on first use) | two warm-up frames on the map's loader thread, and one from its lineup's view once that stage is built: a first visit's frame 5.7 ms (median) against a later visit's 5.3 |
| Opening maps in the background competes for the GPU with the shown one | its frames meanwhile: median 4.7–4.9 ms, p95 15–17, worst 40–56 ms (a few hitches in the first 2 s); smaller submissions for the copies and warm-ups if it shows |
| Windows: checked, never run | `just check-windows` type-checks and lints every crate for x86_64-pc-windows-msvc with stand-ins for the C++ libraries; a run on a Windows machine (DX12, the fallbacks of S23) |
| Whole-frame brightness: dust2, cache, mirage and nuke read 0.8–2.8 levels brighter than the web view against the game, though closer by blurred \|Δ\| | `bench/realism.py` on each look change; its history is `.runs/realism-history.jsonl` |
| Packing needs three outside tools: Source 2 Viewer's CLI, Bun (glTF-Transform) and DepotDownloader | each is a setting with a clear error when missing; `--export` takes a geometry export made elsewhere; reading the meshes from the VPKs directly (§13) would remove the CLI and Bun |
| With `pak01` mounted, the CLI passes over a file whose archive is missing in silence | `nades-pack setup` finds a map's archives with `pak01` unmounted, where the CLI names what it cannot load (S29) |
| Smoke at Retina resolution (4× the pixels) | at 1444×1420 a smoke in view costs 1–2 ms a frame (median 5.6–7.1 against 4.9), p95 under 8.9; a smoke filling a 2880×1800 frame is not measured yet: GPU timestamps of the composite at 1× and 2×, and half resolution with a depth-aware upsample if it needs it |
| The effects' point lights on CS2's baked light: three.js's model (Lambert over π) added to a CS2-style diffuse | `bench/effects.py`: Inferno's flash reaches further up the walls than the web view's (0.21 of the frame against 0.13) |
| A producer leaves facts out (show-me-the-nades' export: blinded players' positions, samples' duck amounts) | the sandbox derives what the facts allow (a blinded player stands at their sample nearest the flash, S24) and `check` names the rest |
| Game updates change the files | packs keyed by game build; `setup` pins the depot's manifest to the one the game folder holds, so a fetch never mixes builds; rebuild on change |
| Two front-ends to keep | the web app keeps analytics; the sandbox replaces `/scene` only when M3 matches it |

## 13. Choices still open

- **Windows:** the workspace builds for it as far as a Mac can check (S23); running it, and packaging it (an installer,
  and the packs and exports made there), wait for a Windows machine.
- **The pack's geometry source:** the packer's own export through Source 2 Viewer's CLI and glTF-Transform (S29) stays
  the input until a Rust reader for the game's meshes (VBIB in `vmdl`/`vwrld`) earns its cost; it would also have to
  reproduce glTF-Transform's welding, instancing and quantization to keep the packs the same.
- ~~**Replay data (M5)**~~ settled (S30): tracks 16 a second in cs2-throws 2.1, measured against every tick; the
  samples, two a second, when a file has none.




The local MCP has its own [specification and per-tool acceptance checks](docs/mcp-spec.md). It uses shared application
commands and camera/state types, and supports map/camera operation, frame capture, diagnostics and headless map
rendering and recorded-throw video. Its verification entry point is `just verify-mcp`; live M3 throw controls are a
separate extension.

## Agent control

The local MCP has its own [specification and per-tool acceptance checks](docs/mcp-spec.md). It uses the shared
application commands, camera, state and scene types, and supports map and camera operation, frame capture, diagnostics,
the window's scene (read, replace, add, update and remove throws, the clock, the views), and headless images and videos
of scenes. Its verification entry point is `just verify-mcp`.
