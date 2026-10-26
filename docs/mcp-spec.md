# Native application MCP v1

## Outcome and boundaries

An agent discovers nine typed tools, understands the currently shown map and camera, operates its own window,
checks actual results, and renders scenes headlessly. It can read bounded logs and correctly labelled frame metrics
without shell automation. The MCP adapter calls the same application commands as the UI. It is not a second renderer.

The server owns one window. Another MCP session and manually opened windows are independent. Switching maps may
restart the owned window; this is explicitly reported. A ready window survives MCP disconnect and becomes the user's
normal window. It cannot be reclaimed by later MCP sessions. Offscreen workers never become retained windows.

M3 throw selection, effects and playback; M4 resident map switching; semantic location navigation, picking, visibility,
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

The server's initialization instructions and each tool description explain this workflow without requiring a
repository checkout or a separate manual:

1. `app_status {}` describes lifecycle, selected map/scene, camera mode and selection, applied versus presented
   revision, viewport and recent CPU frame samples. `next_actions` identifies tools useful in that state.
2. `maps_list {}` discovers actual local pack IDs. `app_start {"map":"de_cache"}` returns the owned instance ID.
3. `camera_set` uses that instance and a known CS2 pose. Its response supplies the actual applied camera and revision.
4. `frame_capture` with that instance and `expected_revision` returns the rendered image and frame provenance.
5. `view_render` accepts the returned map, camera, image dimensions and `render_time_seconds` to reproduce a base-map
   view headlessly. V1 does not render the selected M3 throw/effects; capabilities and descriptions say so explicitly.
   It does not change the visible instance. `diagnostics_get` explains errors or slow operations.

`scene` describes the camera mode, selected lineup/throw, timeline position, playback and active effects when that
application build exposes them. In a map-only view these fields explicitly indicate no selection/playback/effects.
`scene.loading` and `scene.notice` distinguish pending data/stage work, empty results and data failures from map readiness.
Scene readback does not advertise M3 control tools as implemented. No tool description implies GPU or FPS measurement
from CPU submission samples; actual GPU timing remains explicitly unavailable until measured.

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
| `view_render` | Base map, camera, width/height default 1138×640, render_time_seconds defaults zero. | No window; PNG has requested dimensions/provenance; repeated fixed-input renders have maximum channel error ≤1 and mean error ≤0.001 display levels on the same GPU/build; live state unchanged; workers and temporary outputs cleaned. M3 effects are explicitly unsupported. |
| `diagnostics_get` | Log limit defaults 50, maximum 200. | Bounded actual startup/frame/error/build/GPU diagnostics; failures remain inspectable; no image blobs, raw assets or fabricated metrics. |

Capture and offscreen comparison use identical dimensions, camera/FOV and material time. Exposure has no temporal
settling history in M2; animated material masks do vary with time. Record actual render time and reproduce it in
offscreen requests rather than comparing unrelated frames.

## Handoff and failure recovery

The child decides readiness. EOF or failed protocol writes before usable readiness cancel startup; after readiness,
they disable control, clear held input and queued requests, and preserve normal interaction. Windowed children do not
use kill-on-drop. A disconnected client cannot stop a retained window; a new MCP server starts a fresh owned child.
Headless workers and incomplete launches are terminated and reaped on cancellation/disconnect. Explicit app_stop
always closes the currently addressed owned window. Invalid map preflight leaves the old window intact; once restart
begins, failure does not pretend that old state still exists. No graceful final notification is required for handoff.

## Verification and completion

Run `just verify-mcp` against the real MCP stdio server and real sandbox. Retain machine-readable verdicts, tool
metadata, sandbox PNGs and build IDs in `.runs/mcp-*`. Every tool has a positive and negative scenario. Missing local
packs, GPU or desktop are unavailable coverage, not a passing test. Mock protocol tests cannot satisfy local acceptance.

The required journeys are:

1. Negotiate MCP, discover exactly nine tools and their input/output schemas; list maps; inspect stopped state;
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

V1 is complete only when all nine real-tool criteria and the five journeys have passing evidence, the verifier and
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
