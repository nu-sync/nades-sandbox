# Maintain or extend the MCP

Read `docs/mcp-spec.md`, then inspect the current source and the relevant user journey. Do not assume every capability in the application is already exposed by MCP. Agree on observable success and failure behavior, side effects, ownership, units and limits before adding a tool.

## Ownership and interfaces

- `crates/mcp/src/lib.rs` owns the rmcp tool router, server instructions, input/output schemas, descriptions, annotations, error wrapping, and `next_actions`/capabilities. Keep names and descriptions useful to an agent and output schemas accurate for both success and structured failures.
- `crates/mcp/src/backend.rs` owns server sessions, child lifecycle, correlation, deadlines, cancellation, map preflight and the shared headless permit. `crates/mcp/src/video.rs` owns asynchronous video jobs and encoder/worker cleanup. Do not block the async runtime for database/GPU work, or let concurrent headless operations bypass the permit.
- `crates/control/src/` owns typed transport-independent state, commands and validation. `crates/sandbox/src/control.rs` reads the private child protocol; the window event loop owns live scene and camera mutations. Version protocol changes consistently on both sides and preserve readiness, instance identity, applied/presented revisions, and EOF handoff behavior.
- `crates/sandbox/src/sim.rs` simulates throws and draws a moment of a scene; `offscreen.rs` owns the headless scene render and video path. Reuse the app's camera and effect code for live and headless behavior; do not silently render a base map when a scene was requested.

Choose the smallest boundary the new behavior needs. A discovery tool may read through `nades-config::source` and the `nades-model` `Library`; a live mutation goes through typed child commands and the event loop; an offscreen operation belongs in an isolated worker. Validate untrusted values at the boundary, return stable actionable errors, and keep protocol stdout free of logs and image data outside MCP content. Respect cancellation, disconnect and timeouts, and reap only children the server owns. Never pass arbitrary shell commands, user-provided asset paths, or database writes as a convenience API.

## Contract and proof

Update `docs/mcp-spec.md` with each tool's inputs, side effects, failure modes and observable success; update `README.md` if the user workflow changes. Update `crates/mcp/examples/verify.rs` tool discovery and add real positive and negative scenarios, schema validation, output/readback assertions and cleanup to the affected journey. Where relevant test simultaneous calls, stale IDs/revisions, cancellation, disconnect and preservation of the live view. Unit tests support these journeys but cannot substitute for the real protocol.

Run the applicable commands in `AGENTS.md` and `just verify-mcp`; use the [journey index](journeys.md) and [verify-mcp](../../verify-mcp/SKILL.md) to identify prerequisites and gaps. For a renderer/effect change, include the required camera/effect and visual comparisons; for startup or platform changes, follow the corresponding project rules. Capture evidence, measured limitations, and any untested target environment. Update the skill when its operating guidance or references change, but keep the live schemas and spec authoritative.
