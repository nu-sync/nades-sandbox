# M1 bake-off: one goal, two designs

Opened 2026-10-26 at the user's request: "can we just build both of them and then compare them? You can set two
different agents off to achieve the same goal with the two different designs." This file is the contract both builds
follow and the rule that picks between them. The rule was fixed before either build started.

## The goal (M1)

de_cache, textured and lit by its baked lightmap, drawn in a window, with a fly camera and the egui shell, on screen
as soon after the process starts as each design can manage.

## The phases

1. **Pack (agent P, first, alone).** P builds the real pack for de_cache in format 1 (`crates/pack`), with the scene
   description both builds read, and the VPK reader that takes the lightmap out of the map's archive
   (`crates/source2`). When P is done, `crates/pack` and `crates/source2` are frozen. If a build needs something
   from them, it implements it in its own code or asks the coordinator.
2. **Two builds (agents A and B, in parallel, same brief, same time budget):**
   - **A, our own renderer:** wgpu 30.0.1 + winit 0.30.13 + egui 0.37, Rust 1.95, in `crates/render` and `crates/sandbox`.
   - **B, Bevy:** Bevy 0.20 + bevy_egui 0.43.1, Rust 1.99.1, in `apps/bevy`, its own cargo workspace with
     `rust-toolchain.toml`, reading the pack through a path dependency on `crates/pack`.
3. **Comparison (the coordinator):** the same machine, alternated runs, the rule below.

## Who writes where

Concurrent writers never share a file:

| Writer | Writes | Reads |
|---|---|---|
| P | `crates/pack`, `crates/source2`, `crates/packer` (the `nades-pack` command), the root `Cargo.toml` and `Cargo.lock`, `~/.cache/nades/packs/` | the export, the game's files, `prototypes/wgpu-spike` |
| A | `crates/render`, `crates/sandbox`, the root `Cargo.toml` and `Cargo.lock` (dependencies only), `.runs/m1-a/` | the pack, `crates/pack`, the spike |
| B | `apps/bevy/**`, `.runs/m1-b/` | the pack, `crates/pack`, the spike, `prototypes/engine-bakeoff/bevy` |

Nobody edits `SPEC.md`, `docs/` or `bench/` but the coordinator. Nothing is committed.

## The contract both builds meet

- **Input:** `~/.cache/nades/packs/de_cache.pack`, read through `nades_pack::Pack` (memory-mapped), with every draw it
  describes drawn. The pack is the only map input at run time: no glb, no game files.
  - Each design may upload it however it can fastest: B may bypass Bevy's asset server and create GPU resources in
    its render world.
  - Neither may decode, transcode or re-encode texture data at run time; BC blocks go to the GPU as stored.
- **Window:** 1440×900 logical, at the display's own scale factor, titled `nades sandbox (A)` or `nades sandbox (B)`.
- **Camera:** `bench/camera.json` (glTF space, metres, y up; vertical field of view 73.74°; near 0.1016, far 762) at
  start-up. Then a fly camera: WASD to move, the right mouse button held to look.
