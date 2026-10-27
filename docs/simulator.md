# The grenade simulator (the product, from 2026-10-27)

nades-sandbox is a generic CS2 map and grenade simulator. You pick a map, stand somewhere, aim, choose how to throw,
and see the grenade fly through the map, bounce, and go off: the smoke filling around walls, the fire spreading over
the floor, the flash, the HE. Nothing in it depends on recorded matches or on show-me-the-nades. Lineup libraries,
team and player statistics and demo analysis belong to other applications.

## What goes

The recorded-data features leave the app:
- the lineup library: grouping, the lineup list, its team, player, side and round-time filters;
- the data adapters: `crates/source-file` (cs2-throws), `crates/source-demo` (the demo reader), `nades-sandbox check`,
  `tools/cs2_throws.py`, the `--data` flag and the `[data]` setting;
- the round replay (M5);
- the opponent viewpoints chosen from recorded players, the landing callouts learned from player samples;
- `crates/model`'s facts, library, lineups, places, viewpoints, replay and features. `crates/model::smoke` stays (the
  scene's smoke types) and so does the release-model maths in `technique.rs` (moved where the physics needs it);
- MCP tools about recorded throws (`lineups_list`, video of a recorded throw), and benches that compare recorded
  throws with show-me-the-nades' web view (`cameras.py`, `effects.py`, `tag_parity`, `replay_check.py`).

`docs/standalone-api.md` and decisions S24, S25, S28 and S30 stay as history; a new decision records the change.

## What the app is

- **Map selection.** Every packed map, resident as in M4 (`nades-pack setup` / `nades-pack` make packs, unchanged).
- **The throw tool.** Stand where the free camera stands, or click a floor point to stand there. Aim with the camera.
  Choose the grenade (smoke, flash, HE, molotov, incendiary, decoy), the movement (stand, walk, run, jump-throw,
  run-jump-throw), crouch, and the click (left, middle, right). A preview arc shows the flight live as you aim.
  Throw adds it to the scene.
- **The scene.** Every placed throw, listed: select, re-aim, change its settings, duplicate, delete. Play, pause and
  scrub time; every grenade flies and goes off at its own time (a throw has a start time, default 0).
- **Views.** Free fly camera; the thrower's eye at release; the grenade followed in flight; an orbit of the result.
- **The practice command** of the selected throw (`setpos …; setang …`), to copy into CS2.
- **Scenes as files.** Save and open a scene (JSON: map, throws, their start times): the one generic port. Other
  applications (a lineup library, show-me-the-nades) can write a scene file to show throws here.
- **Agent control (MCP).** Tools to pick a map, place and change throws, play time, and render an image or video.

## Contracts (crates)

- `crates/physics` (`nades-physics`): `Throw` (grenade, feet, pitch, yaw, crouch, movement, click) → `release()` →
  `Release { t, p0, v0 }` → `fly(release, grenade, collider)` → `Path { points every tick, bounces, detonation }`;
  `practice_command()`. The collider is the pack's (`nades_scene::collider::Collider`).
- `crates/effects` (`nades-effects`): `smoke_grid(detonation, collider) -> nades_scene::smoke::Grid` (solid voxels
  from the collider; the scene's `fill_cloud`, the port of the web view's smoke, grows the cloud from it) and
  `fire_flames(detonation, normal, collider) -> Vec<nades_scene::fire::Flame>` (a molotov's; the scene's `build_fire`
  draws them), and `fire_flames_of(kind, detonation, collider)` for either grenade, since an incendiary's fire is
  smaller and shorter (S33).
- The app: `crates/sandbox` holds the scene, the throw tool and the views; `crates/control`/`crates/mcp` the agent
  tools. A scene file is versioned (`"format": "nades-sandbox-scene", "version": 1`).

## Done when (per unit)

- **Physics:** for recorded throws (a local, uncommitted cs2-throws export as the oracle; a small committed fixture
  of recorded p0/v0, flights and detonations for CI), the simulated flight from the recorded release lands where the
  game's did: report median and p90 landing error per grenade, and the share within 32 units; the release from the
  recorded setup (feet, angles, movement, click) matches recorded p0/v0. Bounce elasticity, friction, fuse times
  (flash/HE), smoke stop detection and molotov ground detonation (and its mid-air timeout) measured, not guessed.
- **Effects:** for recorded smokes, the simulated grid's cloud against the recorded log's cloud (voxel IoU at full
  size, per map); for recorded fires, simulated flames against recorded ones (coverage overlap). Report per map.
- **App:** the removals done; the throw tool, scene, views, practice command, scene files and MCP tools working;
  offscreen renders of placed throws on several maps; tests (debug and release) and clippy `-D warnings` clean;
  docs (README, SPEC, setup, AGENTS, justfile) describe the simulator. First frame unaffected.
