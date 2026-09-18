# pi-parity features in this fork

This fork of `jcode` carries four features ported from the maintainer's pi
setup (svetlovtech). Each is opt-in or additive; upstream behavior is the
default.

## 1. MCP over HTTP / SSE

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

## 2. pi-style footer

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

## 3. Chat integration tools (`chat_notify` / `ask_user`)

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

- `inbox_list` / `inbox_read { file_id }` / `inbox_claim` - read files and
  images the user sent to the Telegram bot. Images attach to the tool result
  so vision models see them inline; text files return a preview plus the
  saved path under `<jcode-dir>/inbox/`.

## Building and testing

```bash
cargo check -p jcode-base -p jcode-app-core -p jcode-tui
cargo test -p jcode-base --lib chat::
cargo test -p jcode-base --lib mcp::
cargo test -p jcode-app-core --lib permissions::
```
