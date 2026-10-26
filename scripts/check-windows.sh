#!/usr/bin/env bash
# Type-checks and lints the whole workspace for Windows (x86_64-pc-windows-msvc: DX12 through wgpu) from a Mac.
#
# Without a Windows SDK here, the two C++ libraries the workspace builds (DuckDB, bundled; meshoptimizer) cannot be
# compiled for the target, so stand-ins answer their build scripts with empty objects and archives. `cargo check` and
# `cargo clippy` never link, so every crate's Rust (cfg(windows) paths included) is checked in full and nothing built by
# the stand-ins is read. A Windows machine (or the SDK) builds the real thing. Needs: rustup target add x86_64-pc-windows-msvc
set -euo pipefail
cd "$(dirname "$0")/.."
target=x86_64-pc-windows-msvc
stubs="$PWD/target/windows-check/stubs"
mkdir -p "$stubs"
cat > "$stubs/cc" <<'STUB'
#!/bin/sh
# a stand-in C/C++ compiler: an empty object wherever one is asked for (-o, /Fo, -Fo)
out=""
prev=""
for a in "$@"; do
  case "$a" in
    /Fo*) out="${a#/Fo}" ;;
    -Fo*) out="${a#-Fo}" ;;
    --version|-v|-V) echo "clang version 18.0.0 (stand-in)"; exit 0 ;;
  esac
  [ "$prev" = "-o" ] && out="$a"
  prev="$a"
done
case " $* " in *" -E "*) exit 0 ;; esac
[ -n "$out" ] && : > "$out"
exit 0
STUB
cat > "$stubs/ar" <<'STUB'
#!/bin/sh
# a stand-in archiver: an empty archive named by -out: or /OUT: (lib.exe's form), else the first .lib or .a
for a in "$@"; do
  case "$a" in
    -out:*|-OUT:*|/out:*|/OUT:*) : > "${a#*:}"; exit 0 ;;
  esac
done
for a in "$@"; do
  case "$a" in
    *.lib|*.a) : > "$a"; exit 0 ;;
  esac
done
exit 0
STUB
chmod +x "$stubs/cc" "$stubs/ar"
export CC_x86_64_pc_windows_msvc="$stubs/cc" CXX_x86_64_pc_windows_msvc="$stubs/cc" AR_x86_64_pc_windows_msvc="$stubs/ar"
# a target directory of its own: the stand-ins' empty libraries never meet a real build's
cargo check --release --workspace --all-targets --target "$target" --target-dir target/windows-check
cargo clippy --release --workspace --all-targets --target "$target" --target-dir target/windows-check -- -D warnings
echo "the workspace checks and lints clean for $target"
