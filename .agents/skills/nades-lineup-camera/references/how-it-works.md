# How the lineup camera works

## Path of one call

1. `crates/mcp/src/lib.rs` `lineup_render` → `nades_control::scene::LineupInput::throw()`:
   - `parse_console` scans the words for `setpos`/`setpos_exact` (three numbers) and `setang`/`setang_exact` (two;
     a third, the roll, ignored), splitting on whitespace, `;`, `,` and quotes;
   - `Grenade::parse` (smoke, flash/flashbang, he/frag, molotov/molly, incendiary/inc, decoy);
   - `Technique::parse` over the `technique` text and the lineup's own non-numeric words; fields override. "W" or
     "forward" with a jump-throw and no "run" is a jump-throw with `speed` 30 (CS2's air cap from a standstill, S35).
   - The map gets `de_` if it has no prefix.
2. It builds a `VideoRequest` with `shot: [ShotMove::Lineup { throw: 0, seconds }]`, five `stills` at moments
   (aim, aim_point, flight, bloom, result), 1920×1080 at 60 fps, `output` (default the MCP's runs folder), then
   `Backend::video_start` and polls until done (900 s at most).
3. `crates/mcp/src/video.rs` runs `nades-sandbox --render-video request.json` with `NADES_RAW_FRAMES=1`, piping raw
   RGB24 into `ffmpeg -f rawvideo … -c:v libx264`; it reads progress and each `ThrowReport` from the renderer's
   stderr, renames the MP4 to `output` and writes `<output>.json`.
4. `crates/sandbox/src/offscreen.rs` `render_video`: stages the map, effects (noise, flipbooks, grenade models) and the
   collider; simulates the throw (`sim::simulate`: physics then effects); builds `shot::Shot`; per frame asks the shot
   for the camera, the scene's time and whether the crosshair shows; draws (4 jittered samples for a smoke frame);
   reads back RGB; paints the crosshair on the CPU (`offscreen::crosshair`, sized to the frame); writes stills on
   their frames; streams the frame.

## The sequence (`crates/sandbox/src/lineup.rs`, `Sequence`)

| Phase | Base seconds | Camera |
|---|---|---|
| spot | 2.0 | thrower's eye, pitch at least 28° down, FOV 100° |
| aim | 2.0 | eases to the setang and CS2's 73.74° in 0.9 s |
| aim point | 2.2 | zooms to 28° (`AIM_FOV`) in 0.7 s |
| throw | 1.6 | back to 73.74° in 0.5 s; release at its end |
| chase | the flight's real airtime | flight.ts's chase (below) |
| bloom | smoke 4.5, fire 2.5, else 1.5 | pulls out to the turn's start in 1.3 s, turning to the effect in 0.9 s |
| turn | smoke 8, fire lasts−3.5 (4–8), else 5 | one turn round the effect, clockwise, eased |

- A `seconds` length scales every phase but the chase (factor clamped 0.3–3).
- The scene's clock is held at the throw's start until the release falls at the throw phase's end, then runs 1:1
  (`scene_time`). The crosshair shows while the video time is before the release (`crosshair`).
- Moments for stills: `Sequence::moment`.

The chase and the turn port show-me-the-nades' `web/app/src/scene/flight.ts` (its D29):

- **Chase:** sampled 120 a second from the release to the detonation; the eye rides the grenade's own path by arc length,
  `LAG` 70 units behind, easing to `SETTLED_LAG` 150 over the last `SETTLE` 0.6 s, lifted `LIFT` 12 (less under a
  ceiling); eye and look Gaussian-smoothed (σ 0.08 s, 0.14 s); it leaves the thrower's eye over `LEAVE` 0.5 s.
- **Turn circle** (`Circle::new`): radius 400 (smoke, clear 100 of every voxel), 320 (fire, clear 40), 360 (else); at
  each of 72 bearings the lowest of five elevations (18–68°) clear of the map to that distance (+24) and with open sky
  300 units above (ours, for rooms the physics leaves open), else the roomiest nearer place ≥ 150, else through the
  walls with the map under it; neighbours share the nearest/highest choice (`spread`), eased 1-2-3-4-3-2-1; bearings
  pulled into the effect go back out; `Sweep` lingers where the effect shows (`HURRY` 0.4 elsewhere); it starts on
  the shown bearing nearest where the chase ended.

## Pieces it relies on

- `nades_scene::stage::Flight::grenade_at`: where the grenade is and its spin (end over end, rolling on floors, and
  settling onto its side below 40 units a second, `lying`). Hidden within 48 units of the hand and once it goes off.
- `nades_render::throw::ThrowLayer`: draws the game's model per grenade (`set_flipbooks` makes the parts' bind
  groups; `set_moment` adds instances; `throw.wgsl` `model_vertex`/`model`), else a capsule; arcs only when asked.
- Models come from `nades-pack --effects` (`crates/packer/src/grenades.rs`: Source 2 Viewer exports each
  `weapons/models/grenade/*/weapon_*.vmdl_c` to glb; bind pose, node transform, metres→inches, glTF y-up→z-up,
  base colour cut to 256).
- Other shots (`ShotMove::Follow/Orbit/Thrower/Hold`, `crates/sandbox/src/shot.rs`) are for `video_render`.

Decision records: `docs/decisions.md` S31 and S34.
