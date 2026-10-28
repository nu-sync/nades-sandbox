# Native application MCP v1

## Outcome and boundaries

An agent discovers thirteen typed tools, understands the currently shown map and camera, operates its own window,
checks actual results, and renders scenes headlessly. It can read bounded logs and correctly labelled frame metrics
without shell automation. The MCP adapter calls the same application commands as the UI. It is not a second renderer.

The server owns one window. Another MCP session and manually opened windows are independent. Switching maps may
restart the owned window; this is explicitly reported. A ready window survives MCP disconnect and becomes the user's
normal window. It cannot be reclaimed by later MCP sessions. Offscreen workers never become retained windows.

Live M3 throw selection and playback; M4 resident map switching; semantic location navigation, picking, visibility,
saved views and batch rendering are follow-up capabilities. V1 does not advertise them. Database writes, arbitrary
SQL, shell execution and raw game/pack exports are excluded. Assets and the database stay local.

## Architecture

`MCP client → nades-mcp (stdio) → private child pipes → typed commands → winit event loop`.

The compiled `nades-mcp` launches its sibling `nades-sandbox` directly, with an explicit binary override for development.
MCP stdout is exclusively protocol output. Each sandbox starts with `--control-stdio --instance-id ID --map MAP`;
its private stdout carries versioned JSONL ready/reply messages, its stdin carries correlated requests, and stderr
goes to a local file that remains valid after handoff. No socket, port, global process search or manual attach exists.
Production logs and private remembered view state default to `~/.cache/nades/sandbox/mcp`, independently of the MCP
client's working directory; `--runs-dir` overrides this for development and verification.
`nades-control` contains transport-independent domain and IPC types, with no MCP, window or GPU dependency.

The event loop owns the camera, window, renderer, revision and frame identity. IPC readers dispatch user events;
they never hold a mutex around the view. Window mutations/captures are serialized. Status remains available during
startup and reports `starting`, not invented readiness. Offscreen renders use isolated workers and unique temporary
outputs, with one worker active per server. The headless boundary uses the same camera contract and rendering path;
a persistent headless runtime can later replace the worker without changing the tool's meaning.

## Agent entry point

The person talks to an agent; the agent drives the simulator through these tools (S34). The window shows only the 3D
view. The server's initialization instructions and each tool description explain the work without a repository
checkout or a separate manual. The headless path needs no window: `throws_simulate` reports what throws do,
`view_render` renders a moment, `video_render` an MP4 with a scripted shot. With a window:

1. `app_status {}` describes lifecycle, selected map/scene, camera mode and selection, applied versus presented
   revision, viewport and recent CPU frame samples. `next_actions` identifies tools useful in that state.
2. `maps_list {}` discovers actual local pack IDs. `app_start {"map":"de_cache"}` returns the owned instance ID.
3. `camera_set` uses that instance and a known CS2 pose. Its response supplies the actual applied camera and revision.
4. `frame_capture` with that instance and `expected_revision` returns the rendered image and frame provenance.
5. `view_render` accepts the returned map, camera, image dimensions and `render_time_seconds` to reproduce a base-map
   view headlessly, or a scene (map and throws) at a moment from a view of one of its throws. It does not change the
   visible instance. `diagnostics_get` explains errors or slow operations.
6. `scene_get`, `scene_set`, `throw_add`, `throw_update` and `throw_remove` read and change the window's scene (each
   returns the scene with what every throw did, once simulated); `time_set` moves or plays its clock; `view_set` shows
   a throw's view (thrower, follow, orbit) or the free camera; `camera_look_at` points the free camera at a point.

`scene` in the state describes the camera mode (free, thrower, follow, orbit), the number of throws and the selected
one, the clock and where it ends, playback and the effects showing (flight, smoke, fire, flash, he). `scene.loading`
is true while a map switches in or throws are still being simulated; `scene.notice` explains failures. No tool
description implies GPU or FPS measurement from CPU submission samples; actual GPU timing remains explicitly
unavailable until measured.

