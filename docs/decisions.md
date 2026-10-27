# Decision log

Why the sandbox is built the way it is. Each entry has the evidence it rests on. Measurements come from show-me-the-nades'
swarm runs on 2026-10-25: an Apple M5 Max (18 cores, 128 GB), macOS 26.5, Chrome 155. The evidence folders
(`.verify-runs/…`) are in show-me-the-nades and stay on that machine.

## S1 · Native, beside the web app, not instead of it

- **Decision:** a native app for the 3D view and, later, demo replay. The analytics, map guides and 2D replay stay in
  the web app, and the database stays the one `nades` writes.
- **Evidence:** after three swarms the web view's cold open still costs ≈1.5 s to the map's first frame on Cache
  (bare three.js: 1.0–1.1 s; no web framework did better: three.js WebGL and WebGPU, Babylon.js WebGL2 and WebGPU;
  PlayCanvas cannot load the exports). The causes are the browser's own:
  - WebP decoded on the CPU (3.2 CPU-seconds for Inferno);
  - every byte through the GPU process's command buffer;
  - ANGLE translating shaders at first use;
  - nothing kept between page loads;
  - a renderer that dies at ≈16 GiB.
  Swarm 3's native spike drew the same map in 99 ms (S2).
- **Rejected:** moving everything native (two front-ends' worth of analytics UI for no gain); WebGPU in the browser
  (three.js WebGPU: 1.34 s first frame on the same load).

## S2 · wgpu, our own renderer, no engine

- **Decision:** wgpu 30.0.1 over Metal (macOS) and DX12 or Vulkan (Windows), with our own passes. No game engine.
- **Evidence** (swarm 3 lane 06, `prototypes/wgpu-spike`): the textured de_cache rendered offscreen at 1440×900 (1,276
  draws, 2.81 M triangles, 518 textures). GPU-complete first frame from `main`, n=10 alternated at load 23–29:

  | Loader | Median |
  |---|---:|
  | pack with the game's BC textures | 99 ms (84 best) |
  | glb with the game's BC | 131 ms |
  | RGBA pack | 173 ms |
  | glb with WebP decoded at start | 344 ms |

  Re-measured by the coordinator at load ≈47: the BC pack took 91–111 ms. The steady frame takes 3.5–4.8 ms.
- **Why no engine, measured** (`prototypes/engine-bakeoff`): Bevy 0.20 on Rust 1.99.1 runs on the same wgpu
  (30.0.1) and winit (0.30.13), so the engine is the only difference. It was configured at its leanest for this app:
  its 3D renderer, window and app, nothing else. The empty window with an egui panel, process start to the first frame
  presented, alternated, two sets of five:

  | | Ours | Bevy |
  |---|---:|---:|
  | first frame, set 1 (load 9 → 23) | median 195 ms | median 240 ms |
  | first frame, set 2 (load ≈12) | median 148 ms | median 201 ms |
  | Bevy minus ours, per pair | | median +58 ms in both sets |
  | peak memory | 100 MB | 205 MB |
  | release binary | 9.2 MB | 96 MB |
  | clean release build | 21 s | 88 s |

  - With content, Bevy's asset path (each texture written through its render world) cannot take the pack's
    pipelined, mapped-memory upload, which is where the spike's 99 ms comes from.
  - What Bevy would bring (an ECS, an inspector, PBR, glTF loading) is not what this app needs: CS2's materials,
    lightmaps and smoke are our own shaders, the packs are our own format, and a round's replay is ten players and a
    few dozen grenades.
  - Its API also changes every release.
- **Other engines:** Fyrox 1.0's only graphics server is OpenGL (`fyrox-graphics-gl`). macOS stops at OpenGL 4.1,
  with no compute shaders and no BC6H or BC7, the formats of the game's textures and lightmaps. three-d 0.19 is OpenGL
  too.
- **Raw Metal instead of wgpu:**
  - What Metal alone offers (loading straight from storage into GPU resources through `MTLIOCommandQueue`, pipeline
    binary archives, MetalFX upscaling) would mostly be hidden on the first frame. The pack loads on its own thread
    while AppKit spends ≈76 ms making the window (S3), so a faster load saves at most the part that outlasts it, an
    estimated 10–20 ms.
  - The cost would be a Mac-only renderer and Windows given up.
  - If a measurement ever shows a Metal-only win, wgpu's `as_hal` hands over the raw `MTLDevice` and textures for that
    one path, without leaving wgpu.
- **Known limit:** wgpu 30 has no pipeline cache on Metal, so macOS's own shader cache does the job. 10 pipelines took
  1 ms warm and 105–121 ms after a shader change (in parallel). Precompiled `.metallib` through wgpu's passthrough
  shaders is the fallback for a first launch.

## S3 · winit 0.30 and egui 0.37

- **Decision:** winit 0.30.13 (the stable line; 0.31 is in beta), egui 0.37 drawn by egui-wgpu in the frame's last
  pass, input through egui-winit.
- **Evidence:** the workspace resolves to one wgpu (30.0.1) and one winit (0.30.13) across egui-wgpu, egui-winit and
  our crates (`cargo tree -i wgpu`, `-i winit`). wgpu-profiler 0.28 and naga_oil 0.23 share the same wgpu and naga 30.
- **M0's window** (`--exit-after-first-frame`, warm, n=6, load 5–7): the empty shell's first frame was presented
  120–124 ms after the process started.

  | Stage | ms |
  |---|---:|
  | before `main` | 5 |
  | AppKit's event loop | 30 |
  | launch | 14 |
  | window and its layer | 32 |
  | surface and device | 3 |
  | egui | 1 |
  | first frame (CPU) | 28 |
  | first frame (GPU) | 8 |

  A newly built binary's first launch spends 0.3–14 s before `main`, once: macOS checking the new executable.
- **Consequence:** AppKit takes ≈76 ms before a window exists. The map loads on another thread from `main`, so the two
  overlap (SPEC.md, "Start-up").
- **Rejected:**
  - iced (retained mode with its own renderer: mixing in a custom 3D pass is harder);
  - Slint (declarative, a second language);
  - an HTML UI over a native view (the browser again).

## S4 · The game's own textures, in a memory-mapped pack

- **Decision:** one pack per map (`crates/pack`): a header, a postcard table, sections on 16 KiB boundaries. Textures are
  the game's BC blocks with their mip chains, read from `.vtex_c` (`crates/source2`).
- **Evidence:**
  - A census of the local CS2 cache (3,333 `.vtex_c`): BC7, DXT1 (BC1), ATI1N (BC4), one RGBA. A sample of 150 put BC7
    at 74% of the pixels.
  - BC is 4× fewer bytes to stage than RGBA8 with mips (Cache: 470 MB against 1.9 GB) and needs no decode.
  - The BC pack loads 74 ms faster than the RGBA pack (S2).
  - In the browser (swarm 3 lane 01), the same blocks cut CPU by 24–35% and memory by 28–37%, with no visible change.
  - The game's lightmaps are BC6H (irradiance), BC1 (direction) and BC4 (baked sun shadows).
- **Format notes:**
  - Mips are stored smallest first; each is LZ4 when that is smaller.
  - Normal maps are hemi-octahedral in the game's files, so they are decoded in the shader, not converted.

## S5 · The database, read-only and briefly

- **Decision:** DuckDB `~1.20000.0` (bundled), the build `nades` writes with. Every read opens the file read-only, runs
  its queries and closes it (`crates/data`). The queries use the macros `nades` keeps in the file.
- **Evidence:**
  - `nades` locks its database while it runs, and an open connection makes it fail (show-me-the-nades AGENTS.md).
  - Read-only opens coexist with the web app's API: the debug database opened while its API server held it.
  - `lineup_report` for a map: 32–51 ms from open to close.
- **Rejected:** a long-lived connection (it would block ingests); copying the data into the pack (it would go stale
  as the dataset grows).

## S6 · Line of sight against awpy's collision mesh

- **Decision:** the result camera and a smoke's light use awpy's collision mesh and a BVH over it, as the web view
  does since show-me-the-nades' D26.
- **Evidence:** in Rust, a ray against the mesh costs 0.2–0.8 µs, and its BVH builds in 23 ms (Cache) to 39 ms
  (Inferno) (swarm 3 lane 03). A smoke's ≈34,000 light rays then take milliseconds, not the browser's 200–270.
- **The cost, as in D26:** the mesh lacks some props and glass. On Cache, 4.8% of its result cameras see less than a
  quarter of the effect when judged against the drawn triangles.

## S7 · M1 built twice: our own app shell against Bevy as the shell

- **Run:** 2026-10-26, at the user's request to build both designs and compare them. The contract and the rule were
  fixed before either build started (`docs/m1-bakeoff.md`). The same pack, camera, shading and flags; two agents
  with the same brief.
- **Evidence** (`.runs/m1-compare-20261026-111011/`, the coordinator's alternated runs):
  - the map on screen: A 111.3 ms, B 123.0 ms (median of 7; A lower in 5 of 7 pairs);
  - steady frame: A 2.40 ms, B 2.51 ms;
  - identical pictures (mean difference 1.27/255);
  - design-specific code: A 1,249 lines, B 1,096.
- **By the declared rule, B wins.** The first frame missed the 10% / 6-of-7 bar by half a point, the steady frame tied,
  and B has 12% less code of our own.
- **Outside the rule, everything else favours A:**
  - A is at macOS's floor (its window's appearance), while Bevy's first update and extract add ≈12 ms after it;
  - clean builds 17.8 s against 51.7 s;
  - binaries 10.3 MB against 59.6 MB;
  - far fewer crates than Bevy's 301;
  - egui current rather than a version behind;
  - no engine API churn.
  B's renderer is our own wgpu code inside Bevy, so Bevy contributes the window, input, egui wiring and frame loop:
  the ≈150 lines A writes itself.
- **Decision: A, our own renderer,** chosen by the user on 2026-10-26 over the rule's pick.
  - The rule's tie-break weighed code size alone, and B's smaller code comes with a 300-crate engine used only as
    an app shell.
  - A is at the platform's floor on the first frame, builds three times faster and ships a sixth of the binary.
  - B's build stays in `prototypes/m1-bevy` as the reference.

## S8 · CS2's materials in one shader over bindless textures

- **Decision:** one WGSL shader (`crates/render/src/map.wgsl`) draws every material family the pool maps use:
  - csgo_environment and csgo_environment_blend, with up to three layers;
  - csgo_lightmappedgeneric's two layers;
  - csgo_effects;
  - unlit and black surfaces;
  - a generic fallback.
  Pipelines differ only by stage (opaque, alpha-tested, blended, additive, effects, additive effects) and culling.
  Every texture of a map sits in one `binding_array<texture_2d<f32>>`, which each draw's material (784 bytes, in a
  storage buffer) indexes. The map is one bind group, and its draws are recorded once into render bundles.
