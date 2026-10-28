# Operate the MCP

## Connect and observe

Build `just build`; the stdio executable is `target/release/nades-mcp` and must be able to launch its sibling `nades-sandbox`. `just mcp` builds and serves stdio; it is not an interactive terminal UI. Register the absolute executable path in a client's MCP settings when the user asks for configuration. If the current chat does not expose the newly registered tools, a new or restarted MCP connection may be needed; do not claim that a configuration entry proves this chat can invoke it. Use an actual MCP client (the repository verifier is one), not shell text sent to the server as a prompt. Preserve the same client session across calls.

Start with `app_status {}` and `maps_list {}`. `app_status` reports this server's own instance, not every app process on the machine. `maps_list` reports installed packs, including unusable ones; `just pack MAP` is a separate, explicit operation. Use `diagnostics_get` for bounded logs, startup marks and metrics. CPU samples are submission/preparation wall time, not FPS or GPU timing; null means unmeasured. `scene.loading`, `scene.notice`, active effects, and applied vs presented revision distinguish the map being ready from a selected throw being ready.

## Window and camera

1. Call `app_start {"map":"de_cache"}` (or another usable ID); keep the returned `instance_id`. Ready means a presented map frame and a visible window, not that the scene's throws are simulated (`scene.loading`).
2. To change maps in place, `scene_set` a scene on that map. `map_open` restarts the window on another map and returns a new instance ID; replace the old ID. Invalid preflight should preserve the original view.
3. `camera_set` takes `{ "instance_id": ID, "camera": { "position": [x,y,z], "pitch_degrees": p, "yaw_degrees": y } }`. Positions are CS2 player feet in Hammer units, Z up; pitch is positive downward. `eye_height_units` defaults to 64 (0 means eye position); `vertical_fov_degrees` defaults to 73.7398. Use `expected_revision` when intervening changes must reject the request.
4. `frame_capture` returns native PNG image content and structured state. `include_ui: false` returns the scene rectangle; its dimensions may differ from the full viewport. Use the returned frame/revision and, when appropriate, an expected revision; if the scene changes or the window is occluded, read status and handle the explicit error. Inspect the image rather than assuming a successful call proves a useful view.
5. `app_stop` closes only the owned instance. Disconnect after readiness hands the window to the user; another session cannot control it. Incomplete launches are cleaned on disconnect. Never use `app_stop` on a different session's ID or terminate unrelated windows.

## A lineup found online: one call

`lineup_render {"map": "mirage", "grenade": "smoke", "lineup": "<the setpos/setang line as posted>", "technique": "<their
words: jumpthrow, run jumpthrow, crouch, right click…>", "output": "/abs/name.mp4"}` renders the jumpthrow.pro-style
video at 1920×1080, 60 fps (about 35 s for a smoke) and returns the MP4, five stills, a JSON and where and when it went
off. Check `technique.understood` against what the post said; set `movement`, `click` or `crouch` when a word was
missed. For a quick draft pass `width: 640, height: 360, fps: 20`. Renders run one at a time; several lineups render
in a row by calling it again. Look at a still or two before calling it done.

## Headless: what a throw does, images and video

The person's words become tool calls. A throw is a grenade and a spot: the CS2 console string they paste (`"console":
"setpos 1296.00 32.00 -167.97; setang -28.89 -166.00 0"`) or feet/pitch/yaw, with movement (jumpthrow for a jump-throw),
click (left unless they say otherwise) and crouch. `throws_simulate {"scene": {"map": "de_mirage", "throws": [...]}}`
answers where and when each throw lands and goes off (times on the scene's clock: add nothing for a throw starting at
0; `flight_seconds` is from the release). Compare positions with `math.dist`-style 3D distance and say horizontal and
vertical apart.

`view_render` draws a map from a camera, or a scene's moment from a view of a throw (orbit by default). `video_render`
renders an MP4 with a `shot`: e.g. follow then a 360° pan, `[{"move": "follow", "throw": 0}, {"move": "orbit",
"throw": 0, "radius": 380, "height": 320, "seconds": 12, "degrees": 360}]`; in tight places a higher orbit (height
300+) keeps the camera above the roofs. Pass `output` (absolute .mp4) to write where the person wants. Poll
`video_status` until `completed`; it carries the path and the throws' reports. `video_cancel` stops a job. Jobs belong
to the creating MCP session; disconnect cancels unfinished jobs. Look at frames of the result (ffmpeg) before calling it
done.

Video defaults to 1280×720 at 30 fps, without audio. It requires FFmpeg with libx264 on the server's PATH; even dimensions are capped at 1920×1080, duration at 60 seconds, rate at 60 fps, and total frames at 1,800. Only one headless still/video worker runs per server; `busy` is retryable. The video tool renders the scene it is given; it does not change a live window.
