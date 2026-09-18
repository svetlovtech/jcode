# pi-parity features in this fork

This fork of `jcode` carries four features ported from the maintainer's pi
setup (svetlovtech). Each is opt-in or additive; upstream behavior is the
default.

## 1. `/goal` — pi-style goal contracts

Set a goal contract on the session; it is injected into the system prompt on
every turn until completed, blocked, or cleared.

```
/goal ship the MCP HTTP transport end-to-end   # set (prints goal_id)
/goal                                          # or /goal status
/goal clear                                    # clear (aliases: off, stop, cancel)
```

The agent sees a `# Goal Contract` block (objective + goal_id + rules) each
turn and gets three tools:

- `goal_complete { goal_id, summary }` — only after verified completion
- `goal_blocked { goal_id, reason, evidence, repeated_turns }` — requires the
  same external blocker to have recurred for 3+ consecutive turns
- `goal_wait { goal_id, reason, resume_after_ms }` — arranged external wake

State is per session, persisted under `<jcode-dir>/goals/<session>.json`, and
survives daemon restarts. Replacing a goal mints a new goal_id; stale
goal_ids cannot mutate the new contract.

Note: `/goals` (initiatives) is a separate, pre-existing feature and is
untouched.

## 2. Permissions with chat integration

pi-style allow/ask/deny rules evaluated inside `ToolRegistry::execute`
(before the `pre_tool` hook). Fully opt-in:

```toml
[permissions]
enabled = true                 # default false: no gating at all
default_action = "allow"       # for calls no rule matches: allow | ask | deny
ask_timeout_secs = 3600

[[permissions.rules]]
tool = "bash"
pattern = "git *"
action = "allow"

[[permissions.rules]]
tool = "bash"
pattern = "rm -rf *"
action = "ask"

[[permissions.rules]]
tool = "mcp:*"
action = "ask"

[[permissions.rules]]
tool = "write"
pattern = "/etc/*"
action = "deny"

[permissions.chat]
url = "https://services.aabee.tech"
token_env = "OPENCODE_CHAT_SERVICE_TOKEN"   # or token = "..."
timeout_secs = 3600
```

Rule matching: first match wins. `tool` is an exact name, `prefix*`, or `*`.
`pattern` globs the call's primary value (bash `command`, file `path`,
`url`, MCP target). `ask` sends an execution-permission card through the chat
service (`kind: "permission"`, Allow/Deny options) and blocks the tool call
until the user answers or the timeout hits. Without `chat`, `ask` denies
(fail closed), as do transport errors and unrecognized answers.

## 3. MCP over HTTP / SSE

Remote MCP servers now connect instead of being skipped. All previously
documented config shapes work in `~/.jcode/mcp.json` (and Claude Code
configs):

```json
{
  "mcpServers": {
    "search": {
      "type": "http",
      "url": "https://services.aabee.tech/mcp/search/mcp/",
      "headers": { "Authorization": "Bearer ..." }
    },
    "legacy-sse": { "type": "sse", "url": "https://example.com/sse" },
    "bare-url":   { "url": "https://example.com/mcp" }
  }
}
```

Streamable HTTP: POST per message, `Accept: application/json,
text/event-stream`, `Mcp-Session-Id` capture/echo, `Mcp-Protocol-Version`
header, concurrent in-flight requests, SSE response bodies correlated by id.
Legacy SSE: GET stream with `endpoint` event resolution. stdio servers are
unchanged.

## 4. pi-style footer

```toml
[display]
footer_style = "pi"    # or "aabee"; default "classic" = upstream output
```

Renders the overscroll status line as
`dir · branch · model (provider) · effort · context bar · cost · Σ tokens`,
omitting unavailable spans (e.g. cost on quota providers).

## Building and testing

```bash
cargo check -p jcode-base -p jcode-app-core -p jcode-tui
cargo test -p jcode-base --lib goal_contract
cargo test -p jcode-base --lib chat::
cargo test -p jcode-base --lib mcp::
cargo test -p jcode-app-core --lib permissions::
```
