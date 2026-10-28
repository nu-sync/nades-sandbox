# Lineup camera journeys

Run each with the release build. Evidence stays in `.runs/` (gitignored). Missing prerequisites are gaps, not passes.

| # | Journey | Entry point | Success |
|---|---|---|---|
| 1 | A posted lineup to a draft video | `scripts/lineup.py … --draft` (MCP `lineup_render`) | `ffprobe.json` h264 640×360 20 fps; five stills and `lineup.json` exist; `calls.jsonl` shows `technique.understood` matching the words; `frames.png` shows spot, aim with crosshair, zoom, chase with the grenade model, bloom, turn; no crosshair after the release, no lines |
| 2 | The full-quality lineup video | `scripts/lineup.py …` (no `--draft`) | 1920×1080 60 fps, about 26 s for a smoke, rendered in under a minute on an M-series Mac; the same visual checks at full size |
| 3 | Where and when, without rendering | `scripts/lineup.py … --simulate-only` (MCP `throws_simulate`) | a `ThrowReport` with release, bounces, landing, detonation, flight seconds; compare with the game's when known (mirage-smoke-0002: (−1164, −630, −166) at 6.73 s after release; simulated (−970, −587, −286) at 6.16 s) |
| 4 | Loose input | MCP `lineup_render` with `"setpos 1 2 3 setang 4 5"` forms, `molly`, `LMB+RMB` | parsed (unit tests: `cargo test -p nades-control`); a line without `setang` is refused with what it lacks |
| 5 | The MCP as a whole | `just verify-mcp` (window on screen) | its `lineup_render turns a posted lineup…` check passes, with the other 66 |

Last exercised: journeys 1, 3 and 4 on 2026-10-27 (macOS, Apple M5 Max) — see the commit that added this skill; journey
2 is how `.runs/mirage-smoke-0002.mp4` was made in the main checkout (35 s for 26.5 s at 1080p60); journey 5 passed
67 checks on 2026-10-27.
