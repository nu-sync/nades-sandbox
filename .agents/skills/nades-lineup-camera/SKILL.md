---
name: nades-lineup-camera
description: Render CS2 grenade lineups found online (setpos/setang plus a technique) to jumpthrow.pro-style MP4s through nades-sandbox's MCP (lineup_render), and verify the lineup camera end to end. Explains how the camera sequence, crosshair, stills, grenade models and timings are implemented (crates/sandbox/src/lineup.rs, shot.rs, offscreen.rs, crates/control). Use when asked to render a lineup video, check or change the lineup camera, or debug a lineup render.
---

# Lineup camera

An agent renders a lineup someone posted online with one MCP call. Read [how it works](references/how-it-works.md)
before changing the camera, and the [journeys](references/journeys.md) to verify it.

## Prerequisites

- `cargo build --release` (the server is `target/release/nades-mcp`; it starts its sibling `nades-sandbox`).
- The map's pack and the shared effects pack: `nades-pack <map>` and `nades-pack --effects` (the effects pack carries
  the grenades' models; without them the grenade flies as a grey capsule). `just doctor` lists what is packed.
- FFmpeg with libx264 on PATH; ffprobe to check; Pillow (`uv run --with pillow`) for contact sheets.
- No window or screen: renders are headless. A missing pack, FFmpeg or build is a coverage gap, not a pass.

## Render a lineup (the agent's path)

Call the MCP tool `lineup_render` once:

```json
{"map": "mirage", "grenade": "smoke", "technique": "jumpthrow",
 "lineup": "setpos 1296.00 32.00 -167.97; setang -28.89 -166.00 0",
 "output": "/abs/path/name.mp4"}
```

- `lineup` is the line as posted: semicolons, the roll and `_exact` optional; technique words inside it are read too.
- `technique` in words: jumpthrow, run jumpthrow, walk, run, crouch/duck, left/right/middle click, LMB, RMB, LMB+RMB.
  Check the result's `technique.understood` against the post; set `movement`, `click` or `crouch` to override.
- `speed` (u/s along the view at release) for a jump-throw with forward held: "W jumpthrow" reads as 30, or pass
  `speed: 30` (`--speed 30`). About 3 in 10 pro jump-throws drift so; it moves a long smoke 200 units (S35).
- Defaults: 1920×1080, 60 fps, length from the airtime and the effect (a smoke about 26 s, rendered in about 35 s).
  `width: 640, height: 360, fps: 20` is a draft in a few seconds. `seconds` sets the length. `max_megabytes` caps
  the MP4 (default 25, under a phone's 30 MB).
- The summary says whether the camera stayed clear of the map in every frame of the chase and the pull-out
  (`video.camera`: `clipped` and `hidden` seconds); before the release the eye is the lineup's own, and the 360° turn
  may pass through walls.
- It waits, then returns the MP4 path, five stills beside it (`<name>-aim.png`, `-aim_point`, `-flight`, `-bloom`,
  `-result`), `<name>.json` (scene, stills, what the throw did) and the detonation's time and place.
- Renders run one at a time per server (`busy` is retryable); several lineups render by calling it again.
- `throws_simulate` answers where and when without rendering.

Outside an MCP client, the helper drives the real server the same way and keeps the evidence:

```bash
uv run --with pillow python3 .agents/skills/nades-lineup-camera/scripts/lineup.py --map mirage --grenade smoke \
  --lineup "setpos 1296.00 32.00 -167.97; setang -28.89 -166.00 0" --technique jumpthrow --draft
```

It writes `.runs/lineup-<time>/`: `lineup.mp4`, stills, `lineup.json`, `calls.jsonl` (every MCP call and result),
`ffprobe.json`, `frames.png`, `server.log`. The server and renderer are its own children and end with it.

## Verify

Run the journeys in [references/journeys.md](references/journeys.md). Always look at `frames.png` (or frames along the
MP4) before calling a render right: the checks below are what a person sees.

- the spot (wide) and the aim with a green crosshair; the aim point zoomed; no crosshair from the release on;
- the game's own grenade model in the chase, never a grey capsule (unless the effects pack lacks models);
- no drawn trajectory lines; no flat grey disks;
- the camera arrives near the landing, holds on the bloom, then turns once round the effect, staying outside it and
  out of walls and closed rooms.

For the MCP as a whole, `just verify-mcp` (needs the window on screen) includes a `lineup_render` check.
Never commit `.runs/`, packs or anything exported from the game.
