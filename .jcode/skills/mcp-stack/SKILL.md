---
name: mcp-stack
description: Working on jcode's MCP stack (crates/jcode-base/src/mcp, jcode-app-core tool/mcp.rs) or integrating new MCP servers/protocol versions. Covers the client architecture, protocol versions we speak (2024-11-05 through 2025-06-18, legacy era), the 2026-07-28 spec gaps, per-tool permission gating, and the fork's remote transports.
allowed-tools: bash, read, write, edit, grep, agentgrep, webfetch, websearch, batch, todo
---

# MCP stack (jcode fork)

Use this skill when the task touches MCP: adding a server, debugging a
connection, extending the protocol surface, or gating MCP tool calls.

## Where things live

| Path | Role |
|---|---|
| `crates/jcode-base/src/mcp/client.rs` | JSON-RPC client, initialize handshake, protocol version pick |
| `crates/jcode-base/src/mcp/remote.rs` | Streamable HTTP + legacy SSE transports, OAuth, headers |
| `crates/jcode-base/src/mcp/protocol.rs` | Wire types (InitializeParams, ToolDef, ...) |
| `crates/jcode-base/src/mcp/manager.rs` / `pool.rs` | Server lifecycle, shared connections |
| `crates/jcode-base/src/mcp/oauth.rs` | OAuth 2.1 flow for remote servers |
| `crates/jcode-app-core/src/tool/mcp.rs` | `mcp` tool (search/call/connect/...), `mcp_call` alias |
| `crates/jcode-app-core/src/tool/mod.rs` | Registry; MCP alias/policy bookkeeping (`record_mcp_surface`) |
| `crates/jcode-app-core/src/tool/permissions_gate.rs` | Fork: `[permissions]` gate; supports `tool = "mcp"`, `pattern = "server/tool"` |
| `crates/jcode-tui/src/tui/app/mcp_command.rs` | `/mcp` slash picker (fork) |

## Protocol versions we speak

`client.rs::initialize` picks per backend: stdio and legacy SSE send
`2024-11-05`; Streamable HTTP sends `2025-06-18` and accepts back
`2024-11-05 | 2025-03-26 | 2025-06-18` (legacy era). This covers every
server shipped through 2025-11-25.

## 2026-07-28 spec (modern era) - known gaps

The current spec (`https://modelcontextprotocol.io/specification/2026-07-28`)
removed the initialize handshake and `Mcp-Session-Id` sessions. Requests now
carry `_meta.io.modelcontextprotocol/protocolVersion` per call; servers must
implement `server/discover`. Other changes: `Mcp-Method`/`Mcp-Name` required
headers, `subscriptions/listen` instead of the GET stream, MRTR replaces
server-initiated requests (`InputRequiredResult` + retry with
`inputResponses`), Roots/Sampling/Logging deprecated, HTTP+SSE transport
deprecated, `ttlMs`/`cacheScope` on list results, `x-mcp-header` tool params.

A modern-only server will FAIL against us today. When adding support:
1. Probe with `server/discover` on stdio; on HTTP send a modern request and
   inspect a 400 body for a recognized modern error before falling back to
   initialize (era detection is per server, cache it).
2. Echo `requestState` verbatim on MRTR retries; new JSON-RPC id per retry.
3. `resultType` omission from older servers means `"complete"`.

## Per-tool permission gating (fork)

`[permissions]` in config.toml (off by default). MCP calls match on the
`server/tool` pair, so you can gate specific tools without touching others:

```toml
[permissions]
enabled = true
default_action = "allow"
rules = [
  { tool = "mcp", pattern = "playwright/*", action = "ask" },
  { tool = "mcp", pattern = "zai_web_reader/*", action = "allow" },
]
```

`ask` races the TUI ask modal against a Telegram card (first answer wins,
fail closed). The gate runs in `ToolRegistry::execute` before `pre_tool`
hooks and covers bash (`pattern` matches the command), file tools (path),
and `mcp_call` (same pair shape).

## Late tool announcements (mcp_tools = auto)

With `mcp_tools = "auto"` (default) and providers without native deferred
tools, the tool list is cache-frozen; servers connecting mid-session get
their tools announced once via a `<system-reminder>` block
(`announce_late_mcp_tools` in `agent/turn_execution.rs`). Repeats of the
same tool name are impossible (verified across all sessions); a *second*
announcement means a genuinely new tool (typically one slow server joining
later, e.g. `zai-zread`). `mcp_tools = "eager"` puts every server tool in
the tool list at the cost of prompt-cache misses on connect.

## Debugging a connection

1. `jcode mcp status`, then `jcode mcp connect <server>` for the exact error.
2. Logs: `~/.jcode/logs/jcode-<date>.log`, grep `MCP:`.
3. Remote: check `fallback_to_sse` (400/404/405 on initialize POST means the
   server is legacy SSE-only) and OAuth `WWW-Authenticate` handling.
4. Session files keep announcements + ToolReference blocks; count them with
   `grep -c "New MCP tools" <session>.json` (raw counts include tool results
   that quote the marker - only `<system-reminder>`-prefixed text blocks in
   `messages[].content[].text` are real announcements).
