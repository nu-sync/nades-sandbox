# AGENTS.md

## Intent

nades-sandbox is a generic CS2 map and grenade simulator: pick a map, stand somewhere, aim, choose how to throw, and see
the grenade fly through the map, bounce and go off — the smoke filling around walls, the fire spreading over the floor,
the flash, the HE. Nothing in it depends on recorded matches; lineup libraries, statistics and demo analysis belong to
other applications ([docs/simulator.md](docs/simulator.md), decision S31). The maps are packed on this machine from
CS2's files (`nades-pack setup <map>`, `nades-pack <map>`). The one generic port is the scene file (JSON: a map and its
throws, `crates/control/src/scene.rs`), which other applications may write. The physics (`crates/physics`) and the
effects (`crates/effects`) are used through their public API (`release`, `fly`, `practice_command`; `smoke_grid`,
`fire_flames`); the smoke and fire renderers are ports of show-me-the-nades' web view (`nades_scene::smoke`,
`nades_scene::fire`), whose `Grid`, `fill_cloud`, `Flame` and `build_fire` are the effects crate's contract.
[SPEC.md](SPEC.md) is the plan (scope, architecture, milestones and their acceptance checks);
[docs/decisions.md](docs/decisions.md) holds each decision with its evidence. Add a decision there when you make one
that a reader would otherwise have to reconstruct.

## Commands

```bash
cargo build --release
cargo test --release --workspace
cargo clippy --release --workspace --all-targets -- -D warnings
./target/release/nades-sandbox --exit-after-first-frame    # start-up stages as JSON; run it several times, report the median
./target/release/nades-sandbox --scene <file.json>          # the simulator on a scene file's map, with its throws
./target/release/nades-sandbox --scene <file.json> --time 4 --view orbit --throw 0 --render out.png   # a moment offscreen; what each throw did as JSON
./target/release/nades-pack de_cache                        # a map's pack (after any change to crates/pack or crates/packer)
./target/release/nades-pack --effects                       # the shared effects pack: the smoke noise and fire flipbooks
./target/release/nades-pack setup de_vertigo                # fetch a map's files from Steam (DepotDownloader), once
cargo run --release -p nades-packer --example physics_vs_awpy -- de_cache   # the pack's collider against awpy's mesh
cargo run --release -p nades-packer --example lump_vs_export -- de_cache    # entities, post, sky, skybox against export_map.py's
cargo run --release -p nades-packer --example effects_vs_web                # the effects against cs2_effects.py's
cargo run --release -p nades-packer --example pack_diff -- <a.pack> <b.pack>  # what differs between two packs of a map
python3 bench/startup.py                                    # first frame (median of 9), steady frames, memory, load averages
uv run bench/realism.py --nades <checkout> --note "what changed"   # every map rendered beside the game's shots and the web view
./target/release/nades-sandbox --bench-maps de_mirage,de_cache   # each map picked in turn, timed until it shows
./target/release/nades-sandbox --bench-maps all:2           # every packed map picked twice, once all are on the GPU
python3 bench/resident.py                                   # every map in one session: switches, memory, the budget
cargo run --release -p nades-mcp --example verify           # every MCP tool against the real app (window on screen)
just bundle                                                 # the macOS app: target/release/bundle/macos/Nades Sandbox.app
just check-windows                                          # type-check and lint the workspace for Windows (DX12)
cargo run --release -p nades-render --example upload_memory -- <packs>/de_cache.pack   # where a map's memory goes
```

## Working rules

- **Valve's assets stay on this machine.** The game's files (the `[assets] game` folder), the geometry exports (the
  platform cache's `exports/`) and anything built from them (packs, the effects pack) are never committed, uploaded or shared. Rendered
  images at most. `.gitignore` refuses packs, glbs and `.vtex_c` files; keep it that way.
- **Nothing reads recorded matches.** No demo, cs2-throws file or show-me-the-nades database is read by the app or its
  tests; throws come from the throw tool, scene files and the MCP tools. Measuring the physics or effects against
  recorded throws is those crates' own benches' business, with the recorded data local and uncommitted.
- **Nothing names a sibling checkout.** No path dependency leaves the workspace and no default points at a
  show-me-the-nades checkout beside this one (`crates/sandbox/tests/standalone.rs` fails otherwise); a comparison with
  its web view takes the checkout as `--nades`.
- **The scene file is a contract.** `crates/control/src/scene.rs` defines version 1; a change of meaning is a new
  version, and readers refuse versions newer than they know.
- **Simulation stays off the first frame.** The collider, the effects files and every throw's simulation start after
  the first frame is presented; `--exit-after-first-frame` must not move.
- **Numbers are measured, not assumed.** A performance claim needs alternated runs (n ≥ 5), the load average beside
  them (`sysctl -n vm.loadavg`), and the stage marks. A newly built binary's first launch includes macOS checking the
  executable (0.3–14 s here, longer under load); leave that run out of medians.
- **Commit only when asked.**

## Before claiming a change works

Run the tests and clippy above; `cargo test --release --workspace -- --ignored` adds the checks against the local
packs, exports and game files. For anything in the simulator (`scene.rs`, `sim.rs`, `offscreen.rs`, the throws' layer),
render scenes of every grenade kind offscreen on several maps (`--scene … --render`) and look at them, and run
`cargo run --release -p nades-mcp --example verify` with the window on screen. For anything on the start-up path, run
`python3 bench/startup.py` and compare its medians with the targets in SPEC.md §9 (with `NADES_SANDBOX_STATE` on a
scratch file, so the runs do not move the user's remembered view). For anything that changes how maps are kept, loaded
or switched, run `python3 bench/resident.py` (the same scratch file) and compare it with SPEC.md §9; for anything
platform-specific, `just check-windows`. For anything that changes how a map looks, run `uv run bench/realism.py --nades
<checkout> --note …`, which compares every map with the in-game references show-me-the-nades keeps (its
`verify-scene-realism` skill), read in place. The game's shots come from jumpthrow.pro and stay in show-me-the-nades'
`.verify-runs/`: never copy them, or anything made from them, out of it. A change to the pack's Scene layout bumps
`nades_pack::VERSION` (the guard test says when) and needs every pack rebuilt.
