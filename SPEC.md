# nades-sandbox: specification

A native app that shows a CS2 map and the grenades thrown on it, from the throws' own data, instantly. It reads
`cs2-throws` files, the facts CS2 demos record, which any parser can write (docs/standalone-api.md); `show-me-the-nades`
(a separate project, which keeps the analytics, guides and replays in its web app) is one producer. Later it plays back
whole rounds from demos.

Status: **M4 done** (2026-10-26).
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
  workspace builds with no sibling checkout. The demo reader (step 2) and maps from the game's files (step 3) are next.
- M3 made it the lineup sandbox: a map's lineups from the nades database, cut by side, grenade, team, player and phase;
  each seen from the thrower's eye, zoomed on the aim point, from an opponent's eye or orbiting it, with the flight, the
  smoke, fire, flash or HE as the round had it, a time scrubber and the practice command; every view of 84 throws opens
  from exactly the web view's camera, and a lineup switch costs the window 0.5 ms (S16–S18).
- M2 gave the map CS2's look: materials, baked light and sun shadows, light probes, sky, 3D skybox, fog, bloom, tone
  curve and colour lookup table, each pool map differing from the game no more than the web view does (S12). M1 put
  the textured de_cache on screen in 111 ms; it was built twice, as our own renderer and with Bevy as the shell
  ([docs/m1-bakeoff.md](docs/m1-bakeoff.md)), and the user chose our own renderer (S7).
- M5 (demo replay) is next.

## 1. What it is like to use

You open it, and the map you last looked at is on screen in about a tenth of a second.

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│ de_cache ▾  T · CT   smoke flash molotov HE   team ▾   player ▾   phase ▾   ⌕         │
├────────────────────┬───────────────────────────────────────────────┬─────────────────┤
│ LINEUPS  118       │                                               │ Smoke · T       │
│ TSpawn → Mid    23 │                                               │ ropz · Vitality │
│ Rafters → B     15 │         the map, the thrower's eye,           │ run-jumpthrow   │
│ Connector → G   13 │         the grenade's flight, its smoke       │ left click      │
│ OutsideA → Cat  12 │                                               │ setpos … setang │
│ …                  │                                               │ [copy]          │
│                    │                                               │ Lineup Aim      │
│                    │                                               │ Result Orbit    │
├────────────────────┴───────────────────────────────────────────────┴─────────────────┤
│ ◀ ▶  ────●───────────────────  4.0 s after it landed                  first frame 118 ms │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

- **The lineup list** is ordered as `lineup_report` orders it, and cut by side, grenade, team, player and round phase,
  like the web app's Lineups page.
- **Picking a lineup** moves the camera, with no loading. You see it from:
  - the thrower's eye at release, with the crosshair (Lineup);
  - zoomed on the aim point (Aim);
  - an opponent's eye, with the smoke, fire, flash or HE as the round had it (Result);
  - anywhere you drag (Orbit).
- **The time scrubber** plays the flight, the bloom and the fade; the arrow keys step lineups, 1–4 pick the views and
  space plays.