- **Shading, the same in both:**
  - The output colour is the base colour (its texture, sRGB-decoded, times the material's colour factor) times the
    irradiance lightmap (linear; its BC6H texels as stored) for draws that have lightmap coordinates, and times a
    constant 0.6 for draws that have none.
  - No tone mapping, bloom, fog or MSAA, and exposure 1.
  - The swap chain is sRGB, so the hardware encodes.
  - Alpha-test materials discard below their cutoff; blended materials draw after the opaque ones, alpha-blended.
  - Back faces are culled unless a material is double-sided.
- **Sampling, the same in both:**
  - Base colour: repeat, linear magnification and minification, linear between mips, no anisotropic filtering.
  - Lightmap: clamp to edge, linear, linear between its two mips.
- **What the pack says:** `crates/pack/src/scene.rs` documents every field.
  - Opaque materials ignore their textures' alpha.
  - A material with no base colour texture samples white.
  - Draws come in their drawing order (opaque and alpha-test, then blended).
  - Front faces are counter-clockwise.
- **Lightmap coordinates:** TEXCOORD_2 when present, otherwise TEXCOORD_1 when every coordinate lies in [0, 1], as
  show-me-the-nades' web view decides (`mapModel.ts` `lightmapCoordinates`). v = 0 is the texture's first row. P
  records which draws have them.
- **The shell:** an egui left panel, 300 points wide, showing the start-up marks and the draw count.
- **Flags:**
  - `--exit-after-first-frame` prints one JSON line on stdout, `{"pre_main_ms":…,"marks":[[stage,ms since main],…]}`,
    then exits. The last mark is `frame:presented`: the first frame with every draw of the map submitted, finished
    on the GPU (wait for it) and presented. A `window:visible` mark records the first time macOS reports the window
    visible: winit's `WindowEvent::Occluded(false)` (Bevy: `WindowOccluded`), or the `NSWindow`'s occlusion state.
    The run exits only once both marks are in. *Amended during phase 2; see Progress.*
  - `--screenshot <file.png>` writes the first such frame, from the bench camera, as an 8-bit sRGB PNG at the
    window's physical size, then exits.
  - `--bench-frames <n>` draws n frames from the bench camera with vsync off. It prints
    `{"frames":n,"median_ms":…,"p95_ms":…}` (wall time per frame after the first 30) and exits.
- **Quality bar:** the design's own tests and `cargo clippy --release --all-targets` pass. No Valve-derived file
  enters the repository: packs stay in `~/.cache/nades/packs`.

## The selection rule

1. **Gate.** The coordinator checks every point of the contract. Both builds' bench-camera screenshots must match:
   a mean absolute difference of at most 4/255 per channel, and no visible structural difference. A build that
   fails the gate loses, unless both fail; then the one closer to passing gets one fix round.
2. **First frame decides.** The primary metric is process start to the later of `frame:presented` and
   `window:visible`, when the map can first be on screen. It is the median of 7 alternated
   warm runs, run by the coordinator (one throwaway launch per new binary first; the load average recorded). A
   design wins if its median is at least 10% lower and it is lower in at least 6 of the 7 pairs, unless its steady
   frame time (the `--bench-frames 600` median, 3 runs) or its peak memory (`/usr/bin/time -l`) is more than 25%
   worse than the other's. That case, and any result short of the 10% / 6-of-7 bar, goes to step 3.
3. **Tie-breaks, in order:**
   - the lower steady frame time, if at least 10% lower;
   - otherwise the smaller design-specific code (lines of Rust and WGSL written for the runtime, excluding the shared
     crates);
   - otherwise the faster clean release build.

Recorded beside the result, outside the rule: compile time, binary size, dependency count, and how each design would
carry M2 and M3 (CS2 materials, the smoke raymarch, the result camera), with any friction a build met.

## Progress

- 2026-10-26: the contract and the rule fixed. `bench/compare.py` applies the rule and writes the evidence to
  `.runs/m1-compare-<time>/`. It was dry-run on the M0 and Bevy shells: the plumbing works, and the pair bar was made
  proportional to the run count. Phase P (the pack) started.
- Phase P done.
  - The pack: `~/.cache/nades/packs/de_cache.pack`, 346.4 MB (geometry 95 MB, textures 251 MB) holding:
    - 1,276 draws, 991 materials and 159 textures;
    - all 158 base colour images from the game's own BC blocks, found by name;
    - the BC6H irradiance (8192² plus 4096²).
  - Built by `nades-pack de_cache` in 0.45 s, byte-identical on rebuild.
  - The coordinator re-ran the checks: 30 tests and the 4 real-data checks pass, and clippy is clean. The checks:
    - texture blocks against the export's images;
    - the lightmap decoded bit-exact against the export's EXR;
    - lightmap orientation by seam continuity;
    - the whole map validated.
  - `crates/pack` and `crates/source2` are frozen. Phase 2 (A and B) started.
- Amendment, before B finished. Agent A reported that it presents its first frame before macOS reports the window
  visible: the window appears with the map already in it, and its pixels reach the screen only when the window
  shows, ≈30 ms after `frame:presented`. wgpu's own path waits for visibility before presenting. So the two builds'
  `frame:presented` marks did not measure the same event.
  - The contract's goal is the map on screen, so the metric is now the later of `frame:presented` and a
    `window:visible` mark.
  - Presenting before visibility stays allowed: it spares the user a blank window. It no longer counts as time on
    screen.
  - Both builders were told the same change.
- Phase 2 done.
  - Both builds meet the contract. The coordinator checked:
    - A: 33 tests, clippy clean;
    - B: 9 tests on Rust 1.99.1, clippy clean;
    - all 1,276 draws drawn by each, no runtime decode;
    - the three flags and the `window:visible` mark in both.
  - The designs as built:
    - A drew the pack from GPU buffers made over the pack's own memory-mapped pages (no copy) and presents its first
      frame before the window is visible;
    - B hosts the same kind of raw-wgpu map renderer inside Bevy (device handed to Bevy at start, pipelined upload,
      one render bundle replayed in a Bevy pass), with Bevy running the window, input, egui and the frame loop.
- The comparison, by the coordinator: `uv run bench/compare.py`, after all agents had stopped, load 4.3 → 13.0. The
  evidence is in `.runs/m1-compare-20261026-111011/`.

  | Step | A: own renderer | B: Bevy + our renderer | Outcome |
  |---|---:|---:|---|
  | Gate: screenshots | | | match: mean difference 1.27/255; the 3D views are identical, only the panels' text differs |
  | Map on screen, median of 7 alternated | 111.3 ms | 123.0 ms | A 9.5% lower in 5 of 7 pairs: under the 10% / 6-of-7 bar, so no win |
  | Steady frame, median of 3 | 2.40 ms | 2.51 ms | within 10% |
  | Design-specific code | 1,249 lines | 1,096 lines | **B wins the tie-break** |

  - **What the numbers are made of:**
    - A presents the map at a median of 88 ms, but macOS shows its window at 111 ms, so A's time is AppKit's.
    - B's window is visible at 110 ms, the same as A's. B's first complete frame follows at 123 ms: Bevy's first
      update and extract come after the window.
    - So A sits at the floor macOS sets, and B is ≈12 ms above it.
  - **Recorded beside the result, outside the rule:**
    - clean release build: A 17.8 s, B 51.7 s;
    - binary: A 10.3 MB, B 59.6 MB;
    - peak RSS: A 88 MB, B 555 MB. Most of the gap is A's buffers over the mapping, which RSS does not count: the
      agents' peak physical footprints were A 720–755 MB, B 896 MB;
    - B's 301 crates against A's far fewer;
    - B's egui is one version behind (0.36), and Bevy's API changes every release;
    - both agents found that M2 and M3 rendering (CS2 materials, smoke, result camera) would be our own wgpu code in
      either design.
  - **The rule's verdict: B,** on design-specific code. It goes to the user with the factors outside the rule (decision S7).
- **Decided: A,** chosen by the user over the rule's pick (decision S7).
  - B's build moved to `prototypes/m1-bevy`, its own workspace, still runnable.
  - `crates/render` and `crates/sandbox` are now M1 as built by A.
