# AGENTS.md

## Intent

nades-sandbox shows a CS2 map and the lineups thrown on it, drawn from the game's own files and the throws' own data,
instantly. The throws are facts in `cs2-throws` files ([docs/standalone-api.md](docs/standalone-api.md)), which any
parser can write; show-me-the-nades (a separate project) is one producer. The sandbox stands alone: it builds with no
sibling checkout, and everything it computes from the facts is its own rule (`crates/model`). [SPEC.md](SPEC.md) is the plan
(scope, architecture, milestones and their acceptance checks); [docs/decisions.md](docs/decisions.md) holds each
decision with its evidence. Add a decision there when you make one that a reader would otherwise have to reconstruct.

## Commands

```bash
cargo build --release
cargo test --release --workspace
cargo clippy --release --workspace --all-targets
./target/release/nades-sandbox --exit-after-first-frame    # start-up stages as JSON; run it several times, report the median
./target/release/nades-sandbox check <file>                 # a cs2-throws file's problems, and the features its facts allow
python3 tools/cs2_throws.py fixture /tmp/fixture.sqlite     # the format's worked example (the tests read it too)
cargo run --release -p nades-source-file --example tag_parity -- <export> nades   # groups, techniques, callouts vs a producer's tags
./target/release/nades-pack de_cache                        # a map's pack (after any change to crates/pack or crates/packer)
python3 bench/startup.py                                    # first frame (median of 9), steady frames, memory, load averages
uv run bench/realism.py --nades <checkout> --note "what changed"   # every map rendered beside the game's shots and the web view
python3 bench/cameras.py --nades <checkout>                 # every view's camera for 84 throws against the web view's choice
uv run bench/effects.py --nades <checkout>                  # chosen throws' effects beside the web view's
./target/release/nades-sandbox --bench-switch 60            # what a lineup switch costs, once the stages are built
./target/release/nades-sandbox --bench-maps de_mirage,de_cache   # each map picked in turn, timed to its first lineup
./target/release/nades-sandbox --bench-maps all:2           # every packed map picked twice, once all are on the GPU
python3 bench/resident.py                                   # every map in one session: switches, memory, the budget
just bundle                                                 # the macOS app: target/release/bundle/macos/Nades Sandbox.app
just check-windows                                          # type-check and lint the workspace for Windows (DX12)
cargo run --release -p nades-render --example upload_memory -- ~/.cache/nades/packs/de_cache.pack   # where a map's memory goes
```

## Working rules

- **Valve's assets stay on this machine.** Map exports (`~/.cache/nades/maps`), the game's files (`~/.cache/nades/cs2`)
  and anything built from them (packs in `~/.cache/nades/packs`) are never committed, uploaded or shared. Rendered
  images at most. `.gitignore` refuses packs, glbs and `.vtex_c` files; keep it that way.
- **The data is the user's.** cs2-throws files are read read-only; never experiment on one in place, and never open
  show-me-the-nades' `nades.duckdb`: the benches clone it and read the clone's export. Tests write their own files.
- **Facts, derivations, tags.** A source (`ThrowSource`) hands over facts only; everything computed from them lives in
  `crates/model`, with tests, one implementation for every source; a producer's labels are tags, shown and filterable,
  never interpreted. A name in the format is either the game's or a rule written in docs/standalone-api.md §4.
- **Nothing names a sibling checkout.** No path dependency leaves the workspace and no default points at a
  show-me-the-nades checkout beside this one (`crates/sandbox/tests/standalone.rs` fails otherwise); a comparison with
  its web view takes the checkout as `--nades`.
- **The rules are show-me-the-nades' verified ones, ported**, and the stages are ports of its web view
  (`web/app/src/scene/`): keep them number for number, and check with `tag_parity` (the groups, techniques and callouts
  must match its tags), `bench/cameras.py` (the cameras must stay identical) and `bench/effects.py` after any change.
- **Numbers are measured, not assumed.** A performance claim needs alternated runs (n ≥ 5), the load average beside
  them (`sysctl -n vm.loadavg`), and the stage marks. A newly built binary's first launch includes macOS checking the
  executable (0.3–14 s here, longer under load); leave that run out of medians.
- **Commit only when asked.**

## Before claiming a change works

Run the tests and clippy above; `cargo test --release --workspace -- --ignored` adds the checks against the local
packs, exports and game files. For anything on the start-up path, run `python3 bench/startup.py` and compare its
medians with the targets in SPEC.md §9 (with `NADES_SANDBOX_DATA` on a copy and `NADES_SANDBOX_STATE` on a scratch
file, so the runs neither read the user's file nor move the user's remembered view). For anything that changes a
throw's cameras or effects, run `python3 bench/cameras.py --nades <checkout>` and `uv run bench/effects.py --nades
<checkout>`; for anything that changes a derivation, the `tag_parity` example on a show-me-the-nades export. For
anything that changes how maps are kept, loaded or switched, run `python3 bench/resident.py` (the same copy and scratch
file) and compare it with SPEC.md §9; for anything platform-specific, `just check-windows`. For anything that changes
how a map looks, run `uv run bench/realism.py --nades <checkout> --note …`, which compares every map with the in-game
references show-me-the-nades keeps (its `verify-scene-realism` skill) and with the web view's latest run, all read in
place. The game's shots come from jumpthrow.pro and stay in show-me-the-nades' `.verify-runs/`: never copy them, or
anything made from them, out of it. A change to the pack's
Scene layout bumps `nades_pack::VERSION` (the guard test says when) and needs every pack rebuilt.
