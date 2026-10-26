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
