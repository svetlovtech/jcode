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

## 2. advanced footer

```toml
[display]
footer_style = "advanced"    # legacy "pi"/"aabee" still work; default "classic" = upstream output
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

## 4. Quick prompts (`[prompts]` in config.toml)

Named text snippets insertable into the composer via the slash palette
(pi-style insert-only prompts):

```toml
[prompts]
review = "Please review the current diff carefully."
fix = "Fix $ARGUMENTS in the affected module."
compare = "$1 vs $2 - which is better?"
```

Prompt files: one prompt per file in the `[prompts] dir` (default
`~/.jcode/prompts`). The file name without extension is the prompt name and
the file content is the text, so `~/.jcode/prompts/review.md` becomes
`/review`. Extensions `.md`, `.markdown`, and `.txt` are recognized; other
files, subdirectories, and empty files are ignored. Prompt files are read
fresh on every palette/expansion lookup, so adding or editing a file takes
effect immediately, without a restart. A same-named inline `[prompts]` entry
wins over a prompt file, and both lose the palette dedupe to built-in
commands and skills.

Typing `/` lists every configured prompt alongside built-in commands and
skills. Picking one - or submitting `/name extra words` - replaces the
command in the composer with the prompt text for editing; nothing is sent
automatically. `extra words` substitute `$ARGUMENTS` / positional `$1`-`$9`
when present, otherwise the raw text is inserted for manual editing.
Prompt names may shadow neither built-ins nor skills (they lose the
palette dedupe to those).

## 5. ask_user stdin routing (`ServerEvent::StdinRequest.source`)

The stdin-request wire event carries a `source` tag distinguishing the two
producers: `"stdin"` (a running command wants input; upstream bash stdin
detector) and `"ask_user"` (the agent's question). The TUI only intercepts
typed input for `ask_user` requests; command stdin keeps the upstream
status-line-only behavior. `source` defaults to `"stdin"` so older
senders/clients stay compatible.

## Fork layout / merge policy

The TUI `/mcp` picker manages the local process's `McpManager`; sessions
attached to a running daemon (wire clients) are told to use the agent's
`mcp` tool instead, because the daemon owns the manager there.



Fork code is concentrated in dedicated modules; upstream files carry only
small, `// Fork:`-marked call sites. The authoritative module map, hook-site
inventory, and sync recipe live in [FORK.md](FORK.md). Summary:

- `jcode-base/src/mcp/remote.rs` - remote transport, `client.rs` keeps one
  branch point (`ClientTransport::{Stdio, Remote}`).
- `jcode-base/src/chat.rs`, `jcode-base/src/account_login.rs` - chat client,
  account login.
- `jcode-app-core/src/tool/chat.rs`, `tool/inbox.rs` - agent tools.
- `jcode-tui/src/tui/advanced_footer.rs` - footer styles; `ui_input.rs` delegates
  in one `let spans = ...` expression.
- `jcode-tui/src/tui/app/fork_ask.rs` - ask_user TUI prompt/answer flow;
  `server_events.rs`, `remote.rs`, `input_dispatch.rs`, `key_handling.rs`
  each hold a 3-10 line hook.
- `jcode-tui/src/tui/app/fork_ask_modal.rs` - the interactive ask_user modal.
- `jcode-tui/src/tui/app/mcp_command.rs` - `/mcp` slash command + picker.
- `jcode-tui/src/tui/app/quick_prompts.rs` - quick-prompt expansion, palette
  fingerprint, hint interning.
- `jcode-tui/src/tui/app/session_usage.rs` - `/clear` reset for accumulated
  token/cost totals (shared by both clear paths).

`jcode-tui/src/tui/backend.rs` and other files with formatting-only drift
are kept byte-identical to upstream.

## Building and testing

```bash
cargo check -p jcode-base -p jcode-app-core -p jcode-tui
cargo test -p jcode-base --lib chat::
cargo test -p jcode-base --lib mcp::
cargo test -p jcode-app-core --lib permissions::
cargo test -p jcode-tui --lib fork_ask_modal
```
