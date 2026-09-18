# pi-parity features in this fork

This fork of `jcode` carries four features ported from the maintainer's pi
setup (svetlovtech). Each is opt-in or additive; upstream behavior is the
default.

## 1. Permissions with chat integration

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

## 2. MCP over HTTP / SSE

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

## 3. pi-style footer

```toml
[display]
footer_style = "pi"    # or "aabee"; default "classic" = upstream output
overscroll_status = "on"   # upstream default "overscroll" reveals the line
                           # only while scrolling past the bottom
```

Renders the overscroll status line as
`dir · branch · model (provider) · effort · context bar · cost · Σ tokens`,
omitting unavailable spans (e.g. cost on quota providers, git branch outside
a repository).

## 4. Chat integration tools (`chat_notify` / `ask_user`)

```toml
[chat]
url = "https://services.aabee.tech"
token_env = "OPENCODE_CHAT_SERVICE_TOKEN"
timeout_secs = 600
```

- `chat_notify { title, body }` - fire-and-forget Telegram notification.
- `ask_user { question, header?, options?(1-4), timeout_seconds? }` - blocking
  question rendered as a Telegram card; the tool returns the user's answer.
  Without `options` the user types a free-form answer.

Permission `ask` actions fall back to `[chat]` when `[permissions] chat` is
unset, so one section can serve both.

## Building and testing

```bash
cargo check -p jcode-base -p jcode-app-core -p jcode-tui
cargo test -p jcode-base --lib chat::
cargo test -p jcode-base --lib mcp::
cargo test -p jcode-app-core --lib permissions::
```