- **Evidence:**
  - de_cache has 1,208 materials, 687 textures and 1,276 draws.
  - The texture array needs wgpu's `TEXTURE_BINDING_ARRAY` and
    `SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING` (argument buffers on Metal).
  - Reads inside a layer's branch are non-uniform, so every texture is sampled with `textureSampleGrad`, from
    derivatives taken once at the top of the shader.
  - The whole map in CS2's look takes 4.1 ms a frame offscreen at 2880×1800 (SPEC.md §9).
- **Rejected:**
  - a bind group per material (1,208 of them, and a rebind on nearly every draw);
  - texture arrays by size and format: the game's textures come in many sizes and five BC formats.

## S9 · The game says which meshes its lightmap lights

- **Decision:** `nades-pack` reads each world node's mesh flag `m_bHasBakedLightingFromLightMap` from the map's VPK
  through Source 2 Viewer's CLI (`-i <map>.vpk -f maps/<map>/worldnodes/ -e vmdl_c -b MDAT`). A mesh the game lights
  from probes gets no lightmap coordinates. A mesh the CLI does not name falls back to the web view's rule. Without
  the CLI, the rule alone decides, and the report says so.
- **Evidence:**
  - The web view's rule (TEXCOORD_2, else TEXCOORD_1 when it looks like an atlas) lightmapped 70 of de_ancient's
    probe-lit primitives, which drew black.
  - The export cannot tell the two apart. Source 2 Viewer renames the lightmap stream (`texcoord` 3, R16G16_UNORM) to
    the next free TEXCOORD, and glTF-Transform quantizes every [0, 1] set to 16 bits, so a probe-lit mesh's second
    UV set looks like an atlas.
  - The game's answer: de_ancient lightmaps 255 of its 368 world meshes, de_cache 542 of 783.
- **Tried and reverted:** applying `m_vLightmapUvScale` (1.14284 on every pool map) to the coordinates. The export's
  coordinates already carry it, and applying it twice moved every chart.

## S10 · The game's baked light, as Source 2 Viewer reads it

- **Decision:** port Source 2 Viewer's reading of CS2's lightmap version 2 rather than invent a model:
  - diffuse = albedo × (sun · N·L · (1 − the shadow mask's sun channel) + indirect) × occlusion, with no 1/π;
  - the directional lightmap turns the indirect light by the normal map (`ComputeLightmapShading`, clamped at 5°);
  - shadow masks wider than 4096 texels are sampled bicubically;
  - specular is GGX without 1/π, and the sky's reflection is normalised by the irradiance.
  - Where no lightmap reaches, the light probe volumes light the surface. Each `env_combined_light_probe_volume` holds a
    BC6H atlas of six ambient-cube slabs and a BC7 shadow volume. The volumes are placed as
    `SceneLightProbe.CalculateGpuProbeData` places them and blended per pixel by priority, with their edge fades. The
    sky fills what no volume covers.
- **Evidence:**
  - Shadow masks differ by map: BC4 (cache), BC5 (inferno, anubis) and BC7 (dust2, nuke, ancient). The sun's channel is
    its `light_environment`'s `bakedshadowindex`, read from the decompiled entity lump.
  - The probes: 67 volumes on de_cache (atlas 150 MB, shadows 25 MB) and 82 on de_ancient (30 MB, 5 MB).
  - csgo_lightmappedgeneric's two layers blend by the vertex paint in `_TEXCOORD_4.x`. The first guess, the vertex
    colour, darkened dust2's walls.
  - The lightmap blocks in the pack decode to the export's own `irradiance.exr` exactly, every texel (the
    `export.rs` tests).
- **Facing masks, beyond Source 2 Viewer:** a blend layer can be kept to surfaces facing a direction
  (`F_BLEND_BY_FACING_DIRECTION_N`): its paint is scaled by `smoothstep` of the normal against that direction, as
  the game's vertex shader does. Source 2 Viewer skips mode 2 ("Normal Map") on layer 3, which leaves de_ancient's
  "topmoss" under every overhang; the game's shot shows bare stone there. The sandbox draws modes 1 and 2 with the
  surface's normal. de_ancient has 45 such materials and de_cache 118. The change took de_ancient's blurred |Δ| from
  14.1 to 12.7 and its saturation error from 0.056 to 0.005 (S12).

## S11 · What the pack takes from the game, and how big it gets

- **Decision:** pack format 2 adds normal maps (hemi-octahedral, decoded in the shader), height and mask textures, the
  directional lightmap and the baked shadows, the light probes, the sky's cube, the 3D skybox with its own lightmaps,
  the post-processing (tone curve, exposure, bloom, the colour lookup table, fog) and vertex paint.
  - Normal maps and the upper layers' colours are capped at 1024 texels, and data textures (heights, masks,
    occlusion) at 512. Base colours and lightmaps keep every level.
  - Skinned meshes are posed by their joints when packed: glTF ignores a skinned node's transform, which left anubis's
    cloths in the wrong place.
- **Evidence:** the packs, built in 1.5–3.9 s each:

  | Map | Pack | Colour | Normals | Data | Lightmaps (map + skybox) | Probes |
  |---|---:|---:|---:|---:|---:|---:|
  | cache | 1,078 MB | 299 MB | 185 MB | 58 MB | 143 MB | 175 MB |
  | ancient | 1,111 MB | 152 MB | 109 MB | 27 MB | 310 MB | 35 MB |
  | inferno | 1,385 MB | 331 MB | 236 MB | 63 MB | 177 MB | 39 MB |
  | dust2 | 658 MB | 180 MB | 88 MB | 10 MB | 177 MB | 28 MB |
  | nuke | 652 MB | 138 MB | 149 MB | 22 MB | 178 MB | 36 MB |
  | anubis | 651 MB | 145 MB | 107 MB | 9 MB | 177 MB | 58 MB |
  | mirage | 347 MB | 97 MB | 36 MB | 2 MB | 135 MB | 13 MB |

  de_cache was 640 MB in format 1. Pack loading is not the first frame's limit (SPEC.md §9).
- **Rejected:** decoding the normals to RGB at pack time. That means RGBA8, four times the bytes of the game's blocks,
  or re-encoding them, which is slow and lossy. The shader's hemi-octahedral decode costs a few instructions.

