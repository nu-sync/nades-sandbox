# List available commands.
default:
    @just --list

# Build the release binaries.
build:
    cargo build --release

# Open a packed map in the app.
run $map='de_cache':
    cargo run --release --bin nades-sandbox -- --map "$map"

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
