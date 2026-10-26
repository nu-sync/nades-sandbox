# nades-sandbox

A native CS2 map viewer built in Rust with wgpu. This first public preview renders maps with their materials,
baked lighting, sky, fog and post-processing, with a fly camera and offscreen rendering from CS2 console cameras.

![de_cache rendered in nades-sandbox](docs/images/de_cache.png)

## Status

**v0.1.0 — M2 source preview.** Tested on Apple Silicon macOS. The renderer is working; lineup browsing,
grenade playback, practice commands, instant map switching and packaged desktop builds are upcoming milestones.
Windows and Linux are not verified for this preview.

The project is the native 3D companion to `show-me-the-nades`, a separate project for CS2 demo analytics and
lineups. The viewer runs without its database. The read-only database crate is a foundation for M3.

This preview requires locally prepared CS2 assets. It contains no game files or map packs. The preparation
pipeline currently lives in the separate, unpublished `show-me-the-nades` project, so a fresh clone alone does
not provide the inputs needed to render a map. [Asset setup](docs/setup.md) describes the required layout and overrides.

[The specification](SPEC.md) records the milestones and historical performance measurements;
[the decision log](docs/decisions.md) records the architecture and rendering evidence.

## Build and run

Install Rust 1.95 or newer and the Xcode Command Line Tools on macOS. [just](https://just.systems/) is optional;
Python 3 is needed for startup benchmarks, and [uv](https://docs.astral.sh/uv/) for the rendering comparison script.
The first build compiles bundled DuckDB. Its version matches the database writer in `show-me-the-nades`.

```sh
cargo build --release
cargo test --release --workspace
cargo clippy --release --workspace --all-targets

# After preparing the local inputs described in docs/setup.md:
./target/release/nades-pack de_cache
./target/release/nades-sandbox --map de_cache
```

Hold the right mouse button to look. Use WASD to move, E/Q to move up/down, and Shift to move faster.

With just installed:

```sh
just                       # list commands
just pack de_cache
just run de_cache
just check                 # justfile formatting, workspace tests and Clippy
just test-local            # ignored tests against local game files, exports and packs
just startup               # startup, steady frames and memory
```

Render one frame from a CS2 console camera or print the startup stages:

```sh
./target/release/nades-sandbox --render shot.png --map de_dust2 --setpos 1,2,3 --setang 0,90
./target/release/nades-sandbox --exit-after-first-frame
```

`python3 bench/startup.py` records load averages and stage marks, discards the first launch, and reports the
median of nine measured launches. Performance depends on the machine, system load and prepared assets.
`uv run bench/realism.py --note "what changed"` additionally requires the sibling project's private local
reference set and web-view runs. Those reference images and generated comparison outputs are not published.

## Layout

| Path | Role |
|---|---|
| `crates/sandbox` | window, egui shell, fly camera and startup clock |
| `crates/render` | GPU renderer, materials, lighting, post-processing and console cameras |
| `crates/pack` | per-map pack format, writer and memory-mapped reader |
| `crates/source2` | VTEX textures and VPK archives |
| `crates/packer` | builds a map pack from local exports and game files |
| `crates/data` | brief, read-only access to the nades database |
| `bench/` | benchmark camera, startup timing and rendering comparisons |
| `SPEC.md`, `docs/` | plan, setup and design evidence |

Historical engine experiments mentioned in the design documents remain local under `prototypes/` and are not
included in this preview. The production workspace contains the chosen implementation.

## License

The project's source code is [MIT licensed](LICENSE). CS2 game assets are not included or covered by that
license. Game files, exports, packs and user databases stay local. The screenshot above is rendered output
from this application. This is an independent project, unaffiliated with Valve.