## S12 · The realism check: no worse than the web view, measured without cancelling, the game's shots left in place

- **Decision:** `bench/realism.py` renders each pool map from its in-game lineup shot's camera
  (`nades-sandbox --render`) and measures the render with the verify-scene-realism skill's own `compare.py`, loaded
  where it is. Beside the skill's means (Δ luma, Δ top half, Δ saturation) it reports blurred |Δ|: the mean absolute
  luma difference at 1/16 resolution, over the whole frame and its top half. The verdict per map is "differs from the
  game no more than the web view's latest run" by blurred |Δ|, whole and top half.
- **Why not the skill's means alone:** a mean lets a too-bright patch cancel a too-dark one. On de_ancient the web
  view's bright clouds offset its dark mossy cliff, so its top half reads −1.4 while differing more than ours. The
  skill sets no pass line ("the numbers are not pass or fail").
- **The game's shots stay where they are:** they come from jumpthrow.pro, whose pages, and what is read from them, stay
  in show-me-the-nades' `.verify-runs/`. The sheet holds the sandbox's and the web view's renders, and the report names
  each shot's file. A first version pasted the shots into the sheet; those copies were deleted.
- **Evidence** (`.runs/realism-20261026-132923/`, against the web view's run of 2026-10-25 17:16):

  | Map | blurred \|Δ\| | top half | web | web top half | Δ luma | web Δ luma |
  |---|---:|---:|---:|---:|---:|---:|
  | dust2 | 6.5 | 2.9 | 10.3 | 6.8 | +4.9 | +3.6 |
  | cache | 16.7 | 13.4 | 18.6 | 15.7 | +2.8 | +2.0 |
  | anubis | 5.5 | 1.5 | 6.2 | 1.9 | +2.9 | +3.3 |
  | mirage | 10.7 | 8.3 | 11.3 | 9.0 | +7.5 | +4.7 |
  | inferno | 12.7 | 4.4 | 24.1 | 15.9 | +11.9 | +23.4 |
  | ancient | 12.7 | 9.3 | 15.8 | 10.5 | +1.0 | +5.3 |
  | nuke | 7.3 | 3.3 | 9.9 | 5.5 | +5.8 | +4.1 |

  - No worse than the web view on all 7 maps by blurred |Δ| (mean 10.3 against 13.7) and its top half (6.2 against
    9.3).
  - The skill's headline, mean |Δ luma|, is 5.3 against 6.6.
  - By each map's whole-frame Δ luma, 4 maps are further from the game than the web view: dust2, cache, mirage and
    nuke, each brighter by 0.8–2.8 levels more. The skill reads the top half when, as here, every map leans the same
    way: the game's frame carries the player's hands and a watermark.


## S13 · Publish M2 as a source preview

- **Decision:** the first public snapshot is v0.1.0, the working M2 map viewer and its production crates,
  benchmarks and design records. Lineup browsing and grenade playback remain M3.
- **Evidence:** SPEC.md marks M2 complete; the current app renders de_cache with the fly camera. Asset
  preparation still depends on the separate, unpublished show-me-the-nades exporter. The README and
  setup guide state that limitation instead of implying a standalone installation.
- **Publication boundary:** include the user's app-rendered screenshot and MIT-license the project's source.
  Keep game files, exports, packs, databases, local benchmark outputs and historical prototypes local.
  Earlier entries refer to the local prototype paths as historical evidence; those experiments are not
  distributed in this preview.


## MCP v1 · One application owner, inspectable state, and isolated headless rendering

- **Decision:** a separate `nades-mcp` stdio process manages one window through private child pipes and typed
  `nades-control` commands. The winit event loop remains the sole owner of view mutations. A new process receives a
  new instance ID; frame IDs and applied/presented revisions make tool results inspectable.
- **Why:** CLI wrappers cannot change or inspect a running camera. An embedded-only MCP cannot restart itself after
  app shutdown. Sockets add discovery and stale endpoint ownership that v1 does not need: manual attachment is out of
  scope. Private pipes isolate simultaneous agents without introducing a network listener.
- **User choices:** the first release exposes nine tools; map changes may restart; only agent-created windows are
  controlled. MCP disconnect hands a ready window to the user, with no later reattachment. Incomplete launches and
  offscreen workers are cleaned up. Persistent headless sessions are future work; headless screenshots are in v1.
- **Headless contract:** a screenshot records its camera, dimensions and material-animation time. `view_render` uses
  those same inputs without changing the live window. GPU exposure currently has no settling history, but animated
  material masks depend on time, so matching the camera alone is insufficient for a meaningful pixel comparison.
- **Observability:** status describes actual scene/camera/viewport and suggests next tools. Metrics distinguish CPU
  submission wall time from GPU time; unavailable GPU measurement is null. Images are native MCP content and omitted
  from textual logs. The spec and repeatable verifier, rather than successful compilation alone, define completion.
- **Contract and evidence:** [MCP specification](mcp-spec.md). Real acceptance reports are local under `.runs/mcp-*`;
  the specification lists all nine per-tool checks and the overall journeys. No asset bytes are published.
- **Principles that changed the design:**
  - **Boundary Discipline** and **Foundational Thinking:** shared typed commands/state live outside MCP and winit;
    external input is validated before dispatch, and the event loop owns mutations.
  - **Laziness Protocol:** private child pipes avoid endpoint discovery, a broker and manual-attachment state.
  - **Make Operations Idempotent:** repeated start on the current map reuses the child; repeated stop is harmless.
  - **Build the Lever**, **Test Behavior Not Implementation** and **Prove It Works:** the reusable verifier drives
    the real stdio protocol, validates advertised schemas and inspects actual process exits and rendered pixels.
  - **Fix Root Causes:** the database worker is cancelled and joined before native global teardown, rather than
    suppressing the crash or bypassing normal shutdown.
  - **Explain the Number:** CPU samples and native memory readings retain their measured definitions; GPU time stays
    unavailable, and repeat-render tolerances come from pinned-input pixel comparisons.

### MCP v1 rendering reproducibility · measured pixel tolerance

The repeated-headless criterion is a maximum channel error of one 8-bit display level and mean channel error at most
0.001, with identical binary, map, pose, FOV, size and material time. Literal PNG or pixel hash equality is not promised.
In the first acceptance run, two fresh offscreen processes differed in 101 of 8,201,920 RGBA channels (84 pixels),
maximum one level, mean 0.000012314. Five additional renders with pinned binaries and identical inputs yielded three
with that same tiny difference and two pixel-identical results. The difference is confined to a small image region;
its precise driver/shader cause has not been isolated, so this is a measured acceptance envelope, not a claimed fix.
The independent live/headless gate remains mean error ≤1 level, and exact state/camera/frame provenance is checked
separately. Inputs, binary/image SHA256s, stage marks, load averages and comparisons are in
`.runs/mcp-1793045083732279000/repeat-sampling.json`. A different camera, timestamp, image size or substantial pixel
drift cannot satisfy the repeat criterion.

## S13 · show-me-the-nades as a library, without its CLI's crates

- **Decision:** show-me-the-nades gains a library target (`src/lib.rs`: `extract`, `physics`, `places`, `tracks`, and a
  new `voxels`), and the sandbox depends on it by path with `default-features = false`. Its CLI's own dependencies
  (Chrome, clap, DuckDB, tokio and the rest) move behind a default `cli` feature that the `nades` binary requires
  (`required-features`), so the binary and its default build are unchanged.
- **Why:** the voxel log's decoder (`tracks::voxel_frames`) and the track types (`PathPoint`, `Flame`) live there, and
  copying them would let the two drift. Without the feature split the dependency would bring 195 crates, 118 of them
  chromiumoxide's; with it, 37 (awpy, anyhow, rstar).
- **The emitters:** `voxel_frames` counts each frame's emitters but does not keep their cells, which the cloud spreads
  from. `voxels::voxel_frames_with_emitters` (a new file there) reads them out of the same frames, after
  `voxel_frames` has validated them. Folding the cells into `tracks::VoxelFrame` would be the tidier end state; the
  brief allowed new library files only.
- **Evidence:** show-me-the-nades' `cargo test --release` (13 library and 37 binary tests, then 15 library with
  `voxels`'s) and `cargo clippy --release --all-targets` pass, and `cargo clippy --no-default-features --lib` builds the
  library alone.

## S14 · The working tree's views, applied as temp objects on every read

- **Decision:** every read (`crates/data`) applies the statements of show-me-the-nades' `sql/views.sql` that its queries
  need (`lineup_report`, `throw_scene`, `scene_viewers`, `team_demos`, `slice_players` and what they call) as temp views
  and macros before it runs them. `NADES_VIEWS_SQL` names another file; without one, the definitions stored in the
  database serve.
- **Why:** `nades` writes the views into the file only when it opens it, so the user's file can hold older ones: on
  2026-10-26 its `lineup_report` had no `example_throw_id`, the column the list opens throws by. The web app's API does
  the same (`toReadOnly` in `web/server/src/views.ts`): the temp catalog is writable on a read-only connection, and names
  resolve to it first. The sandbox then reads exactly what the web view reads.
- **Cost:** the statements are chosen once (a closure over the names each statement uses, comments and strings masked)
  and run per open. The list's read (open, views, `lineup_report`, teams, players, close) took 93–114 ms with DuckDB held
  to 4 threads, beside drawing; it is off the first frame's path (S16).

## S15 · The collider from awpy's mesh at run time, tested as three.js tests a ray

- **Decision:** the result camera and the smoke's light use awpy's collision mesh (`~/.cache/nades/maps/<map>.mesh`, the
  web view's collider since its D26) under a BVH built when a map opens (`crates/scene/src/collider.rs`: binned surface
  area, built in parallel). Each triangle is tested with three.js's watertight ray/triangle test in doubles, a hit's
  distance is |point − origin| as three-mesh-bvh reports it, and `clear_between`, `blocked` and `floor_below` compare
  distances as `render/mapModel.ts` does. The tree only decides which triangles are tested, so it need not be
  three-mesh-bvh's. The pack's reserved Collider section stays unused.
- **Evidence:**
  - Build time with the file's read, per map: mirage 5.6 ms (73,290 triangles), nuke 8.0, dust2 15.3, cache 22.0, anubis
    25.7, ancient 33.4, inferno 39.0 (547,966). It runs after the first frame, on its own thread.
  - A random-ray test checks the tree against every triangle, exactly (`the_tree_answers_as_testing_every_triangle_would`).
  - Same cameras (S18): 84 of 84 result shots are the web view's.
- **Rejected for now:** the collider in the pack (format 2 reserves the section). Building it costs less than the list's
  read and is off every path that waits, so a pack rebuild for all seven maps buys nothing yet; M4, with every map
  resident, can revisit it.

## S16 · Stages built ahead, off the window's thread

- **Decision:** a lineup's stage (its cameras, its cloud and the light the map lets reach it, its fire, its result shot)
  is built before it is picked. After the first frame the data thread reads the list, then each listed lineup's example
  throw (`throw_scene`, `scene_viewers`), the selected one first and then outward from it, eight to a read; a newer
  request (a lineup just picked) goes first. Each throw's stage is built on every core (rayon) once the map's collider
  is there. A switch takes a built stage: the window's thread writes the tube's vertices and the 128 KB cloud volume,
  and draws.
- **The first frame needs no database:** the last run's map, lineup, view and camera are remembered per map
  (`~/.cache/nades/sandbox/state.json`, `NADES_SANDBOX_STATE`), so the first frame is drawn from the camera it was left
  at, and the list, the collider and the game's effect files are read after it. A map with no remembered camera stays
  clear until its first lineup's camera is known, rather than being drawn from another map's (Cache's bench camera puts
  Inferno out of bounds). A map picked in the shell is loaded on a thread and shown once its first lineup is built:
  1.55–1.72 s from the pick for Mirage, Inferno and Cache (`--bench-maps`), the map before staying up meanwhile.
- **Evidence** (`--bench-switch`, de_cache, a 1444×1420 view, vsync off, each switching frame finished on the GPU, load
  8–12 while another agent compiled):

  | View | Switch, CPU (median, p95, max) | Frame with the new lineup (median, p95) |
  |---|---|---|
  | Lineup | 0.53, 0.85, 1.06 ms | 4.9, 7.9 ms |
  | Result (the smoke drawn) | 0.50–0.52, 0.82–0.87, 0.96–1.04 ms | 5.6–7.1, 7.1–8.9 ms |

  One frame in 60–120 took 37–40 ms in two of the four runs, a hitch outside the switch. A smoke's stage builds in
  2.8–3.0 ms, 2.4–2.6 of them its light (≈34,000 rays on 18 cores); the web view takes 200–270 ms for the light alone.
- **Start-up holds:** de_cache on screen in 172.4 ms from process start (median of 9, 158–189, load 8–11; window made
  109.8, map loaded 115.2, presented 139.2, shown 172.4), as M2's 172; reading the list before the first frame made the
  window appear later (≈148 ms against 110 in one run), which is why it waits. Steady frame offscreen at 2880×1800:
  4.18 ms (M2 4.08: the composite pass). Peak footprint 2.07 GB (M2 2.0).

## S17 · One composite pass, always: the scene into its own target, the smoke averaged in

- **Decision:** the sky, the map, the throw's meshes and the effects draw into a scene target; a composite pass
  (`smoke.wgsl`, a port of `render/composite.ts`'s smoke) writes it into the HDR target the post-processing reads,
  raymarching the shown smoke through its volume against the scene's depth, in Hammer units (the camera's ray is turned
  into the database's frame). A still picture averages 16 jittered frames by the blend constant, as the web view's
  accumulation does; a frame that changes nothing after that is not drawn. The fire's sprites and ground draw in the
  effects pass, which reads the depth; the flight's tube, its marks and the grenade draw in the map's pass. The fire's,
  a flash's and an HE's point light is added to the map's diffuse light in `map.wgsl` as three.js lights a standard
  material (its Lambert divides by π; CS2's baked light does not).
- **Why always:** without a smoke the pass is a copy (≈0.1 ms at 2880×1800), and one path through the frame keeps the
  start-up's pipelines and the steady frame's passes the same with and without a throw.
- **The game's own effect files**, where show-me-the-nades' `scripts/cs2_effects.py web` wrote them
  (`~/.cache/nades/cs2/effects/web`): the smoke's 128³ noise and the fire's flipbooks, read after the first frame; the
  stand-ins (`noise::stand_in`, the fire's fbm) where not, as in the web view (its D27).

## S18 · Checked against the web view: the same cameras, the same effects

- **Same cameras** (`bench/cameras.py`, `.runs/cameras-20261026-150526/`): for the web's three most thrown lineups of
  each grenade on all seven pool maps (84 throws), the web view's own code (`stage.ts` read in place, fed by its API on a
  clone of the database and awpy's mesh in a three-mesh-bvh tree) and `nades-sandbox --cameras` chose the same camera
  for every view: Lineup, Aim, Result and the orbit's first pose, eyes 0.0000 units apart and angles 0.0° (the result
  shots: 36 opponents' views, 11 blinded players', 37 standing views, each the same player at the same tick). Choosing a
  result shot takes 0.18 ms here (median) and 2.1 ms in the web view's code under Bun.
  - The ports keep the web view's arithmetic: doubles, `Math.hypot` as JavaScript engines compute it, three's
    `normalize` as a multiply by the reciprocal, JavaScript's rounding.
- **Effects** (`bench/effects.py`, `.runs/effects-20261026-150851/`): the result shots of the web's most thrown lineup of
  each grenade on de_cache, de_mirage and de_inferno, at the moment the view opens on, shot by the web view's own tool
  (the verify-nades-scene skill's `effect_shots.ts`) and rendered by the sandbox (`--render --throw`). The maps differ
  (S12), so each effect is compared as what it changes against the same shot a second before it went off. The changed
  regions overlap by 0.56–0.94 of their union (median 0.75) and cover the same share of the frame within 0.012 for 9 of
  the 12; the smokes' mean colours are within 3 levels on Cache and Mirage, and 8 brighter on Inferno. Where they differ
  most: Inferno's flash changes 0.21 of the frame here against 0.13 there (its light reaches further up the walls), and
  the fires' shares differ by 0.09–0.10 on Cache (the web's larger) and Mirage (the sandbox's).
- **Orbit is free:** it opens where the web view's orbit move starts, the thrower's view at release (the fourth camera
  checked above), and from the first drag circles the effect as three.js's OrbitControls do (80–5,000 units, from
  straight down to a little past level). The web view's scripted chase and turn (`flight.ts`) are not ported.

### MCP integration · preserve numerical ports when fixing lint

The web's ember phase is literally `6.28` (`fire.ts`), and its sky ray raw Z is literally `Math.SQRT2 / 1.4142`
(`render/volumes.ts`). Keep these values number for number. Targeted Clippy expectations document these intentional
approximations; substituting mathematical constants changes the port, even if the numerical difference is small.
The final camera/effect verification uses database clones. The MCP itself continues to reuse the application's
existing cameras and renderer rather than introducing independent rendering math.

### MCP integration · stop native database work before process teardown

A protected-copy startup run exposed SIGSEGV in the background `data` thread's DuckDB keyword helper. The macOS
crash report's binary UUID matched the pinned probe; another report showed main concurrently in `std::process::exit`
and `__cxa_finalize_ranges`. The application discarded its worker JoinHandle, allowing native queries to outlive
C++ singleton state. Library now owns its cancellation signal and JoinHandle, wakes busy/collider waits and joins
the native database worker before process exit. It does not skip queries or bypass C++ teardown to hide the fault.
Ten actual first-frame launches on a database clone and three immediate controlled shutdowns then exited zero
(`.runs/mcp-app-shutdown-fixed/stress.json`). MCP stop checks now surface unexpected exit codes as `app_exit_failed`
and forced cleanup as `app_stop_forced`, retaining actual stopped state and diagnostics instead of claiming clean
shutdown. Process-boundary regressions prevent a repeat from silently passing the quality gate.

## S19 · Every pool map resident, within a budget

- **Decision:** the maps not shown stay on the GPU (`crates/sandbox/src/resident.rs`), each with what it holds
  (`Map::gpu_bytes`: its textures as Metal allocated them, and its geometry).
  - Once the first frame is on screen (presented, and the window shown) the other packed maps open in the background,
    one at a time (the ones looked at before first), each only while it fits beside what is held. Each is drawn twice
    offscreen as it arrives (S21), its collider built, its lineup list read and its selected lineup's stage built.
  - Picking a kept map is a swap, in the frame that handles the pick: the GPU maps, and the lineups (`MapLineups`:
    list, selection, stages, collider). The targets are made again for the map (its post-processing is its own),
    0.3 ms.
  - A map that does not fit is loaded when picked (M3's path), after the maps shown least recently have been let go
    to make room for it.
  - The budget: `--budget-mb`, else `NADES_SANDBOX_BUDGET_MB`, else 8 GiB but no more than half of what Metal
    recommends a process keep (`recommendedMaxWorkingSetSize`); 4 GiB where the GPU does not say. All seven pool maps
    hold 6.2 GB.
  - The effects' textures (the smoke's noise, the fire's flipbooks) are made once and every map binds them.
  - The data thread serves in three tiers: the shown map (its list, a lineup just picked and its neighbours), then the
    kept maps (their lists and selected stages), then the rest of the shown map's stages. With two tiers the kept
    maps' lists waited 24 s behind 300 stage reads; with three every map is warm 1.6–1.75 s after start-up.
- **Evidence** (`python3 bench/resident.py`, `.runs/resident-20261026-163237/`: 3 runs of 2 rounds of the seven maps,
  load ≈12.3, each frame finished on the GPU with vsync off):
  - 42 of 42 picks showed in the next frame. That frame took 5.3 ms (median; GPU 3.7; worst 8.9), and 5.7 ms on a
    map's first visit in a session (GPU 4.1; worst 8.0); the window's own share of a switch is 1.6 ms.
  - 6,221.8 MB on the GPU of an 8,589.9 MB budget; footprint 6.32–6.56 GB at the end, 6.32–6.62 GB after each pick,
    peak 6.46–6.74 GB.
  - The maps opened in 49–97 ms each; the shown map's frames meanwhile: median 4.7–4.9 ms, p95 15–17, worst 40–56.
  - The first frame is no slower for them: they start once the window shows (on screen 169.5 ms with them, 167.4
    without, medians of 7 alternated pairs; −2.1 ms per pair). Started at the first frame's present instead, the window
    showed 4.7 ms later per pair (median).
  - Under `--budget-mb 3000`: two maps kept beside de_cache, the other five loaded when picked (≈1.5 s each), held at
    most 3.05 GB throughout, peak footprint 4.3 GB.
- **Rejected:** targets kept per map (≈50 MB each at 1444×1420, to save 0.3 ms); one renderer with the maps' bind groups
  swapped in (the swap is the same work, and the map's pipelines, bundles and post-processing are already its own);
  opening other maps before the first frame (it must not wait).

## S20 · de_cache's 2 GB, accounted for

- **The question:** de_cache peaked at a 2.0 GB footprint against M1's 0.72–0.75 GB, and the upload's staging, kept
  after the upload, was the suspect.
- **Method:** the process's footprint and its graphics share at every start-up mark (`nades_render::memory`, macOS's
  `TASK_VM_INFO`), and two probes: `cargo run --release -p nades-render --example upload_memory -- <pack> [--vmmap]`
  (a map's upload step by step, then frames offscreen) and `--example first_submit` (a device with no map).
- **What holds it,** de_cache:
  - the textures, as Metal allocates them: 982.8 MB for 855.7 MB of BC blocks (Mirage 310.3 for 280.1, Inferno 896.2
    for 845.1);
  - the driver, once a process: its first submission 118.5 MB of GPU memory and its first render pass (1444×1420
    half floats) 198 MB more, with no map at all; Mirage's first frames add as much as Cache's, and ≈270 MB of it
    outlive the map;
  - what drawing the map's triangles holds: 170–360 MB of memory the driver owns for the process, one region a texture,
    growing with the triangles drawn (Inferno 569 MB, Mirage 55), on the first two frames;
  - the window: ≈150 MB between the first frame's present and the window shown (drawables, CoreAnimation).
- **Not the staging:** the buffer over the pack's Textures section adds nothing to the footprint during or after the
  copies, and letting it go changes nothing; the geometry's buffer over the mapping is not charged either (its pages
  are the file's cache). With copied buffers instead, as off Metal, the peak reached 3.3 GB.
- **Bounded:** where a backend charges the staging (off Metal), the textures go through it a chunk (≈32 MiB) at a
  time, at most 256 MB in flight, instead of one copy of the whole section; across maps, the budget (S19). M1's
  0.72–0.75 GB came before most of the above: format 1's smaller textures, one pass, no post-processing.

## S21 · The first frame: the map warmed twice, and the window server's share

- **Decision:** the loader draws two warm-up frames, from the camera the first frame is drawn from, while AppKit makes
  the window; a map opened in the background is warmed the same way, from its remembered camera or the middle of its
  draws, and once its lineup's stage is built the window's thread draws it once more offscreen from that lineup's view,
  with the stage (one map a frame, submitted and not waited for).
- **Evidence:**
  - Offscreen, a map's first frame costs the GPU 27–58 ms, its second 10–28, its third ≈7; a new camera's first frame
    ≈12 against 5–8. The first-use cost is the map's more than the camera's.
  - With one warm-up frame the window's first frame waited 21 ms for the GPU; with two, 4.5–4.9 ms (7 alternated
    pairs).
  - A map's first visit in a session: 13.5 ms that frame (median; worst 32.4) with the loader's two warm-up frames,
    5.7 ms (worst 8.0) with the lineup's too, against 5.3 for a later visit. What it costs: the shown map's frames
    while the others open went from p95 ≈10 ms to 15–17 (`.runs/resident-20261026-161418/` against `-163237/`).
  - de_cache on screen 168.8 ms from process start (median of 9, 160–180, load ≈10); de_inferno 185.1 (163–201,
    load ≈8.4; ≈239 before): `.runs/startup-20261026-163255/`, `-163315/`. Two earlier pairs of runs: 167.4 and 177.7
    (AppKit's window 37 ms after `resumed`), 169.5 and 194.1 (56–62 ms, under more load).
- **The window server's share:**
  - Metal's presented handler reports the first frame, presented into the layer before the window is visible,
    as skipped (presented time 0): the window appears with the layer's contents.
  - The window is shown 25–31 ms after the present (median), and making it without AppKit's opening animation
    (`NSWindowAnimationBehaviorNone`) changed nothing: 31.4 against 31.8 ms (7 alternated pairs).
  - AppKit makes the window 37 ms after `resumed` (38 in M3's run), and 56–62 ms when the machine is busier (57 in
    a run before any M4 change).
  - Ours after the map is ready is 7–9 ms.
- `--exit-after-first-frame` waits up to 0.5 s more for the presented handler, and records `frame:on_screen` or
  `frame:skipped`.

## S22 · The macOS app: a recipe, not a packager

- **Decision:** `just bundle` assembles `target/release/bundle/macos/Nades Sandbox.app`. It takes the Info.plist from
  `crates/sandbox/macos/Info.plist` (the version from Cargo) and the release binary, and signs the bundle ad hoc for
  this machine (`codesign --sign -`). cargo-packager and cargo-bundle would each be a new install for what twelve lines
  do.
- **It finds the packs and the database as the binary does:**
  - LaunchServices sets HOME for the apps it starts, so the packs, maps, effects and state are under `~/.cache/nades`.
  - The database and show-me-the-nades' views are where the build was made from.
  - `open --env NADES_DB=…` points it elsewhere.
- **Evidence:**
  - Started by LaunchServices (`open -n -W`, as Finder does), on a database copy and a scratch state:
    - first launch 1.39 s, of it 497 ms of pipelines (Metal's shader cache is the bundle's own), then 185 and 201 ms;
    - a screenshot of its first frame;
    - an all-maps session (`--bench-maps all`): six maps opened in the background, 300 lineups each read from the
      database, every pick shown in the next frame.
  - `codesign --verify` and `plutil -lint` pass.
- **Not done:** an icon; notarization, which only matters off this machine.

## S23 · Windows: checked from the Mac, not yet run

- **Decision:** `just check-windows` (`scripts/check-windows.sh`) type-checks and lints every crate for
  x86_64-pc-windows-msvc. DuckDB (bundled) and meshoptimizer are C++, which does not compile for MSVC here without
  Microsoft's SDK. Stand-in compilers answer their build scripts with empty objects, which `cargo check` and Clippy
  never link.
- **What runs differently there, as designed:**
  - wgpu draws through DX12, the only Windows backend the workspace enables;
  - no buffers over the pack's pages: the geometry is copied, and the textures go through at most 256 MB of staging
    (S20);
  - no early frame, so the first frame waits for the surface;
  - a 4 GiB budget unless asked for (DXGI's budget is not read yet);
  - no memory marks, pre-`main` time or presented handler;
  - the packer's synthetic test is for Unix only (its stand-in CLI is a shell script).
- **Evidence:** from a fresh target directory both pass with no warnings (Clippy with `-D warnings`).
- **Not done:** a build that links (the Windows SDK, or the GNU target with MinGW), and a run.


## MCP video · persistent headless throw rendering

- **Decision:** `lineups_list` finds the database's recorded example throw. `video_render` starts a session-owned
  job that loads its map and stage once, renders fixed timeline steps through the existing effect path, and streams
  PNG frames to FFmpeg/libx264. `video_status` reports progress and the local MP4; `video_cancel` stops the job.
  Camera keyframes use linear position/pitch interpolation and shortest-path yaw. The video and still renderer share
  the single headless worker permit. No audio or live-window recording is added.
- **Principles:** **Boundary Discipline** puts limits and camera validation on the shared request boundary.
  **Build the Lever** and **Test Behavior Not Implementation** extend the real MCP verifier with encoded-video,
  failure and subprocess-lifecycle checks. **Prove It Works** inspects the MP4 and decoded Molotov/smoke frames,
  not merely a successful encoder exit.
- **Evidence:** [MCP verification](../.runs/mcp-1793055808478007000/results.json),
  [clip metadata](../.runs/video-accept/panorama-result.json), and the video verification section of
  [the MCP specification](mcp-spec.md). An occluded live window caused one intermediate run to fail; the rerun passed.

## S24 · A generic contract: cs2-throws facts, the sandbox's own rules, producer tags

- **Decision:** the sandbox reads throws as facts through a port, `ThrowSource` (`crates/model`), from `cs2-throws`
  version 1 files (`crates/source-file`, SQLite through rusqlite, bundled), and derives everything else itself, in
  `crates/model` with tests: times and landing; the eye; technique (CS2's release model, ported from show-me-the-nades'
  `physics.rs` with its tests); the practice command; lineup grouping; round time and after-plant; landing callouts;
  viewpoints; the voxel log's decoder (ported from its `tracks.rs` and `voxels.rs` with their tests). A producer's own
  labels are tags: shown, filterable, never interpreted. `crates/data`, DuckDB and the `show-me-the-nades` path
  dependency are gone, and `crates/sandbox/tests/standalone.rs` checks that no path dependency leaves the workspace and
  that nothing names a sibling checkout. `--data` (then `NADES_SANDBOX_DATA`, the config file, the default;
  `crates/config`, which the MCP server can share) names the files; `nades-sandbox check` reports a file's problems and
  the features its facts allow; `tools/cs2_throws.py` writes the schema and a fixture with Python's standard library,
  which the tests read.
- **Why:** the reviewed proposal (docs/standalone-api.md, revision 2): anyone's demos, from any producer, on any map.
  Revision 1's "facts with the producer's labels" let one producer define what every filter means.
- **Refinements to §4, found while porting** (the proposal's text is updated to match):
  - **Throws outside play are not grouped.** In a demo with rounds, a throw in the warm-up or after its round ended
    starts no group and joins none. show-me-the-nades gives lineups to live throws only; grouping warm-up throws would let
    them anchor groups and split live ones. A demo without rounds (a practice server's) is all play.
  - **A fire is one kind for grouping:** a T molotov and a CT incendiary from the same spot are one lineup, as
    show-me-the-nades' `nade = 'molotov'` has them. The grenade filter still tells them apart.
  - **Viewpoints end at the recorded expiry, or 5 s after the detonation without one** (show-me-the-nades'
    `scene_viewers`), not at the nominal duration, and stay inside the throw's round when its ticks are known.
  - **A flash's blinded player without a recorded position stands at their sample nearest the detonation, within
    0.5 s**, matched by Steam id, else by name. show-me-the-nades' `throw_scene` placed them so from its samples; its
    export leaves the blinds' positions empty, since they are not facts the demo records for the event.
  - **The walking flag is carried, not used:** the verified classifier tells a walk from a run by speed (140 u/s), and
    the flag would change techniques against the results it was verified on.
  - **No initial velocity from the flight yet:** technique needs `m_vInitialVelocity` until that estimate is checked
    against the throws that record both, as §4 asks.
  - **The round-time filter is half-open** (`from ≤ s < to`), so the presets 0–20, 20–60 and 60+ share no boundary.
- **API beyond §5:** `ThrowSource::map_of` (a throw's map, for `--render --throw` and `--cameras`); `Throw::flight_end`
  (the flight's last point, which lands a throw that never went off without reading its flight); `Demo::file_name`;
  `Derived::effect_recorded` and `release`; `Viewpoint::origin` (the result camera judges from the standing eye, as the
  web view does); `SmokeVoxels::origin` optional; `Library::viewpoint_window` and `blind_window` (which samples to ask
  the source for); `Derived::of` (one throw without its map).
- **Evidence:**
  - Tests: model 30 (the ports' own included), the file adapter 7 (5 read the Python fixture: facts, tracks, `check`,
    merging and refusing version 2, a file written again), the app's 20 (one lists the fixture and builds every stage
    through the data thread) and its 2 standalone checks, the settings' 2, the scene's 21; Clippy clean.
  - The fixture through the binary: `check` reports no problems and its feature table; `--cameras` and
    `--render --throw` read it.
  - **Parity with show-me-the-nades**, on its export of an APFS copy of `nades.duckdb` (247 demos, 116,480 throws,
    2.47 GB, `check`: no problems). The exporter was the in-progress `nades export --cs2-throws` of another session's
    uncommitted worktree, run read-only; the main checkout had none yet. `tag_parity` and `bench/cameras.py`:
    - **technique**: 111,654 throws the same, none different; 4,826 lack the release facts on both sides.
    - **landing callouts**: 114,542 the same (98.3%), 1,140 different, 784 missing on both, 14 on one side. From
      show-me-the-nades' own `place_cells` the sandbox's lookup gives 116,241 of 116,480 (99.8%): the rule holds, and
      the difference is the samples. Its cells count every living pawn every 32 ticks, warm-up and freeze time
      included; the file carries live-round samples twice a second.
    - **groups**: 32,688 of ours against 32,682 of theirs; 23,100 agree exactly (41,727 throws), and 85.5% of throws
      carry their group's commonest `nades.lineup`. Replaying show-me-the-nades' own anchors (its `lineups` table, in
      creation order) over the file's facts with the sandbox's rule gives all 115,434 tagged throws their own lineup:
      the rule and the facts are exact, and the difference is the order groups start in. Its registry keeps anchors
      across ingest runs, and 34 of the 247 demos were ingested out of match order; §4 takes `played_at`. The 3 demos
      without rounds add 599 grouped throws it leaves untagged.
    - **cameras**: 84 throws on 7 maps, all four views identical to the web view's (0.0000 units): 11 flash victims
      placed from samples, 36 opponents, 37 overviews (`.runs/cameras-20261026-190548`).
  - **Costs** on that file: a map's first list 0.25–0.37 s (reading 180–275 ms, the rules 66–110 ms), off the first
    frame's path; every filter change after it 6–12 ms in memory, where each was a 93–114 ms DuckDB read before (on
    ≈96 demos). Three runs each, load average 5–14.
- **Not done:** the demo adapter (§6, step 2) and maps from the game's files (§7, step 3); labelled stand-ins for a smoke
  without voxels or a fire without flames (§2: today such a stage says what is missing); players across name changes
  by Steam id; the MCP `lineups_list`, which exists only in uncommitted work at the time of writing.

## S25 · cs2-throws 2.0: integer keys inside the file

- **Decision:** inside one file, rows refer to a demo, a player, a place and a throw by small integer keys
  (`demos.demo`, `players.player` per demo, `places.place`, `throws.throw`); `demo_id` and `throw_id` stay the ids
  across files, in the API and in merging. `player_samples` and `flames` are `without rowid` tables whose primary keys
  are their only index; tags split into `demo_tags` and `throw_tags`. Writers set `page_size = 16384`. The reader takes
  2.x and refuses 1.x, saying to write the file again. Version 1 was days old with no outside users.
- **Why:** version 1 of the 247-demo export was 2.47 GB, 77% of it `player_samples` (1,280 MB) and its index (611 MB):
  each of 7.78 M rows, and each index entry, repeated the 64-character `demo_id`, the player's name and Steam id, and
  the place name.
- **Measured** on version 1's own rows, re-keyed with SQL (same 247 demos; ≈ rows are version 1's other tables plus
  the measured samples):
  | Layout | File |
  |---|---:|
  | version 1 | 2,467 MB |
  | integer demo and player keys, `without rowid` samples, place as text (samples alone: 622 MB) | ≈1,200 MB |
  | the same with integer place keys (samples: 564 MB) | ≈1,140 MB |
  | also integer throw keys for flames, blinds, voxels and tags; tags split | 911 MB |
  | the same, 16 KB pages (voxel logs 143 → 97 MB) | 831 MB |
  | the same, 64 KB pages | 819 MB |
  16 KB pages take most of the gain without making a point read (one throw's tracks) read 64 KB. show-me-the-nades'
  export inserts row by row, which leaves `without rowid` pages about two thirds full; it builds into a scratch file and
  `vacuum into` the final one (0.9 s), so its file is 830 MB, the same bytes on every run, in 9.5 s (version 1: 11.6 s).
- **Parity** on that export: every fact equal to version 1's file (each table rebuilt through the keys and compared
  with `except` both ways, 7.78 M samples included); `check`'s census identical; `tag_parity`'s tallies identical
  (techniques 111,654 the same and none different, callouts 98.3%); `bench/cameras.py` 84/84 in every view
  (`.runs/cameras-20261026-221946`).
- **Read cost:** a map's facts read in 137–207 ms on the large maps against 165–252 ms from version 1 (three runs,
  median, load average 8–14). Every map query starts from `demos` with `cross join`: left to itself SQLite scanned all
  430 k throw tags to skip a sort, and de_train's read went from 4 to 35 ms.

||||||| a38394e

## S26 · Maps from the game's files: the collider, the effects and the records, made by the packer

- **Decision:** building a pack reads the game's own files through Source 2 Viewer's CLI, which the packer runs itself
  (docs/standalone-api.md §7, step 3), and nothing show-me-the-nades' scripts make except the geometry export:
  - **The collider** (`crates/packer/src/physics.rs`): the CLI's glTF export of `maps/<map>/world_physics.vmdl_c`,
    reduced exactly as awpy-data's `process_geometry.py` reduces it into the mesh the web view's line of sight reads:
    meshes named for a non-occluder dropped (`playerclip`, `grenadeclip`, `sky`, `passbullets`, `glass`, `window`), the
    positions welded by their bits in export order, then clustered on an 8-unit grid (each cell's first vertex stands
    for it; collapsed and repeated triangles go). It goes into the pack's Collider section in awpy's AWMH layout, and
    the app reads it from the pack (`crates/sandbox/src/assets.rs`). The export needs no `--gltf_export_materials`: the
    meshes carry the same names, and the flag makes the master build load the game's shaders, which a depot download
    of a few maps does not have.
  - **The effects** (`crates/packer/src/effects.rs`, `nades-pack --effects`): one shared pack,
    `<packs>/shared/effects.pack` (`SectionKind::Effects`, `nades_pack::effects::Effects`): the smoke noise read from
    `pak01` (`worley_perlin`, 128³ RGBA8, as R + 0.95 G in f32), and the four fire flipbooks decompiled by the CLI into
    their sheets and frames and laid out as `cs2_effects.py web` lays them, each frame resized by a port of Pillow's
    Lanczos filter (premultiplied, 22-bit fixed point, 16-bit frames to their high byte).
  - **The records** (`crates/packer/src/lump.rs`): the map's and its 3D skybox prefab's entity lumps, the sky
    materials and the `.vpost` with its lookup table, decompiled per build into a scratch folder; from them the
    post-processing, sky and skybox records by `export_map.py`'s own rules, ported line for line (its blocks, its
    `keyvalue`, its number regex, which reads `1e-05` as 1 and −5, kept so the packs stay the same).
  - **The textures** come from `pak01` alone: the game index finds a base colour by name among `pak01`'s entries that
    the depot holds, as it found it among `export_map.py`'s unpacked files (`--unpacked` still adds such a folder).
  - **Settings** (§8, `crates/config`): `--packs`/`NADES_SANDBOX_PACKS`/`[assets] packs` (default `packs/` in the
    platform's cache), `--game`/`NADES_SANDBOX_GAME`/`[assets] game` (the folder holding `game/csgo`; no default),
    `--s2v`/`NADES_SANDBOX_S2V`/`[tools] s2v` (no default). This machine's config file names the old folders under
    `~/.cache/nades`. The MCP server takes its packs folder from the same setting and passes it on.
- **Why:** a pack for any map the game has, including those awpy does not publish, from one tool anyone can download,
  with no other project's scripts, folders or Python environment.
- **Evidence** (2026-10-26, game build 2347770_67499503218971311, Source 2 Viewer 20.1 master):
  - **Collider:** byte for byte awpy-data's published `<map>.mesh` on all 7 pool maps (`examples/physics_vs_awpy`;
    e.g. de_cache 1,632,087 exported triangles, 368,176 dropped, 1,263,911 welded, 338,375 kept; de_inferno 547,966 of
    2,733,603). `bench/cameras.py`: 84 / 84 throws, every view identical to the web view's
    (`.runs/cameras-20261026-223620`), so no result camera changes.
  - **Effects:** the noise and all four atlases byte for byte `cs2_effects.py web`'s files (`examples/effects_vs_web`).
    Source 2 Viewer's release build (`tools/s2v`) decompiles the sheets differently (frames cropped to their alpha,
    unnamed sequences dropped: 72 frames of fire_small_sim_a against 73), so the master build is the one to use.
  - **Records:** the entity lumps, `post.json`, `lut.bin`, `sky.json` and `skybox.json` the same as `export_map.py`'s
    on all 7 maps (`examples/lump_vs_export`); 4 CLI runs a map, 0.8–1.8 s.
  - **Packs:** every pool map rebuilt from `pak01` without the unpacked folder: the scene, the Geometry and the
    Textures sections identical to the packs before (`examples/pack_diff`), the Collider section new. 2.4–6.3 s a map
    (the collider 0.4–2.2 s of it); the effects pack 2.6 s, 32.7 MB.
- **What remains:** the geometry and materials (`<map>.glb`, `<map>.skybox.glb`): `export_map.py` runs the CLI's glTF
  export and then compresses it (meshopt, WebP) and adds layer averages; the packer decodes that compressed form. Running
  the export itself needs the packer to read an uncompressed glTF and compute the averages, the next step. The sky
  record differs in one point: `export_map.py` wrote it only once the sky texture decompiled to EXR; the packer needs
  the texture in an archive the depot holds.

## S27 · The timed benches stop when the window is not on screen

- **Decision:** `--bench-frames`, `--bench-maps` and `--bench-switch` end with an error that says the window "needs
  the window on screen" when it gives no frame to draw into for 5 s, or macOS says it is hidden or covered for 5 s
  (from launch until it first shows it, too); `--bench-frames` and `--bench-maps` end at once when macOS hides the window
  while they measure. `--bench-switch` with an empty list (no data, or a filter matching none) is an error as soon as
  the list arrives. `bench/startup.py` reads that error as it read its 120 s timeout (`timed_out`).
- **Why:** a hidden window's drawables come from the window server about once a second, so a bench either never ended
  (`--bench-frames`: 600 frames at 1 Hz) or recorded the wait as a frame (`--bench-maps`: a 1,004 ms frame), and the
  switch bench waited for a list it would never have (the bench-switch fix's follow-ups, 2026-10-26).
- **Evidence** (screen locked, so every window hidden; watchdog caps 40 s, 5 GB): the new binary exits 1 after 5.1–5.5 s
  for `--bench-frames 200`, `--bench-maps de_mirage,de_cache` and `--bench-switch 20`, and after 0.11 s for
  `--bench-switch 20` on an empty data folder; main's binary ran `--bench-frames` and `--bench-maps` until killed at
  15 s. Not run tonight: the same benches with a window on screen (the screen stayed locked).

## S28 · The demo reader: awpy into the cache, one file per demo

- **Decision:** `crates/source-demo` reads a CS2 `.dem` with awpy 3.1 and writes one cs2-throws 2.0 file per demo into
  the cache (`throws/<sha256>.sqlite` in the platform's cache folder), which the file adapter then reads. It is a port
  of show-me-the-nades' `extract.rs` and `tracks.rs` with only the facts kept (no kills, damage, physics or labels): one
  filtered entity pass for projectiles (`m_vInitialPosition`, `m_vInitialVelocity`, `m_hThrower`, `m_bIsIncGrenade`),
  the thrower's pawn at the projectile's first tick (origin, eye angles, velocity, ground, duck, walking, place, and the
  last origin on the ground as the take-off point), the flight every 2 ticks while it moves, the smoke's voxel log, the
  inferno's flames, team names and samples every 32 ticks; then awpy's rounds, bomb and blind events, smoke and fire
  windows and detonate events. Fires are matched to their molotov by place and time (within 250 units, starting at
  most 32 ticks after the projectile went). The writer is the file adapter's own `SCHEMA` (a test keeps it equal to §3
  and `tools/cs2_throws.py`). Ids are §3's recommendation; `played_at` is the `.dem` file's modification time, which
  archive tools keep. `meta.producer` names the reader's revision, and a cached file from another revision is parsed
  again; the same revision never parses a demo twice (its SHA-256 names the file).
- **The app:** `--data` takes demos and folders of them (`nades_config::import_demos`, before the source opens);
  dropping a demo, a cs2-throws file or a folder onto the window imports it on a thread of its own
  (`crates/sandbox/src/imports.rs`), shows "Reading x.dem… 45%" in the right panel and adds the file to the open source
  (`FileSource::add`), whose revision change makes the window read its lists again at once.
- **Evidence** (`.runs/demo-parity-20261026-2310`): the two ESL Pro League S24 archives from `~/.cache/nades/hltv`,
  unpacked into a temp folder (the archives' SHA-256 unchanged after), six demos. The reference is show-me-the-nades main
  `87e338a` in a temp clone, ingesting the archives into a throwaway database and exporting it.
  - **Row for row, keys resolved to values:** demos 6, rounds 130, throws 2,709 (every column, flights byte for byte),
    smoke voxel logs 811, flames 9,954, blinds 1,481, player samples 194,981: all present on both sides, equal except
    two columns the reference never stores. Blinds' positions: ours are awpy's `victim_x/y/z` (1,378), null for the
    103 victims on no side (spectators, whom awpy puts at the world's origin); they lie a median 11 units (at most 74)
    from the victim's own sample within 16 ticks. Samples' duck amount: ours is `m_flDuckAmount` (0 in 87%, 1 in 8%).
  - `nades-sandbox check`: no problems on each file. The Result view of a Cache smoke rendered with `--data <the .dem>`
    is the same image as from the reference export (`--render --throw … --view result`; repeated renders of one input
    differ in 11 of 2.2 M bytes, GPU noise).
  - **Cost:** 3.6–5.2 s a demo (six in 25.5 s, one at a time, load average ≈13), 668 MB peak; a demo already in the
    cache 0.5 s (its SHA-256). 3.4–4.0 MB a file. Two imports from scratch give the same bytes.
- **Not checked:** dragging a demo onto the window by hand (the screen was locked; `imports.rs`'s test drives the same
  path without a window). Demos other than HLTV's pro matches (Premier, FACEIT) are untested.

## S29 · The geometry export, made by the packer

- **Decision:** `nades-pack <map>` makes the map's geometry export itself (`crates/packer/src/export.rs`), step for
  step as show-me-the-nades' `export_map.py` made it, and `nades-pack setup <map>` fetches a map's files
  (`setup.rs`). What remains of show-me-the-nades in a pack's inputs is nothing; three public tools remain (Source 2
  Viewer's CLI, Bun for glTF-Transform 4.5.1, DepotDownloader).
  - **Stage:** a folder of links: `gameinfo.gi` (a copy), the maps' VPKs, `pak01_dir.vpk` with the archives the game
    folder holds, `game/core`'s files, and no `shaders_vulkan_*` (core's or csgo's). The exports were made before the
    shader directory reached the depot (2026-10-25 19:14, after the last export at 14:54, for `cs2_effects.py`); with
    the directory and not its archives the CLI fails on maps whose materials ask a shader (Nuke: no glTF at all).
  - **Export:** `export_map.py`'s arguments (`-d --gltf_export_format gltf --gltf_export_materials
    --gltf_textures_adapt --gltf_export_extras --game … --threads 8`). The CLI exits 2 on its side export of the
    physics mesh, which wants the shaders; the map's glTF is what is read.
  - **Layers:** `layer_textures` ported line for line, with its Pillow resizes (`resample.rs`: box and bilinear beside
    the Lanczos of S26, and Pillow 13's pass order: vertical first when the height shrinks by more than twice what the
    width does; without it 4 of 7 maps' averages differed in the 4th place), SHA-1 names and the glTF edited with its
    key order kept (`ojson.rs`), since glTF-Transform keeps `extras` as they come.
  - **Compress:** `bunx --bun @gltf-transform/cli@4.5.1 optimize … --texture-compress webp --texture-size 1024`, then
    `meshopt --quantize-texcoord 14 --quantize-generic 16`.
  - **Setup:** DepotDownloader, anonymous, app 730, depot 2347770, `-manifest` pinned to the one the game folder holds
    (the current one, 67499503218971311, at the time), so fetched files never mix builds. With `pak01` mounted, the CLI
    passes over a file whose archive is missing in silence (de_mirage without `pak01_013`: 620 files written of 626,
    no message), so setup exports without it, as `export_map.py` did, fetches the archives holding what the CLI could
    not load, unpacks those files into its stage and repeats (at most 8 rounds); then the layer textures', the sky's,
    the post-processing's and the effects' archives.
- **Rejected for now:** reading the meshes from the VPKs directly. Source 2 Viewer's export is deterministic (the same
  glTF and bin, byte for byte, as export_map.py's of the day before) and takes 0.7–7 s a map; a reader of CS2's
  meshes, materials and world nodes would have to reproduce glTF-Transform's welding, instancing, joining and
  quantization to keep the packs the same.
- **Evidence** (2026-10-26):
  - All 7 pool maps: `<map>.glb` and `<map>.skybox.glb` byte for byte `export_map.py`'s, and the packs byte for byte
    the installed ones (`cmp`). A pack takes 14 s (Mirage) to 3.4 min (Cache: 1,015 materials, the layer step's
    decompiles and resizes), most of it the layer step and glTF-Transform.
  - de_vertigo, never exported before: `nades-pack setup de_vertigo` fetched its VPK, its 3D skybox prefab
    (`prefabs/de_vertigo/skybox2`) and one `pak01` archive (102) in 42 s, export rounds 189 → 407 → 33 → 0 files the
    CLI could not load; `nades-pack de_vertigo` built a 450 MB pack in 14 s with nothing missing (179 base colours by
    name, 170 material textures from the game, sky, post-processing, 69 probe volumes, collider of 66,048 triangles),
    and it renders offscreen (`--render` from both spawns).
  - `bench/realism.py --note "geometry export made by nades-pack (U3b)"` on the rebuilt packs: headline 5.3 / 10.3 /
    6.2, as before; 5 of 7 renders byte for byte the last run's (`.runs/realism-20261026-232937`).
||||||| dd73944
