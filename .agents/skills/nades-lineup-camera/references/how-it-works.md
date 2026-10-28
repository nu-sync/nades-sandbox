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
   RGB24 into `ffmpeg -f rawvideo … -c:v libx264`; it reads progress, each `ThrowReport`, the `camera_audit` and the
   `video_plan` (the frame count, before the first frame) from the renderer's stderr. ffmpeg starts only once the plan
   is in: CRF 20 with `-maxrate` from `max_megabytes` (default 25) over the clip's length (`bitrate_cap`), so a 27.5 s
   1080p60 smoke is about 20 MB. It renames the MP4 to `output` and writes `<output>.json` (with `camera` and
   `megabytes`).
4. `crates/sandbox/src/offscreen.rs` `render_video`: stages the map, effects (noise, flipbooks, grenade models) and the
   collider; for a shot also the **sight** collider (`assets::sight`: every opaque or cut-out triangle the pack draws,
   about 1.3M on Mirage, 0.1 s), which the shot's camera is planned against (S36); simulates the throw
   (`sim::simulate`: physics then effects); builds `shot::Shot` and prints its audit; per frame asks the shot
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
  ceiling); eye and look Gaussian-smoothed (σ 0.08 s, 0.14 s); it leaves the thrower's eye over `LEAVE` 0.5 s. Round
  a corner the eye closes in (`CLOSER`: 1 down to 0.15 of the lag) until `chase_fits` (10 units from every surface,
  the grenade in sight); where smoothing cuts a corner the sample falls back to the path, else is `nudge`d off the wall.
- **Turn** (`Circle::new`, S36): radius 400 (smoke, clear 100 of every voxel), 320 (fire, clear 40), 360 (else). Each
  of 72 bearings × 10 heights (`HEIGHTS`, 10–76° at the radius) gets the furthest distance (`PULLS`, 1 down to 0.3 of
  the radius) where `eye_fits`: map under it, 24 units from every surface, `clear` of the effect, and the effect in
  sight (`sees_effect`: its middle, or two of its `effect_marks`, 70% out to its extremes). The turn is a search over
  those places, a bearing at a time, at most one height step between bearings, and every move between neighbours
  checked at 4 points: the cheapest whole circle up to 42° (`LOW_TOP`), else the longest arc that low if it turns
  90° or more (`MIN_ARC`), else any whole circle, else the longest arc. The camera moves evenly along it (`Sweep`), from
  the bearing nearest where the chase ended that it reaches in a line, or from the arc's nearer end.
- **Pull** (`Pull`): from the chase's end to the turn's start, straight if a tube of 12 is clear, else up then across,
  else a cut.
- **Last pass** (`clear_of_near`): every pose from the release on, 120 a second, is pushed off any surface its near
  plane (4 units, `near_corners`) or 10 units round it would meet; the shifts are eased and kept at least as large.
- **Audit** (`Sequence::audit`, `Shot::audit`): every frame from the release on, at the video's fps: `clipped` where
  a ray to the near plane's middle or a corner meets what the map draws; `hidden` where the grenade (chase) or the
  effect (`sees_effect`) is out of sight. It goes in the render's JSON (`camera`) and the `lineup_render` summary.
  `cargo test --release -p nades-sandbox lineup_probe -- --ignored --nocapture` audits a set of lineups without
  rendering (`NADES_PROBE_CASES` for a JSON-lines file of others).

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