A scene is a map and its throws (the scene file, `nades-sandbox-scene` version 1, docs/simulator.md, reads as one): each
throw a grenade with its spot as CS2's console string (`"console": "setpos x y z; setang pitch yaw 0"`) or the
player's feet (Hammer units, Z up) with pitch and yaw (degrees, positive pitch down, within ±89), crouch, movement
(stand, walk, run, jumpthrow, run_jumpthrow), click (left, middle, right), speed (optional: the player's speed along
the view at release when it is not the movement's own; 30 for a jump-throw with forward held, S35), start (0–120 s on the scene's clock) and an
optional label; at most 64 throws. What a throw did (`ThrowReport`): `release` (t, at) and `velocity`, `bounces` (t,
at, normal), `landing` (the first bounce on a floor), `detonation` (t, at, normal), `flight_seconds` from release to
detonation, `effect_seconds`, `effect_focus`, `smoke_voxels` or `fire_flames`, and `practice`; times are seconds on the
scene's clock.

## Common contract

- Lifecycle: `stopped → starting → ready → stopping → stopped`; failures are explicit. Ready means the map's first
  frame was presented and the window is visible. New process launches receive new instance IDs, including restarts.
- All stateful actions target an instance ID. Stale IDs never target a replacement. Optional expected revisions are
  checked by the event loop at application time. User camera movement and viewport changes also update the revision.
  `presented_revision` distinguishes applied state from the last frame actually submitted for presentation while
  occluded or waiting for redraw; captures identify their own matching applied/presented revision.
- Camera position is CS2 player feet in Hammer units, Z up. Pitch/yaw are degrees; positive pitch looks down. Eye
  height defaults to 64, vertical FOV to 73.7398°. Eye height 0 denotes a direct eye position. Pitch is within ±89.9°,
  eye height within 0–128 units, FOV strictly between 1° and 179°. All values are finite; yaw normalizes to [-180,180).
- Results are structured and have output schemas. Images are native MCP PNG content, accompanied by map, camera,
  physical dimensions, UI inclusion and material-animation timestamp. Live captures also identify their instance,
  revision and frame ID; independent headless jobs have no live-instance identity. Textual results and verification
  logs omit base64. An image path alone does not establish success.
  `viewport` is the whole physical window and `scene_viewport` is the 3D rectangle within M3's panels. Scene-only PNGs
  crop that rectangle; their explicit image width/height can differ from the full-window viewport.
- Status summarizes what is shown and suggests applicable next operations. It includes actual camera/viewport,
  lifecycle, supported capabilities and frame identity. Diagnostics expose startup stage marks, build/GPU information,
  recent errors and bounded frame samples. CPU preparation/submission wall time is labelled as such; unavailable GPU
  timing is null, never zero or an inferred FPS figure. Tool descriptions state units and side effects.
  Memory reports the measured physical footprint, peak and graphics-ledger portion in bytes where supported; the
  graphics ledger is not total GPU allocation, and unsupported platforms return null.
- Operational deadlines: start/render 60 s, capture 15 s, query/graceful stop 5 s. These are failure bounds, not measured
  performance claims. Errors have stable code, explanation, retryability and current state. An operation that applied
  before cancellation is not rolled back; read state to recover. No late queued command applies after handoff.
  MCP cancellation is cooperative: handlers observe the SDK cancellation token. The SDK may suppress a cancelled
  request's response, so recovery uses a fresh status request rather than waiting for a cancellation result.
- Offscreen dimensions are positive, no larger than 4096 per side or the GPU limit. PNG payloads are at most 16 MiB.
  Captures use the current physical viewport; oversized outputs fail explicitly. Maps are IDs, never paths.
- No implicit asset packing. Pack failures explain how to run `just pack MAP`. Only configured cache packs may be read.

## Tools and individual acceptance

