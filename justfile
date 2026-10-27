pool := "de_ancient de_anubis de_cache de_dust2 de_inferno de_mirage de_nuke"

# List available commands.
default:
    @just --list --unsorted

# --- Use ---

# Open a packed map in the app; extra flags go to nades-sandbox (e.g. --data match.dem). Throws come from --data, NADES_SANDBOX_DATA or the config file.
run $map='de_cache' *args:
    cargo run --release --bin nades-sandbox -- --map "$map" {{ args }}

# Open demos, cs2-throws files or a folder of either on a map (each demo is read once into the cache).
open $data $map='de_cache' *args:
    cargo run --release --bin nades-sandbox -- --map "$map" --data "$data" {{ args }}

# Replay a round of a demo in the window (pick it under Replay); with --time and --render, write that moment as a PNG.
replay $demo $round='1' *args:
    cargo run --release --bin nades-sandbox -- --data "$demo" --replay "$(basename "$demo" .dem)" --round "$round" {{ args }}

# Render a frame from a CS2 console camera (comma-separated position and angles).
render $output $setpos $setang $map='de_cache':
    cargo run --release --bin nades-sandbox -- --render "$output" --map "$map" --setpos "$setpos" --setang "$setang"

# Render one throw's view offscreen: view is lineup, aim, result or orbit; time in seconds after the release.
render-throw $output $throw $data $view='result' $time='4':
    cargo run --release --bin nades-sandbox -- --render "$output" --throw "$throw" --data "$data" --view "$view" --time "$time"

# Report a cs2-throws file's problems and the features its facts allow.
check-data $file: build
    ./target/release/nades-sandbox check "$file"

# Write the cs2-throws fixture (the format's worked example) to a file.
fixture $file:
    python3 tools/cs2_throws.py fixture "$file"

# --- Prepare maps (Valve's files stay on this machine) ---

# Fetch and pack maps from the game's files in one step, e.g. `just map de_vertigo de_train`.
map +maps: build
    #!/usr/bin/env bash
    set -euo pipefail
    ./target/release/nades-pack setup {{ maps }}
    for m in {{ maps }}; do ./target/release/nades-pack "$m"; done

# Fetch and pack every pool map, and the shared effects pack.
maps: build
    ./target/release/nades-pack setup {{ pool }}
    for m in {{ pool }}; do ./target/release/nades-pack "$m"; done
    ./target/release/nades-pack --effects

# Fetch what maps need from Steam's CS2 depot (DepotDownloader), once before their first pack.
setup +maps: build
    ./target/release/nades-pack setup {{ maps }}

# Build a map's pack from the game's files (after `just setup <map>`).
pack $map='de_cache': build
    ./target/release/nades-pack "$map"

# Build the shared effects pack (the smoke noise and the fire's flipbooks) from the game's pak01.
effects: build
    ./target/release/nades-pack --effects

