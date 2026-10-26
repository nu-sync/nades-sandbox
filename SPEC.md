# nades-sandbox: specification

A native app that shows a CS2 map and the grenades pros throw on it, from the throw's own data, instantly. It is the
3D half of `show-me-the-nades` (a separate project), which keeps the analytics, guides and replays in its web app.
Later it plays back whole rounds from demos.

Status: **M2 done** (2026-10-26).
- de_cache in CS2's look is on screen 172 ms after the process starts (median of 9, load ≈10), and a frame of it takes
  4.1 ms at 2880×1800. The look covers its materials, baked light and sun shadows, light probes, sky, 3D skybox, fog,
  bloom, tone curve and colour lookup table.
- Rendered from the in-game lineup shots' cameras, every pool map differs from the game no more than the web view does
  (decision S12 in [docs/decisions.md](docs/decisions.md)).
- M1 put the textured, lightmapped de_cache on screen in 111 ms. It was built twice, as our own renderer and with Bevy
  as the shell, and compared under a rule fixed in advance ([docs/m1-bakeoff.md](docs/m1-bakeoff.md)). The user chose
  our own renderer (S7).
- M3 (the database, the four views and the throws) is next.

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
- **The time scrubber** plays the flight, the bloom and the fade; the arrow keys step lineups.
- **Copy** puts the practice command (`setpos`, `setang`) on the clipboard.
- **Switching maps** takes one frame, because every pool map stays on the GPU: in the game's own compressed textures
  a map holds 0.35–1.4 GB.
- **No browser:** no page loads, no memory ceiling of a tab, no shader compiles on each visit.
- **Later:** a round plays back with its players and every grenade.

## 2. Why native, measured

