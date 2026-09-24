# Upstream Merge Guide (fork)

Goal of this guide: keep the fork's divergence from `upstream/master`
mechanically mergeable. The rule:

> Fork features live in fork-owned files (`fork_*.rs`, fork-only crates).
> Shared (upstream) files carry **only** small, clearly-marked hooks.

## Current fork surfaces

### Fork-only crates (no merge cost)

| Crate | Purpose |
|---|---|
| `crates/jcode-export-core` | Session export: JSON + self-contained HTML viewer (assets embedded) |

### Fork-only files (no merge cost)

TUI:
- `crates/jcode-tui/src/tui/app/fork_ask*.rs` - ask_user modal
- `crates/jcode-tui/src/tui/app/fork_export.rs` - `/export` command + Bus handling
- `crates/jcode-tui/src/tui/app/fork_info_tools.rs` - `/info` tool-call stats
- `crates/jcode-tui/src/tui/app/mcp_command.rs` - `/mcp` command
- `crates/jcode-tui/src/tui/app/quick_prompts.rs` - `[prompts]` slash commands
- `crates/jcode-tui/src/tui/app/tests/fork_export_info.rs` - export/info tests
- `crates/jcode-tui/src/tui/advanced_footer.rs`, `crates/jcode-tui/src/tui/chat_status.rs`

Base/app-core:
- `crates/jcode-base/src/chat.rs`, `crates/jcode-base/src/mcp/remote.rs`
- `crates/jcode-app-core/src/tool/chat.rs`, `crates/jcode-app-core/src/tool/inbox.rs`
- `docs/FORK.md`, `docs/PI_PARITY.md`, `docs/UPSTREAM_MERGE_GUIDE.md`

CLI:
- `src/cli/fork_session_export.rs` - full `jcode session export` handler

### Hooks in shared upstream files (the only merge-relevant lines)

Each hook is 1-6 lines, marked `// Fork:`. When upstream touches these
regions, resolve by keeping both sides; never move fork logic into shared
files.

| File | Hook |
|---|---|
| `Cargo.toml` | workspace member `jcode-export-core` + dep entry |
| `crates/jcode-base/src/bus.rs` | `SessionExportReady` event + variant |
| `crates/jcode-tui/Cargo.toml` | dep `jcode-export-core` |
| `crates/jcode-tui/src/tui/app.rs` | `pub(crate) mod fork_export; fork_info_tools;` |
| `crates/jcode-tui/src/tui/app/commands_dispatch.rs` | `|| super::fork_export::handle_export_command(...)` |
| `crates/jcode-tui/src/tui/app/local.rs` | `Ok(BusEvent::SessionExportReady(..))` arm |
| `crates/jcode-tui/src/tui/app/remote.rs` | same arm |
| `crates/jcode-tui/src/tui/app/state_ui.rs` | `/info` tool-calls block (5 lines) |
| `crates/jcode-tui/src/tui/app/state_ui_input_helpers.rs` | `RegisteredCommand::public("/export", ...)` |
| `crates/jcode-tui/src/tui/app/input_help.rs` | `"export" =>` help topic |
| `crates/jcode-tui/src/tui/app/commands.rs` | `pub(super) slash_command_rest` visibility |
| `crates/jcode-tui/src/tui/app/tests.rs` | `include!("tests/fork_export_info.rs")` |
| `crates/jcode-tui/src/tui/app/tests/scroll_copy_02/part_01.rs` | 2 struct fields (`timestamp`, `tool_duration_ms`) |
| `crates/jcode-tui-messages/src/message.rs` | 2 fields + None in ctors (duration/timestamp fork) |
| `crates/jcode-session-types/src/lib.rs` | `timestamp`, `tool_duration_ms` on `StoredMessage`/`RenderedMessage` |
| `crates/jcode-message-types/src/lib.rs` | `tool_duration_ms` on `Message` |
| `src/cli/args.rs` | `SessionCommand::Export` variant |
| `src/cli/mod.rs` | `pub mod fork_session_export;` |
| `src/cli/commands.rs` | `pub use super::fork_session_export::run_session_export_command;` (2 lines) |
| `src/cli/dispatch.rs` | `SessionCommand::Export` arm |

### Broader fork history in shared files

Older fork work (tool duration/timestamp feature, PRs #1478/#1480 upstream)
touches render/session/plumbing files in larger blocks. Those changes are
being upstreamed; after they merge upstream, this section shrinks.

## Merge recipe

1. `git fetch upstream && git merge upstream/master`
2. Conflicts should only appear in the hook table files. For each:
   - keep upstream's new logic,
   - re-add the `// Fork:` lines (they are additive and self-contained).
3. Run: `cargo test -p jcode-export-core -p jcode-tui --lib fork_`
4. Build: `scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode`

## Adding new fork features

- New behavior: create `fork_<name>.rs`, add one `mod` line + one dispatch
  line. Do not grow logic inside shared files.
- New bus events: add variant + payload struct in `bus.rs` (single hunk),
  handle in `fork_*.rs` via `impl App`.
- New tools/config: prefer fork-only crates over edits to upstream crates.
