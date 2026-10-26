# List available commands.
default:
    @just --list

# Build the release binaries.
build:
    cargo build --release

# Serve the local application MCP over stdio.
mcp: build
    ./target/release/nades-mcp

# Verify all nine MCP tools against real local maps and the application.
verify-mcp: build
    cargo run --release -p nades-mcp --example verify

# Open a packed map in the app.
run $map='de_cache':
    cargo run --release --bin nades-sandbox -- --map "$map"

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

# Build a map pack from local CS2 files and exports.
pack $map='de_cache':
    cargo run --release -p nades-packer -- "$map"

# Run workspace tests.
test:
    cargo test --release --workspace

# Run ignored checks against local packs, exports, and game files.
test-local:
    cargo test --release --workspace -- --ignored

# Run Clippy on every workspace target.
lint:
    cargo clippy --release --workspace --all-targets

# Type-check and lint the workspace for Windows (DX12) from a Mac; the C++ libraries get stand-ins (scripts/check-windows.sh).
check-windows:
    scripts/check-windows.sh

# Check justfile formatting, workspace tests, and Clippy.
check: format-check test lint

# Check justfile formatting.
format-check:
    just --fmt --check

# Print one launch's startup stages as JSON.
first-frame $map='de_cache':
    cargo run --release --bin nades-sandbox -- --map "$map" --exit-after-first-frame

# Build and measure startup, steady frames, and memory.
startup $map='de_cache': build
    python3 bench/startup.py --map "$map"

# Build and compare rendered maps with the game and web references.
realism $note: build
    uv run bench/realism.py --note "$note"

# Render a frame from a CS2 console camera (comma-separated position and angles).
render $output $setpos $setang $map='de_cache':
    cargo run --release --bin nades-sandbox -- --render "$output" --map "$map" --setpos "$setpos" --setang "$setang"

# Read the top lineups from a database copy.
lineups $database $map='de_cache':
    cargo run --release -p nades-data --example lineups -- "$database" "$map"
