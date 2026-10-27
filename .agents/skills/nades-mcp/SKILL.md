---
name: nades-mcp
description: Operate, maintain, extend, and verify the nades-sandbox MCP server and its application-control tools.
---

# Nades sandbox MCP

Use this skill when an agent needs to operate the native sandbox through MCP, diagnose or maintain the server, or add an MCP tool. Read the project's `AGENTS.md` and [MCP contract](../../../docs/mcp-spec.md) for current scope and requirements. Inspect the advertised tools and schemas for the running build; this skill is a guide, not a substitute for live discovery.

- To use the server, read [operations](references/operations.md). Start from status and map discovery, then observe the returned state after each action. For a structured tour of real behavior and acceptance evidence, see the [journey index](references/journeys.md).
- To change or add a tool, read [development](references/development.md). Trace ownership from the MCP adapter to the application or headless worker; keep tool descriptions, schemas, the contract, and the real verifier aligned.
- For a change or requested acceptance check, follow [verify-mcp](../verify-mcp/SKILL.md) and the applicable project checks. `just verify-mcp` runs real clients, binaries, maps and GPU; its report and prerequisites are described in the journey index. A previous run or a skill-format check is not evidence for a new build.

Keep game files, exports, packs and the user's database on this machine. Do not change another session's work, attach to a manual window, kill by process name, or expose raw assets. For verification, use the verifier's private database clone and isolate remembered state; operate only on the instance IDs and PIDs created by your own session. Honor the user's request to leave a ready window open: disconnect hands that window to the user and it cannot be reattached later.