# Show the settings, the tools packing needs, the packed maps and the demo cache.
doctor:
    #!/usr/bin/env bash
    set -uo pipefail
    cfg=""
    for c in "$HOME/Library/Application Support/io.github.nu-sync.nades-sandbox/config.toml" "${XDG_CONFIG_HOME:-$HOME/.config}/nades-sandbox/config.toml"; do
        [ -f "$c" ] && cfg="$c" && break
    done
    get() { [ -n "$cfg" ] && sed -n "s/^$1 *= *\"\(.*\)\"/\1/p" "$cfg" | head -1 | sed "s|^~|$HOME|"; }
    ok() { if [ -n "$2" ] && [ -e "$2" ]; then echo "  ok      $1: $2"; else echo "  MISSING $1: ${2:-not set}"; fi; }
    echo "config:  ${cfg:-none (defaults; see docs/setup.md)}"
    echo "tools:"
    ok "Source 2 Viewer CLI ([tools] s2v)" "$(get s2v)"
    ok "DepotDownloader ([tools] depotdownloader)" "$(get depotdownloader)"
    ok "bunx (glTF-Transform)" "$(command -v bunx || true)"
    ok "python3 (fixtures, benches)" "$(command -v python3 || true)"
    ok "CS2 files ([assets] game)" "$(get game)"
    packs="$(get packs)"; packs="${NADES_SANDBOX_PACKS:-${packs:-}}"
    ok "packs folder" "$packs"
    if [ -n "$packs" ] && [ -d "$packs" ]; then
        echo "packed maps:"
        for p in "$packs"/*.pack; do [ -e "$p" ] && printf '  %-12s %s\n' "$(basename "$p" .pack)" "$(du -h "$p" | cut -f1)"; done
        ok "shared effects pack" "$packs/shared/effects.pack"
    fi
    for d in "$HOME/Library/Caches/io.github.nu-sync.nades-sandbox/throws" "${XDG_CACHE_HOME:-$HOME/.cache}/nades-sandbox/throws"; do
        [ -d "$d" ] && echo "demo cache: $d ($(ls "$d" | wc -l | tr -d ' ') files)"
    done
    data="${NADES_SANDBOX_DATA:-$(grep -A3 '^\[data\]' "${cfg:-/dev/null}" 2>/dev/null | sed -n 's/^paths *= *//p' || true)}"
    echo "data:    ${data:-the demo cache only (pass --data, set NADES_SANDBOX_DATA, or [data] paths in the config)}"

# Build the macOS app bundle (target/release/bundle/macos/Nades Sandbox.app), signed for this machine.
bundle: build
    #!/usr/bin/env bash
    set -euo pipefail
    app="target/release/bundle/macos/Nades Sandbox.app"
    version="$(cargo pkgid -p nades-sandbox | sed 's/.*@//')"
    rm -rf "$app"
    mkdir -p "$app/Contents/MacOS"
    sed "s/@VERSION@/$version/g" crates/sandbox/macos/Info.plist > "$app/Contents/Info.plist"
    cp target/release/nades-sandbox "$app/Contents/MacOS/nades-sandbox"
    codesign --force --sign - "$app"
    echo "$app"

# --- Agent control ---

# Serve the local application MCP over stdio.
mcp: build
    ./target/release/nades-mcp

# Verify the MCP tools against real local maps and the application (needs the window on screen).
verify-mcp: build
    cargo run --release -p nades-mcp --example verify

# --- Develop ---

# Build the release binaries.
build:
    cargo build --release

# Check justfile formatting, workspace tests, and Clippy.
check: format-check test lint

# Run workspace tests.
test:
    cargo test --release --workspace

# Run ignored checks against local packs, exports, and game files.
test-local:
    cargo test --release --workspace -- --ignored

# Run Clippy on every workspace target, warnings as errors.
lint:
    cargo clippy --release --workspace --all-targets -- -D warnings

# Check justfile formatting.
format-check:
    just --fmt --check

# Type-check and lint the workspace for Windows (DX12) from a Mac; the C++ libraries get stand-ins (scripts/check-windows.sh).
check-windows:
    scripts/check-windows.sh

# --- Measure ---

# Print one launch's startup stages as JSON.
first-frame $map='de_cache':
    cargo run --release --bin nades-sandbox -- --map "$map" --exit-after-first-frame

# Build and measure startup, steady frames, and memory (needs the window on screen).
startup $map='de_cache': build
    python3 bench/startup.py --map "$map"

# Map switches and memory with every packed map in one session (needs the window on screen).
resident: build
    python3 bench/resident.py

# Check replayed players and effects are drawn where the facts put them.
replay-check +data: build
    python3 bench/replay_check.py --data {{ data }}

# --- Compare with show-me-the-nades (optional; its checkout is named, never assumed) ---

# Every view's camera for 84 throws against its web view.
cameras $nades: build
    python3 bench/cameras.py --nades "$nades"

# Compare rendered maps with the game's shots and the web view.
realism $nades $note: build
    uv run bench/realism.py --nades "$nades" --note "$note"

# Compare the sandbox's groups, techniques and callouts with a producer's tags (show-me-the-nades: nades).
parity $file $ns='nades':
    cargo run --release -p nades-source-file --example tag_parity -- "$file" --ns "$ns"