The web view is fast now: a map that is already loaded opens in about 0.1 s. The native app wins where the browser
cannot follow (show-me-the-nades' `docs/research/native-sandbox.md` and the measurements below):

| | Browser (today's web app) | Native (measured here) |
|---|---|---|
| First frame of a map, cold | Cache orbit: 1.5 s to the map's first frame shown, 2.2 s settled; bare three.js 1.0 s | on screen 172 ms from process start in CS2's look (M2), 111 ms lit by its lightmap alone (M1) |
| Texture memory | RGBA8 decoded from WebP: 0.5 GB (Mirage) to 1.7 GB (Inferno) of pixels per map | the game's own BC blocks, 4× smaller |
| A smoke's light (≈34,000 rays against the map) | 200–270 ms on the main thread | 0.2–0.8 µs a ray against awpy's mesh (swarm 3 lane 03), so ≈7–28 ms on one core, ≈1–2 ms on all cores (estimate) |
| Memory ceiling | a tab's renderer dies at ≈16 GiB (D26) | the machine's memory |
| Frame of the whole map | 15–18 ms at 1440×900 with CS2's shading and a smoke | 4.1 ms at 2880×1800 with CS2's shading and post-processing, no smoke yet (M2) |

## 3. Scope

**Now (M1–M4):** the pool maps with their textured exports, the lineups and throws in the nades database, the four
views, the throw's flight, smoke, fire, flash and HE, the time scrubber, the practice command, every map resident.

**Later (M5):** demo replay, round by round, with players and every grenade.

**Not here:**
- the analytics, map guides and 2D replay, which stay in the web app;
- editing data;
- non-pro demos;
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
| Database | DuckDB, bundled, the same build nades writes with; read-only, opened per read | duckdb ~1.20000.0 |
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
crates/sandbox   the app: winit window, egui shell, views, input, start-up clock          (bin: nades-sandbox)
crates/render    wgpu device, passes, pipelines, uploads from packs, frame graph
crates/pack      the per-map pack: format, writer, memory-mapped reader                   (format 2)
crates/source2   CS2's files: VTEX textures (BC blocks, mips, cubes, volumes), VPK archives; later KV3
crates/packer    `nades-pack <map>`: builds a map's pack from the export, the game's files and their entities
crates/data      the nades database, read-only and briefly                                (M0: read and tests)
crates/scene     (M3) a throw becomes a stage: cameras, effect timeline, smoke volume, fire
crates/replay    (M5) rounds from demos
```

### Start-up, on three threads

```
main     main ─ EventLoop (AppKit, ~30 ms) ─ launch (~14) ─ window + CAMetalLayer (~32) ─ surface ─┐
loader   main ─ mmap pack ─ device (~3–14) ─ buffers and textures, pipelined (~40–60) ─ pipelines ─┼─ first frame (~20) ─ on screen
data     main ─ DuckDB, read-only: the last lineup and the list (~30–50 ms) ─ closed ─────────────┘
```

- The window and the map arrive together. Measured on M2's de_cache (medians of 9 from process start, load ≈10):
  - the window exists at 110 ms;
  - the map is ready at 117 ms, its 1 GB of textures uploaded and one frame drawn offscreen;
  - the first frame is presented at 146 ms;
  - macOS shows the window at 172 ms.
  The last two steps, ≈55 ms, are now most of what is left: the surface's first frame and the window server.
- Adapter selection does not wait for the window: on macOS every Metal adapter presents, and the surface is checked
  when it exists.

### The frame

1. The sky (the map's `env_sky` cube) and the 3D skybox, from its own camera at its own scale, with its own lightmaps
   and fog.
2. The map in CS2's material families (S8). It is lit by its irradiance and directional lightmaps and its baked sun
   shadows, or by its light probes where the game does not lightmap (S9, S10), and fogged.
3. The effects pass: additive, depth-feathered surfaces.
4. (M3) The smoke, by a port of CS2's smoke shader against the depth buffer, at reduced resolution if the frame budget
   needs it.
5. (M3) The fire's particles, the flight path, flashes and HE.
6. Bloom (four levels), then present: exposure (a GPU reduction, never read back), the map's tone curve and its
   colour lookup table.
7. egui.

- **The static map** is recorded once as render bundles.
- **Uploads** come straight from the pack's mapping, through a ring of reused staging buffers: a fresh buffer is zero-filled at 8–10 GB/s.
- **Every map stays resident** while there is room: a BC map is ≈0.5 GB of GPU memory, least recently used out first.

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
  - **Collider** (reserved for M3): awpy's collision mesh and a BVH over it.
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
| `render/composite.ts` (543), `render/volumes.ts` (253), `smoke.ts` (284), `smokeNoise.ts` (145) | smoke: the cloud rebuilt from its voxel log, its volume, CS2's smoke shader, its noise; the voxels' light computed in Rust against the collider | M3 |
| `fire.ts` (570), `render/fire.ts` (409) | fire as CS2's particle systems draw it | M3 |
| `render/bloom.ts` (208), the present pass in `composite.ts` | bloom, exposure, tone curve, lookup table | M2 (`post.wgsl`) |
| `render/csgoEffects.ts` (297), `render/effects.ts` (133) | the effects pass, flash and HE bursts | the effects pass in M2; flash and HE in M3 |
| `camera.ts` (165), `stage.ts` (198) | CS2 cameras (vertical field of view 73.74°, aim 28°), the result camera | a lineup camera from `setpos` and `setang` in M2 (`View::from_setpos`); the rest in M3 |
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

- **Reads:** every read opens `nades.duckdb` read-only, runs its queries and closes it (`crates/data`).
  - `nades` locks the file while it runs, and an open connection makes it fail, so nothing stays open.
  - A file held by `nades` is `Busy`, and the app retries when it is free.
  - Read-only opens coexist with the web app's API.
  - Measured: `lineup_report` (open, query, close) in 32–51 ms.
- **Queries:** the views and macros nades keeps in the file:
  - `lineup_report` for the list;
  - `throw_scene` for a throw's flight path, voxel log and flames;
  - `scene_viewers` for opponents' positions.
  The DuckDB build must be the one nades writes with, or the file does not open.
- **Decoding:** the voxel log decoder already exists in Rust (show-me-the-nades `src/tracks.rs`, `voxel_frames`).
  M3 needs a library target there, so the sandbox depends on it by path instead of copying it.
- **The database path:** `NADES_DB`, or show-me-the-nades' default location. An ingest that changes it is noticed by
  its modification time, and the list reloads.

## 9. Measured, and the targets

| Step | Measured | Target |
|---|---|---|
| Process start → `main` | 4–7 ms (a newly built binary's first launch: 0.3–14 s once, while macOS checks the executable) | |
| `main` → window (AppKit event loop, launch, window) | 75–78 ms | overlap the load with it |
| Window's first frame on screen, empty shell (M0) | 120–124 ms from process start (warm, n=6) | ≤ 150 ms |
| Map's first frame, GPU-complete, offscreen, BC pack (spike) | 99 ms median from `main`, 84 best (load 23–29); 91–111 at load 47 | |
| Map's first frame on screen, windowed, lit by its lightmap (M1) | 111 ms median from process start (presented at 88 ms, the window shown at 111) | ≤ 200 ms; stretch ≤ 130: met |
| **Map's first frame on screen, windowed, in CS2's look (M2)** | **172 ms** median from process start, 9 runs, 157–216 ms, load ≈10 (window 110, map ready 117, presented 146, shown 172) | ≤ 200 ms: met |
| Lineup switch | | ≤ one frame, with a smoke's light |
| Map switch, resident | | ≤ one frame |
| Frame, whole map with CS2 materials and a smoke | M2, no smoke yet: 4.08 ms at 2880×1800 offscreen, each frame finished on the GPU (median of 3 runs of 600 frames, 95th percentile 4.83); in the window 3.5 ms when macOS does not pace it to the display (it did in 2 of 3 runs, which then read the 120 Hz refresh, 7.9–8.3 ms) | ≤ 8.3 ms (120 Hz): met without a smoke |
| Pipelines at start-up | warm: none waited for (macOS's shader cache); ≈360–440 ms on the first launch after a shader change (M2's pipelines, in parallel) | |
| Peak memory, de_cache | 2.0 GB footprint, GPU memory included (108 MB resident); M1's 640 MB pack: 0.72–0.75 GB | |
| Pack build | 1.5–3.9 s a map (M2) | |
| Database: list and throw | 32–51 ms, open to close | off the first frame's path |

Timing runs: `nades-sandbox --exit-after-first-frame` prints every stage as JSON. `python3 bench/startup.py` runs the
first frame, both steady-frame benches and peak memory with the load average beside each, into
`.runs/startup-<time>/` (M2's: `.runs/startup-20261026-133719/`). Alternate builds within a run when comparing two.

## 10. Milestones

| | Scope | Done when |
|---|---|---|
| **M0** ✓ | workspace; window, egui shell, GPU; start-up clock; pack format, VTEX reader, read-only data (tested) | builds clean (clippy); 7 tests pass; first present 120 ms from process start |
| **M1** ✓ | a VPK reader for the map's lightmaps; `nades-sandbox pack de_cache` (geometry from the export, BC textures and the BC6H irradiance from the game's files); the textured map in the window, lit by its lightmap, loaded on its own thread while AppKit makes the window; a fly camera | met: the map on screen 111 ms from process start (median of 7 alternated runs, the later of `frame:presented` and `window:visible`); 2.4 ms steady frame; the map's frame is ready at 88 ms, and the window's appearance (AppKit) is the limit |
| **M2** ✓ | CS2 materials, directional lightmap and baked shadows, light probes, sky and skybox, fog, bloom, tone curve, lookup table, GPU exposure; offscreen renders from the in-game lineup shots' cameras | met: every pool map's render differs from the game's shot no more than the web view's, by blurred \|Δ\| whole and top half (S12; the skill's references read in place); de_cache on screen 172 ms from process start (median of 9); a 4.1 ms frame at 2880×1800 |
| **M3** | the database: lineup list and filters; the four views; flight path; smoke from its voxel log; fire, flash, HE; time scrubber; result camera through the collider; practice command | every throw the web view opens opens here, from the same camera within a few units; lineup switch ≤ one frame |
| **M4** | every pool map packed and resident; map switch; macOS app bundle; a Windows build | map switch ≤ one frame; all seven maps in one session with bounded memory |
| **M5** | demo replay: rounds with players and grenades over time | (specified then) |

## 11. Rules

- **Valve's assets stay on this machine.** Packs, exports and the game files live under `~/.cache/nades`. They are
  never committed, uploaded or shared; rendered images at most.
- **The user's database is read, briefly, never written,** and never experimented on: tests use their own databases,
  checks use a copy.
- **Nothing waits for a human:** a missing pack is built, and a locked database is retried.

## 12. Risks and unknowns

| Risk or unknown | What settles it |
|---|---|
| A window macOS paces to its display's refresh: the windowed bench then reads 8.3 ms (120 Hz) or 16.7 ms (60 Hz), not the frame | M3: GPU timestamps per pass (wgpu-profiler) beside the wall clock |
| Peak memory: 2.0 GB for de_cache's 1.08 GB pack, while M4 keeps every map resident (≈5.9 GB of packs) | M4: measure peak and resident memory per map; free the upload's staging once it is done |
| The first windowed frame takes ≈55 ms from the map being ready to the window shown | M3: time the surface's first frame and the window server apart |
| Whole-frame brightness: dust2, cache, mirage and nuke read 0.8–2.8 levels brighter than the web view against the game, though closer by blurred \|Δ\| | `bench/realism.py` on each look change; its history is `.runs/realism-history.jsonl` |
| Packing needs Source 2 Viewer's CLI and its decompiled entity lumps | without the CLI the packer falls back to the web view's lightmap rule and says so; without the lumps it packs no light probes and no sky |
| Smoke at Retina resolution (4× the pixels) | M3: GPU timestamps of the smoke pass at 1× and 2×, and at half resolution with a depth-aware upsample |
| Game updates change the files | packs keyed by game build; rebuild on change |
| Two front-ends to keep | the web app keeps analytics; the sandbox replaces `/scene` only when M3 matches it |

## 13. Choices still open

- **Windows:** when to build for it. wgpu covers DX12 and Vulkan, and most CS2 players are on Windows. Planned for
  M4, sooner if friends want it.
- **The pack's geometry source:** the Source 2 Viewer export stays the input until a Rust reader for the game's meshes
  (VBIB in `vmdl`/`vwrld`) earns its cost.
- **Replay data (M5):** awpy's full-tick states or the database's half-second samples, interpolated.
