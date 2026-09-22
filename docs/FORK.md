# Fork code layout and merge policy

This fork of `jcode` tracks `1jehuang/jcode` (upstream) and carries a set of
features upstream does not have (pi-parity footer, chat/ask_user integration,
MCP over HTTP, quick prompts, `/mcp` picker, and a few UX fixes). This
document is the contract for how fork code is organized so that syncing with
upstream stays cheap and mechanical.

## Core principle: dedicated modules, tiny marked hooks

Fork code lives in dedicated modules named after what they do (NOT after the
fact that they are fork code - "fork" prefixes are kept only for modules that
exist purely to bridge into upstream state). Upstream files carry only small,
`// Fork:`-marked call sites, ideally a single expression or a 3-10 line
hook. A merge from upstream should never produce conflicts inside fork
modules; conflicts in upstream files should be resolvable by re-applying a
one-line hook.

Rough current sizes (lines, fork modules vs fork-touched upstream files):

| Fork module | LOC | | Upstream file | fork lines |
|---|---|---|---|---|
| `jcode-tui/src/tui/app/fork_ask_modal.rs` | ~1360 | | `tui/app/input.rs` | ~35 |
| `jcode-app-core/src/tool/chat.rs` | ~740 | | `tui/app/remote/server_events.rs` | ~35 |
| `jcode-base/src/mcp/remote.rs` | ~470 | | `tui/app.rs` | ~23 |
| `jcode-base/src/chat.rs` | ~435 | | `tui/app/tui_state.rs` | ~18 |
| `jcode-tui/src/tui/app/mcp_command.rs` | ~330 | | `tui/ui.rs` | ~11 |
| `jcode-tui/src/tui/app/fork_ask.rs` | ~270 | | `tui/ui_input.rs` | ~12 (net -69) |
| `jcode-tui/src/tui/advanced_footer.rs` | ~230 | | `tui/app/commands_review.rs` | ~3 |

## Module map (fork-owned files)

- `jcode-base/src/chat.rs` - Telegram/chat service client (notify, ask, inbox).
- `jcode-base/src/mcp/remote.rs` - remote MCP transports (streamable HTTP,
  legacy SSE). `client.rs` keeps one branch point
  (`ClientTransport::{Stdio, Remote}`).
- `jcode-app-core/src/tool/chat.rs` - `chat_notify` / `ask_user` agent tools.
- `jcode-app-core/src/tool/inbox.rs` - `inbox_list` / `inbox_read` /
  `inbox_claim` agent tools.
- `jcode-tui/src/tui/app/fork_ask.rs` - all pending ask_user state
  (`ForkAskState`: typed-answer interception, staged answers) and the
  `ForkAskOps` facade other modules call.
- `jcode-tui/src/tui/app/fork_ask_modal.rs` - the interactive ask_user modal
  (rendering, key handling, wrapping).
- `jcode-tui/src/tui/app/mcp_command.rs` - `/mcp` slash command and picker
  plumbing (connect/disconnect, reports).
- `jcode-tui/src/tui/advanced_footer.rs` - footer styles; `ui_input.rs`
  delegates in one `let spans = ...` expression.
- `crates/jcode-tui/src/tui/app/quick_prompts.rs` - quick-prompt expansion and
  palette fingerprint/interning (see below).

Fork-local test files live next to their modules (`tests/mcp_command.rs`,
`tests/quick_prompts.rs`, `ui_tests/advanced_footer.rs`).

## Hook-site inventory (upstream files with fork call sites)

Each entry lists every fork touchpoint in an upstream file. When resolving a
merge conflict, re-apply exactly these.

- `tui/app.rs` - `mod fork_ask; mod fork_ask_modal; mod mcp_command;` +
  `pub fork_ask: ForkAskState` field + `PendingMcpCommand` struct/field +
  `fingerprint` on `CommandCandidatesCache`.
- `tui/app/input.rs` - modal key intercept in `handle_modal_key`;
  `arm_dispatch_if_pending()` in `queue_message`;
  `reset_session_usage_totals` helper (see below).
- `tui/app/commands_review.rs` - `reset_session_usage_totals()` call in
  `reset_current_session` (local `/clear`).
- `tui/app/remote/key_handling.rs` - modal key intercept + `remote /clear`
  calls `reset_session_usage_totals()`.
- `tui/app/remote/server_events.rs` - `StdinRequest` handler branches on
  `StdinRequestSource::AskUser` and opens the modal; `clear_if_pending()` on
  `TextDelta`.
