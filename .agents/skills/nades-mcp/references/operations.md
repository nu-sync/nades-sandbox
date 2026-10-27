# Operate the MCP

## Connect and observe

Build `just build`; the stdio executable is `target/release/nades-mcp` and must be able to launch its sibling `nades-sandbox`. `just mcp` builds and serves stdio; it is not an interactive terminal UI. Register the absolute executable path in a client's MCP settings when the user asks for configuration. If the current chat does not expose the newly registered tools, a new or restarted MCP connection may be needed; do not claim that a configuration entry proves this chat can invoke it. Use an actual MCP client (the repository verifier is one), not shell text sent to the server as a prompt. Preserve the same client session across calls.

Start with `app_status {}` and `maps_list {}`. `app_status` reports this server's own instance, not every app process on the machine. `maps_list` reports installed packs, including unusable ones; `just pack MAP` is a separate, explicit operation. Use `diagnostics_get` for bounded logs, startup marks and metrics. CPU samples are submission/preparation wall time, not FPS or GPU timing; null means unmeasured. `scene.loading`, `scene.notice`, active effects, and applied vs presented revision distinguish the map being ready from a selected throw being ready.

## Window and camera

1. Call `app_start {"map":"de_cache"}` (or another usable ID); keep the returned `instance_id`. Ready means a presented map frame and a visible window, not that lineup data has finished loading.
2. To change maps, call `map_open` with the current instance and a discovered map ID. A restart returns a new instance ID; replace the old ID. A same-map call can reuse the process. Invalid preflight should preserve the original view.
3. `camera_set` takes `{ "instance_id": ID, "camera": { "position": [x,y,z], "pitch_degrees": p, "yaw_degrees": y } }`. Positions are CS2 player feet in Hammer units, Z up; pitch is positive downward. `eye_height_units` defaults to 64 (0 means eye position); `vertical_fov_degrees` defaults to 73.7398. Use `expected_revision` when intervening changes must reject the request.
4. `frame_capture` returns native PNG image content and structured state. `include_ui: false` returns the scene rectangle; its dimensions may differ from the full viewport. Use the returned frame/revision and, when appropriate, an expected revision; if the scene changes or the window is occluded, read status and handle the explicit error. Inspect the image rather than assuming a successful call proves a useful view.
5. `app_stop` closes only the owned instance. Disconnect after readiness hands the window to the user; another session cannot control it. Incomplete launches are cleaned on disconnect. Never use `app_stop` on a different session's ID or terminate unrelated windows.

## Headless images and video

`view_render` needs a map and camera; optional width, height and `render_time_seconds` control the PNG. It is base-map only and does not change a live window. To compare with a live capture, use the exact pose, image dimensions and material time, and first wait for a paused, stable scene with no active effects. See the contract for pixel tolerances.

For a recorded throw, call `lineups_list` with map, optional `grenade` and case-insensitive destination substring, e.g. `{"map":"de_dust2","grenade":"molotov","destination":"BombsiteA"}`. Use a returned `throw_id` with `video_render`; negative start/end times are seconds before detonation. An omitted camera uses the throw's result view; `camera` gives a fixed pose; `camera_keyframes` gives timed poses from clip start. The first keyframe must be at 0; successive times increase; yaw takes the short route. A 360° orbit needs intermediate poses around the circle: one 0°→360° pair is normalized and will not orbit. Poll `video_status` with the returned job ID; present the completed local MP4 or use `video_cancel` and poll to a terminal state. Jobs belong to the creating MCP session; disconnect cancels unfinished jobs. Completed files remain local.

Video defaults to 1280×720 at 30 fps, without audio. It requires FFmpeg with libx264 on the server's PATH; even dimensions are capped at 1920×1080, duration at 60 seconds, rate at 60 fps, and total frames at 1,800. Only one headless still/video worker runs per server; `busy` is retryable. The video tool renders a recorded grenade; it does not place a new grenade or control live playback.