- **Copy** puts the practice command (`setpos`, `setang`) on the clipboard.
- **Switching maps** shows the other map at its lineup in the next frame: every pool map stays on the GPU (0.37–1.43 GB
  each, its textures in the game's own compressed blocks and its geometry), its lineups read and its lineup's stage
  built ahead. A map that did not
  fit the budget loads when picked, in about 1.5 s.
- **No browser:** no page loads, no memory ceiling of a tab, no shader compiles on each visit.
- **Later:** a round plays back with its players and every grenade.

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

**Now (M1–M4):** the pool maps with their textured exports, the lineups and throws in cs2-throws files, the four
views, the throw's flight, smoke, fire, flash and HE, the time scrubber, the practice command, every map resident.

**Later (M5):** demo replay, round by round, with players and every grenade.

**Not here:**
- the analytics, map guides and 2D replay, which stay in the web app;
- editing data;
- reading demos themselves, until the demo reader (docs/standalone-api.md §6, step 2);
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
| Data | cs2-throws files: SQLite, bundled, read-only (`crates/source-file`); the config file in TOML, its folder from `directories` | rusqlite 0.41, toml 1, directories 6 |
| Assets | packs memory-mapped (memmap2), tables in postcard, LZ4 (lz4_flex), our own VTEX reader; meshopt for the export's geometry; Source 2 Viewer's CLI when packing (S9) | 0.9, 1.1, 0.14, 0.7 |
| Maths, threads | glam, rayon | 0.34, 1.12 |
| Profiling | start-up marks (JSON), wgpu-profiler (GPU timestamps; built on the same wgpu 30), tracing with Tracy | 0.28, 0.1 |
| Collision | awpy's collision mesh and a BVH over it, as the web view does since D26 | awpy 3.1 |

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
crates/model     the sandbox's vocabulary and rules, no I/O: CS2's facts, the ThrowSource port, the derivations
                 (technique, grouping, practice, round time, callouts, viewpoints), the voxel log's decoder, Library (S24)
crates/source-file  the cs2-throws file adapter: version 2 SQLite files, several merged, and `check`
crates/config    where the settings come from (§8): flag, environment, config file, default
crates/scene     a throw becomes a stage: CS2's cameras, the result shot through awpy's collider, the smoke's cloud,
                 volume and light, the fire, the timeline (ported number for number from the web view)
crates/control   app control's protocol, and crates/mcp its MCP server (a separate workstream on the same app)
crates/replay    (M5) rounds from demos
tools/cs2_throws.py  the format's schema and reference writer (Python's standard library only)
```

### Start-up, on two threads, then three more

```
main      main ─ EventLoop (AppKit, ~30 ms) ─ launch (~14) ─ window + CAMetalLayer (~32) ─ surface ─┐
loader    main ─ mmap pack ─ device (~3–14) ─ buffers and textures, pipelined (~40–60) ─ pipelines ─┴─ first frame ─ on screen
                                                                                                                    │
