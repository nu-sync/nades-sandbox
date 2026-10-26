# AGENTS.md

## Intent

nades-sandbox is the native 3D half of show-me-the-nades (`../show-me-the-nades`): a CS2 map and the pro lineups
thrown on it, drawn from the game's own files and the throws' own data, instantly. [SPEC.md](SPEC.md) is the plan
(scope, architecture, milestones and their acceptance checks); [docs/decisions.md](docs/decisions.md) holds each
decision with its evidence. Add a decision there when you make one that a reader would otherwise have to reconstruct.

## Commands

```bash
cargo build --release
cargo test --release --workspace
cargo clippy --release --workspace --all-targets
./target/release/nades-sandbox --exit-after-first-frame    # start-up stages as JSON; run it several times, report the median
./target/release/nades-pack de_cache                        # a map's pack (after any change to crates/pack or crates/packer)
python3 bench/startup.py                                    # first frame (median of 9), steady frames, memory, load averages
uv run bench/realism.py --note "what changed"               # every map rendered beside the game's shots and the web view
```

## Working rules

- **Valve's assets stay on this machine.** Map exports (`~/.cache/nades/maps`), the game's files (`~/.cache/nades/cs2`)
  and anything built from them (packs in `~/.cache/nades/packs`) are never committed, uploaded or shared. Rendered
  images at most. `.gitignore` refuses packs, glbs and `.vtex_c` files; keep it that way.
- **The nades database is the user's.** Read it read-only, open it only for the length of a read (`crates/data`), and
  never experiment on `nades.duckdb` itself: copy it, or build a test database. `nades` fails while anything holds
  the file, and the reverse.
- **The DuckDB build is the one `nades` writes with** (`duckdb ~1.20000.0`). Change it only with show-me-the-nades.
- **Numbers are measured, not assumed.** A performance claim needs alternated runs (n ≥ 5), the load average beside
  them (`sysctl -n vm.loadavg`), and the stage marks. A newly built binary's first launch includes macOS checking the
  executable (0.3–14 s here, longer under load); leave that run out of medians.
- **Commit only when asked.**

## Before claiming a change works

Run the tests and clippy above; `cargo test --release --workspace -- --ignored` adds the checks against the local
packs, exports and game files. For anything on the start-up path, run `python3 bench/startup.py` and compare its
medians with the targets in SPEC.md §9. For anything that changes how a map looks, run `uv run bench/realism.py --note
…`, which compares every map with the in-game references show-me-the-nades keeps (its `verify-scene-realism` skill)
and with the web view's latest run, all read in place. The game's shots come from jumpthrow.pro and stay in
show-me-the-nades' `.verify-runs/`: never copy them, or anything made from them, out of it. A change to the pack's
Scene layout bumps `nades_pack::VERSION` (the guard test says when) and needs every pack rebuilt.
