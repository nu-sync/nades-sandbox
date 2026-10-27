---
name: verify-mcp
description: Verify nades-sandbox MCP through its real stdio protocol, owned window, state readback, captures, headless renders, and disconnect handoff.
---

# Verify the native MCP

Read [the contract](../../../docs/mcp-spec.md) before changing or verifying MCP behavior.
Use `just verify-mcp` for the full local acceptance journey and `just check` for the ordinary quality gate.
The acceptance run must use the real MCP binary and sandbox, not a mocked backend. It owns its child PIDs and
cleans only those children, including any window deliberately left open by its disconnect test. Never kill by
process name. A missing GPU, desktop session, de_cache, de_mirage or de_dust2 pack, FFmpeg/libx264, ffprobe, or a cs2-throws file with the A-site Molotov is unavailable coverage, not a pass.
The verifier APFS-clones its configured cs2-throws file (the first of `NADES_SANDBOX_DATA`) before starting clients
and deletes that copy after owned windows exit. `--keep-handoff` retains the copy for the handed-off window; remove it
only after closing that window. Startup benchmarks also need `NADES_SANDBOX_DATA` on a copy and `NADES_SANDBOX_STATE`
on a scratch file, as AGENTS.md specifies.

Retain protocol results, frame metadata, sandbox screenshots and the verdict under `.runs/`; do not embed base64
images in logs. Game screenshots and their derivatives stay in the neighboring project's `.verify-runs/`.
For startup or renderer changes also run the project's startup and realism checks. Record load averages and stage
marks; performance comparisons alternate baseline/candidate runs and exclude a new binary's first launch.

For the desktop handoff journey, run `cargo run --release -p nades-mcp --example verify -- --keep-handoff`.
The report identifies the retained instance/PID; use native UI tools to verify movement and quit after disconnect.
Record that proof separately and confirm the original process exited. On macOS native UI automation may require a
temporary `.app` wrapper around a byte-identical binary under `.runs/`; configure the MCP server with
`--sandbox-binary` pointing to that executable and repeat start/disconnect before the desktop check. The verifier
pins bare executable copies, so passing it a bundle executable alone does not preserve the bundle. This is a
verification fixture, not the M4 shipping app bundle.

App readiness and selected-scene readiness are separate: wait for `scene.loading` to clear and a paused, stable
scene before testing whether headless rendering preserves its revision. V1 `view_render` renders the base map;
only compare it with a live scene whose `active_effects` is empty. Copy actual image dimensions, camera, FOV and
material time. Repeated outputs use the measured tolerance in the specification, not a PNG-hash assumption.

Video acceptance uses the real asynchronous job tools and a private database clone. Probe the completed MP4 with
`ffprobe`; inspect decoded frames before and after detonation, test cancellation, and confirm partial files are gone.