data      the list (~100 ms), then every listed lineup's throw, the selected first ─────────────────────────────────┤
collider  awpy's mesh and its BVH (6–39 ms) ─────────────────────────────────────────────────────────────────────────┤
effects   the game's smoke noise and fire flipbooks ─────────────────────────────────────────────────────────────────┘
stages    each throw's stage on every core (rayon), once the collider is there
maps      once the window shows: the other packed maps, one at a time, each drawn offscreen; their lists, stages (S19)
```

- The first frame needs no data: it is drawn from the camera the last run left on that map (S16). Reading the list
  before it made the window appear later.

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
   The shown throw's flight (its tube, marks and the grenade), a flash's or HE's burst and the lines to who it blinded
   draw in the same pass; an effect's point light (fire, flash, HE) adds to the map's diffuse light.
3. The effects pass: additive, depth-feathered surfaces, and the fire's sprites and ground (`throw.wgsl`).
4. The composite: the scene into the HDR target, with the smoke raymarched by a port of CS2's smoke shader against the
   depth (`smoke.wgsl`); a still picture averages 16 jittered frames (S17).
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

`~/.cache/nades/packs/<map>.pack`, built by `nades-pack <map>` (`crates/packer`) in 1.5–4 s from:
- the Source 2 Viewer export in `~/.cache/nades/maps`:
  - geometry, and materials with their `vmat` parameters;
  - the 3D skybox (`<map>.skybox.glb`);
  - post-processing and sky (`<map>.post.json`, `.lut.bin`, `.sky.json`, `.skybox.json`);
- the game's own files in `~/.cache/nades/cs2`:
  - textures as BC blocks, loose or in `pak01`;
  - each map's lightmaps and light probes, from its VPK;
- Source 2 Viewer's decompiled entity lump (`~/.cache/nades/cs2/export/<map>`): the sun's shadow channel, the light
  probe volumes, the sky's material;
- Source 2 Viewer's CLI (`~/.cache/nades/tools/s2v`): which meshes the game lightmaps (S9).

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
  - **Collider** (reserved): unused, since the BVH over awpy's mesh builds in 6–39 ms when a map opens (S15).
- **Texture sources:**
  - Base colours map to `.vtex_c` files by name, or else through each material's `TextureParams` (Cache: 296 of 518
    images by name, 69% of the pixels; Mirage: 353 of 407). The rest are the export's WebP, decoded at pack time.
  - The other material textures come from the material's `vmat` parameters. Normal maps and the upper layers' colours
    are capped at 1024 texels, data textures at 512 (S11).
  - Normal maps stay hemi-octahedral, as the game stores them, and are decoded in the shader.
  - The lightmaps and light probes are `.vtex_c` files inside each map's VPK
    (`~/.cache/nades/cs2/game/csgo/maps/<map>.vpk`; the 3D skybox's are in its prefab's). On de_cache, stored:
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

## 8. Data

- **Facts, derivations, tags** (docs/standalone-api.md §2): a source hands over facts only, as `crates/model` names
  them; everything computed from them is the model's, one implementation for every source; a producer's own labels are
  tags, shown and filterable, never interpreted.
- **The source:** `crates/source-file` reads cs2-throws version 2 files read-only. Several files merge into one source,
  each demo taken from the first file that has it; another major version is refused, naming both; unknown tables and
  columns are ignored. A file written again (its size or modification time) is opened again, and the source's revision
  changes: the app looks every 5 s and reads the list and the stages again. A file being written is `Busy`, retried
  every half second.
- **The library:** one `nades_model::Library` per map and revision, built on the data thread from the map's facts and
  its place-labelled samples. A list is that library cut by side, grenade, team, player, round time, plant and tag, in
  memory; a stage reads the throw's tracks and the samples of its viewpoint window (and, for a flash whose blinded
  players have no position, the samples around its detonation).
- **Where the data is:** `--data`, `NADES_SANDBOX_DATA`, the config file's `[data] paths`, then the demo reader's cache
  (`crates/config`, which the MCP server can share); `nades-sandbox check <file>` reports a file's problems and the features its facts allow.

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
| **M5** | demo replay: rounds with players and grenades over time | (specified then) |

## 11. Rules

- **Valve's assets stay on this machine.** Packs, exports and the game files live under `~/.cache/nades`. They are
  never committed, uploaded or shared; rendered images at most.
- **The user's data is read, never written,** and never experimented on: tests write their own files
  (`tools/cs2_throws.py fixture`), checks use a copy.
- **Nothing waits for a human:** a missing pack is built, and a file being written is retried.

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
| Packing needs Source 2 Viewer's CLI and its decompiled entity lumps | without the CLI the packer falls back to the web view's lightmap rule and says so; without the lumps it packs no light probes and no sky |
| Smoke at Retina resolution (4× the pixels) | at 1444×1420 a smoke in view costs 1–2 ms a frame (median 5.6–7.1 against 4.9), p95 under 8.9; a smoke filling a 2880×1800 frame is not measured yet: GPU timestamps of the composite at 1× and 2×, and half resolution with a depth-aware upsample if it needs it |
| The effects' point lights on CS2's baked light: three.js's model (Lambert over π) added to a CS2-style diffuse | `bench/effects.py`: Inferno's flash reaches further up the walls than the web view's (0.21 of the frame against 0.13) |
| A producer leaves facts out (show-me-the-nades' export: blinded players' positions, samples' duck amounts) | the sandbox derives what the facts allow (a blinded player stands at their sample nearest the flash, S24) and `check` names the rest |
| Game updates change the files | packs keyed by game build; rebuild on change |
| Two front-ends to keep | the web app keeps analytics; the sandbox replaces `/scene` only when M3 matches it |

## 13. Choices still open

- **Windows:** the workspace builds for it as far as a Mac can check (S23); running it, and packaging it (an installer,
  and the packs and exports made there), wait for a Windows machine.
- **The pack's geometry source:** the Source 2 Viewer export stays the input until a Rust reader for the game's meshes
  (VBIB in `vmdl`/`vwrld`) earns its cost.
- **Replay data (M5):** awpy's full-tick states or the database's half-second samples, interpolated.


## Agent control

The local MCP has its own [specification and per-tool acceptance checks](docs/mcp-spec.md). It uses shared application
commands and camera/state types, and supports map/camera operation, frame capture, diagnostics and headless map
rendering and recorded-throw video. Its verification entry point is `just verify-mcp`; live M3 throw controls are a
separate extension.