| Tool | Inputs and behavior | Passing evidence |
|---|---|---|
| `app_start` | Map defaults to de_cache. Same running map reuses the window; a different map directs callers to map_open. | Presented, visible map and actual state returned; repeated call creates no second child. |
| `app_status` | Inspect lifecycle/state without launching anything. | Reports a cold server, a ready window, and externally closed/failed children honestly; includes scene, camera, metrics and next actions. |
| `app_stop` | Explicit instance; bounded graceful shutdown then termination of only its owned child. Last stopped instance is safe to stop again. | Clean shutdown exits zero. Unexpected exits return app_exit_failed; forced cleanup returns app_stop_forced. Both retain stopped state and diagnostics. Unrelated/handed-off windows survive. |
| `maps_list` | Sorted local pack IDs with usable/incompatible/corrupt status. | Matches actual files; empty cache is empty; one corrupt pack does not hide valid packs or expose bytes. |
| `map_open` | Running instance, map ID, optional camera. Preflight both inputs before stopping. Restart on new map; same map avoids restart. | New map is ready; restart yields a new ID and restarted=true; invalid preflight preserves the original. Failed launch reports actual failure. |
| `camera_set` | Instance, absolute camera, optional expected revision. Reset held fly input. | Atomic pose readback within 0.01 Hammer units and 0.001°; reply identifies a presented frame at the applied revision. Invalid/stale requests do not mutate the view. |
| `frame_capture` | Instance; include_ui defaults true; optional expected revision. Native physical window or cropped scene rectangle. | Decodable PNG from the reported frame/revision; scene-only copy occurs before UI in the same submission, with UI still presented normally; app stays open. Occlusion/readback failure is explicit. |
| `view_render` | Map with a camera; or a scene on that map with `time_seconds`, a view (orbit by default, follow, thrower) of throw `throw` (the first by default) and arcs on or off, the camera optional. Width/height default 1138×640, render_time_seconds defaults zero. | No window; PNG has requested dimensions/provenance; a scene's render reports each throw's release, bounces, detonation and effect; repeated fixed-input map renders have maximum channel error ≤1 and mean error ≤0.001 display levels on the same GPU/build; live state unchanged; workers and temporary outputs cleaned. A scene on another map, without throws and without a camera, or naming a throw it lacks is refused before launching. |
| `diagnostics_get` | Log limit defaults 50, maximum 200. | Bounded actual startup/frame/error/build/GPU diagnostics; failures remain inspectable; no image blobs, raw assets or fabricated metrics. |
| `lineup_render` | Map (de_mirage or mirage), grenade (people's names: molly, inc, flashbang, frag…), the lineup line as posted (`setpos … setang …`: semicolons, roll and `_exact` optional), the technique in words or as fields (`speed` 30 for a jump-throw with W held); optional `max_megabytes` (default 25); optional output .mp4, seconds, size, fps, stills, wait. | One call renders the jumpthrow.pro-style video (spot, aim, aim point, throw, chase, landing, bloom, 360° turn; crosshair only before the release; no paths; 1920×1080 at 60 fps), five stills and a JSON beside it, and returns them with the technique as understood and where and when the grenade went off; a line without `setpos x y z` or `setang pitch yaw` is refused with what it lacks. |
| `throws_simulate` | A scene (console strings or feet/pitch/yaw). No window, no GPU. | Each throw's report within a second or two; a console string lacking `setang` (or given beside feet) is refused. |
| `scene_get` | Instance. | The window's scene, and each simulated throw's report (waiting up to 30 s for the simulations). |
| `scene_set` | Instance, scene, optional expected revision. Another map switches the window to it in place. | The scene read back equals the one set, with the reports; an invalid scene does not change the window. |
| `throw_add` | Instance, throw (console string or feet/pitch/yaw). Selects it. | The scene read back has it last, with its report. Invalid throws are refused without change. A view revision that moved only because a simulation finished is retried, not refused. |
| `throw_update` | Instance, index, throw. | The throw at the index is replaced; an index the scene lacks is refused. |
| `camera_look_at` | Instance, from, at (Hammer units), optional vertical FOV. | The free camera at `from` looks at `at` (eye height 0); the same point twice is refused. |
| `throw_remove` | Instance, index. | The throw is gone and later ones move down. |
| `time_set` | Instance, optional seconds (finite, ≥ 0), optional playing. | The clock reads back as set (clamped to the scene's end); effects showing then are reported. |
| `view_set` | Instance, view (free, thrower, follow, orbit), optional throw index. | The camera mode reads back; a view of a throw the scene lacks is refused. |
| `video_render` | A scene, start (and end) seconds on its clock; a `shot` (follow, orbit, thrower, hold moves), or a fixed camera, keyframes or a view of one of its throws; size, fps; an optional absolute `output` .mp4 path. | Returns a job ID immediately; uses one headless renderer and FFmpeg H.264; a shot's length comes from the simulation; the finished file is at `output` when given; the status carries the throws' reports; live state is unchanged. |
| `video_status` | Job ID from this MCP session. | Reports running/completed/failed/cancelled, rendered frame count, errors and final local MP4 path. |
| `video_cancel` | Job ID. | Stops and reaps the owned renderer and encoder; polling reaches cancelled and no partial MP4 remains. |

Capture and offscreen comparison use identical dimensions, camera/FOV and material time. Exposure has no temporal
settling history in M2; animated material masks do vary with time. Record actual render time and reproduce it in
offscreen requests rather than comparing unrelated frames.

## Headless video

Call `video_render` with a scene and poll `video_status`. The default is 1280×720 at 30 fps; dimensions are even and at
most 1920×1080, duration at most 60 seconds, frame rate at most 60, and the total at most 1,800 frames. Times are seconds
on the scene's clock (each throw begins at its own start), from 0; the scene's clock runs with the clip.

With no camera given, the clip is the lineup video of the first throw (`{"move": "lineup", "throw": 0, "seconds":
null}`): the spot (wide), the aim with the crosshair, the aim point (zoomed), the throw, the chase down the grenade's own
path (ported from show-me-the-nades' Orbit view, its D29), the landing, the bloom, a slow 360° turn round the effect;
its phases scale with the airtime and the effect, `seconds` stretches them; the scene's clock waits for the throw.
Flight paths are drawn only with `arcs: true`; a smoke frame averages `samples` (default 4) jittered frames. `stills`
keeps frames as PNGs, each at a moment (`spot`, `aim`, `aim_point`, `flight`, `detonation`, `bloom`, `result`) or
seconds into the clip.

The camera may instead be a `shot`: moves played one after another.
- `{"move": "follow", "throw": k, "after_seconds": s}`: 80 units behind the grenade until it goes off, then rising over
  what it left for `s` seconds.
- `{"move": "orbit", "throw": k, "radius": 350, "height": 120, "seconds": 8, "degrees": 360, "start_degrees": null}`:
  a horizontal circle round what the throw left (the smoke's middle, the fire, the burst), looking at it, from the
  camera's bearing (or `start_degrees`, 0 = +x) and easing in over its first second. Where the map hides the point the
  camera rises (up to 480 units) or tries nearer circles; it keeps 24 units from walls and open sky above it (no rooms
  the players never enter), and never comes nearer than what the throw left reaches.
- `{"move": "thrower", "throw": k, "seconds": 2}` and `{"move": "hold", "camera": {…}, "seconds": 2}`.

With a shot the clip may end with it (`end_seconds` left out). Instead of a shot: `camera` (fixed), `camera_keyframes`
(seconds from clip start, the first at 0, yaw the short way) or `view` of `throw` for the whole clip. `output` (an
absolute path ending in .mp4 whose folder exists) is where the finished file goes. For example, the smoke a person
gives as `setpos 1296.00 32.00 -167.97; setang -28.89 -166.00 0`, a jump-throw on Mirage, followed and then circled:
`{"scene": {"map": "de_mirage", "throws": [{"grenade": "smoke", "movement": "jumpthrow", "console": "setpos 1296.00
32.00 -167.97; setang -28.89 -166.00 0"}]}, "shot": [{"move": "follow", "throw": 0}, {"move": "orbit", "throw": 0,
"radius": 380, "height": 320, "seconds": 12}], "output": "/abs/mirage-smoke-0002.mp4"}`.

The renderer simulates the scene once, , emits frames to FFmpeg with libx264, and stores finished MP4s under the configured
runs directory. It shares the single headless-worker limit with `view_render`. A job belongs to its MCP session;
disconnect cancels running work, while completed files remain local. FFmpeg is required and a missing encoder is
reported in the job failure. The output has no audio. Live video recording is not supported.

## Handoff and failure recovery

The child decides readiness. EOF or failed protocol writes before usable readiness cancel startup; after readiness,
they disable control, clear held input and queued requests, and preserve normal interaction. Windowed children do not
use kill-on-drop. A disconnected client cannot stop a retained window; a new MCP server starts a fresh owned child.
Headless workers and incomplete launches are terminated and reaped on cancellation/disconnect. Explicit app_stop
always closes the currently addressed owned window. Invalid map preflight leaves the old window intact; once restart
begins, failure does not pretend that old state still exists. No graceful final notification is required for handoff.

## Verification and completion

Run `just verify-mcp` against the real MCP stdio server and real sandbox. FFmpeg with libx264, ffprobe, the
de_dust2 pack and a database with a Dust II A-site Molotov are required for video acceptance. Retain machine-readable verdicts, tool
metadata, sandbox PNGs and build IDs in `.runs/mcp-*`. Every tool has a positive and negative scenario. Missing local
packs, GPU or desktop are unavailable coverage, not a passing test. Mock protocol tests cannot satisfy local acceptance.

The required journeys are:

1. Negotiate MCP, discover all thirteen tools and their input/output schemas; list maps; inspect stopped state;
   start de_cache, repeat start, inspect ready state, capture, stop, and start again.
2. Set distinct known camera poses; independently assert conversion and numerical readback. Capture the actual
   resulting revision and compare scene-only pixels with offscreen output at the same pose, size and material time.
   First wait for M3 loading to finish and the paused scene to stabilize; require no active effects for the base-map
   comparison. A background M3 selection arriving is a legitimate revision change, not a headless-render mutation.
   Decoded mean absolute channel error must be at most 1 display level. Repeated fixed offscreen inputs use the stricter
   per-channel ≤1 and mean ≤0.001 display-level gate; bit-identical PNGs are not assumed.
3. Reject a missing map while preserving the current view; open de_mirage; confirm old child exit, fresh identity,
   rendered replacement and stale-ID rejection. Test invalid cameras, sizes, limits and revisions without mutation.
4. Render another scene headlessly while the visible window's map/camera/revision remain unchanged. Exercise child
   failure, query/capture errors, diagnostics, bounded shutdown and restart recovery.
   Also render before app_start: verify a real PNG, no window/event-loop stages and a still-stopped manager.
5. Disconnect after readiness; confirm the retained window still responds to movement/quit. A new MCP session must
   not address it. Clean only test-owned child IDs, including deliberately retained windows, and preserve evidence.

The normal gate is `just check` (release workspace tests, Clippy, justfile formatting). Also run ignored local asset
checks. Changes affecting startup/rendering require startup benchmarks and the full realism comparison per AGENTS.md.
Alternate baseline/candidate timing runs (n≥5), record load averages/stage marks and discard a new binary's first
launch. Preserve SPEC §9 targets. Game reference images and derivatives remain in the neighboring project's
`.verify-runs/`; do not copy them into MCP output or evidence.

V1 is complete only when all thirteen real-tool criteria and the five journeys have passing evidence, the verifier and
verification skill can be rerun by a fresh agent, and unresolved coverage is explicitly reported. No commit or external
client configuration change is part of this implementation.

## Verified locally · 2026-10-26

- `just verify-mcp`: **48 checks passed**, all nine tools exercised with positive/negative scenarios; every structured
  result checked against its advertised JSON Schema. [Report](../.runs/mcp-1793049074199691000/results.json); binary
  SHA256s and environment are beside it. The verifier clones the database before launching clients and confirms
  window/worker cleanup before deleting that copy.
- Desktop movement and normal ⌘Q after MCP disconnect were exercised with native UI tools using a byte-identical
  temporary app fixture. [Desktop proof](../.runs/mcp-ui-handoff-latest/results.json). Later native shutdown changes
  were checked separately with ten protected-copy startup probes and three immediate controlled shutdowns, all exit
  code zero: [regression proof](../.runs/mcp-app-shutdown-fixed/stress.json). The final public verifier also requires
  successful graceful exits; unexpected nonzero exits and forced cleanup are explicit tool errors.
- Release workspace tests, Clippy, ignored local assets, strict MCP checks and justfile formatting passed. Logs are
  local under `.runs/`. The optimizer-elided memory-allocation test now keeps the measured allocation alive across the
  actual OS reading; status exposes physical footprint, peak and graphics-ledger bytes with their precise definitions.
- Protected-copy startup: median **170.25 ms over nine measured launches**, below the 200 ms target; initial executable
  checks were discarded. [Report](../.runs/startup-20261026-160217/report.md). Seven alternated baseline/candidate pairs
  measured 187.4/176.2 ms at recorded load averages 15.93–16.35. No speedup is claimed; the baseline predates concurrent
  M3 work. [Paired report](../.runs/m1-compare-20261026-160249/report.md). Database and remembered-view paths were isolated.
- Seven-map realism metrics match the previous accepted run: blurred whole-frame and top-half gates pass on all
  seven maps; existing per-map luminance differences remain unchanged.
  [Report](../.runs/realism-20261026-152803/report.md).
- Numerical-port checks: all four views match for **84/84 throws** against the web camera selection.
  [Camera report](../.runs/cameras-20261026-153524/report.md). Twelve effect comparisons were also run against the web
  view on database clones; their existing visual differences are recorded, not claimed identical.
  [Effects report](../.runs/effects-20261026-153705/report.md).

Evidence stays local and is ignored by Git. Each report identifies its tested build; concurrent M3 changes require
fresh verification. Saved success is not proof of new code.

## Headless video verification · 2026-10-26

The real MCP verifier passed 54 checks against the local application, including all thirteen tool schemas; a moving
A-site Molotov MP4, fixed-camera smoke, invalid inputs, missing throw and encoder, busy rejection, cancellation after
both subprocesses start, disconnect cleanup, and preservation of the live view.
[Report](../.runs/mcp-1793055808478007000/results.json). The desktop movement check is a separate manual journey and
is marked pending in that automated report.

A six-second Dust II A-site clip was separately rendered at 1280×720, 30 fps: ffprobe found 180 H.264 frames.
[Video](../.runs/video-accept/runs/video-1ce27ae2-9818-48de-8dac-0f80f3246824.mp4),
[metadata](../.runs/video-accept/panorama-result.json). Decoded frames before and after detonation were inspected.
A fixed-camera smoke video frame was also inspected against a standalone still at the same effect time; the encoded
frame includes H.264 loss and material animation advances, so exact pixel identity is not assumed.
Camera comparison matched all four views of 84 throws ([report](../.runs/cameras-20261026-175923/report.md));
effect comparison ran for twelve throws ([report](../.runs/effects-20261026-180047/report.md)).
Protected-copy startup was 149.84 ms median over nine runs at the recorded load
([report](../.runs/startup-20261026-180559/report.md)); this is not a speedup claim.