- `tui/app/remote/server_event_handlers.rs` - `clear_if_pending()` when the
  `ask_user` tool call finishes.
- `tui/app/remote/input_dispatch.rs` - typed-answer interception in
  `submit_prepared_remote_input`; quick-prompt insertion in
  `submit_remote_slash_input`.
- `tui/app/remote.rs` - staged-answer flush + queued typed answer at the top
  of `process_remote_followups`; `AskQuestionResolved` bus event; MCP poll.
- `tui/app/local.rs` - mirrors of the two remote hooks (bus event, MCP poll).
- `tui/app/tui_state.rs` - `pending_ask_modal()` accessor; user-defined
  provider classified as `CostBasedApiKey`.
- `tui/app/commands_dispatch.rs` - `handle_quick_prompt_command` +
  `/mcp` dispatch entry.
- `tui/app/state_ui_input_helpers.rs` - `/mcp` palette entry + quick-prompt
  palette entries (interned hints).
- `tui/app/inline_interactive.rs` - `PickerAction::McpServer` handler.
- `tui/app/state_ui_runtime.rs` - UTC+3 turn-completion stamp in footer.
- `tui/app/tui_lifecycle.rs` - state init for `fork_ask` /
  `pending_mcp_command`; candidate cache invalidation on config reload.
- `tui/ui.rs` - late overlay draw of the ask modal (2 hooks).
- `tui/ui_input.rs` - advanced footer delegation (one expression).
- `tui/mod.rs` - `TuiState::pending_ask_modal()` (default `None`) +
  `PickerAction::McpServer`.
- `jcode-base/src/bus.rs` - `BusEvent::AskQuestionResolved`.
- `jcode-base/src/config.rs` - `chat` + `prompts` config sections.
- `jcode-config-types/src/lib.rs` - `ChatConfig`, `QuickPromptsConfig` types.
- `jcode-protocol/src/wire.rs` - `AskSpec`/`AskOptionSpec` +
  `StdinRequest { source, ask }` fields.
- `jcode-tool-core/src/lib.rs` - `StdinRequestSource` + `StdinInputRequest`
  extensions.
- `jcode-app-core/src/server/client_lifecycle.rs` - forwards `source`/`ask`
  into the wire event.
- `jcode-app-core/src/tool/mod.rs` - registers chat/inbox tools.
- `jcode-app-core/src/tool/bash.rs` - passes `source: Command` for stdin.
- `jcode-app-core/src/agent.rs`, `agent/environment.rs`,
  `agent/interrupts.rs` - cosmetic `provider_name()` renames only.

## Rules for adding fork code

1. New feature => new module. Put logic in `crates/jcode-<crate>/src/...` as
   its own file (or a `fork/` submodule folder for multi-file features). Do
   not grow hooks; grow the module.
2. Hooks are marked `// Fork:` on every site. If a hook grows past ~10 lines,
   move the body into a module and leave a one-line call.
3. State belongs in a fork-owned struct (like `ForkAskState`) with one field
   on `App`, never as scattered fields. Access goes through an ops facade
   (`fork_ask_ops()`), not direct field pokes from other modules.
4. Fork-local tests live in the fork module's own `#[cfg(test)]` or a
   dedicated test file; never extend upstream test helpers unless the test
   covers an upstream-file hook.
5. Wire/protocol additions must be backward compatible (`#[serde(default)]`,
   `skip_serializing_if`) so an older upstream server/client pair keeps
   working.

## Separation idea under evaluation: a `fork/` module folder

The TUI crate is where fork code concentrates. A stricter layout would group
`fork_ask.rs`, `fork_ask_modal.rs`, `mcp_command.rs`, `quick_prompts.rs`,
`usage_totals.rs` under `crates/jcode-tui/src/tui/fork/`, leaving upstream
files with one-line `super::fork::...` calls. Rejected for now: it changes
every `mod` path and import in one big commit, which is exactly the kind of
merge friction the dedicated-module policy exists to avoid. Revisit if the
number of fork modules in `app/` grows past ~8.

## Sync recipe (upstream -> fork)

1. `git fetch upstream && git merge upstream/master` (or rebase per feature
   branch).
2. Conflicts inside fork modules (listed above) should not happen; if one
   does, the fork module drifted - fix the module, not the merge.
3. Conflicts in upstream files: resolve by keeping upstream and re-inserting
   the marked hooks from this document.
4. `cargo check -p jcode-base -p jcode-app-core -p jcode-tui && cargo test
   -p jcode-tui --lib fork_ask_modal` as the fast post-merge gate.
