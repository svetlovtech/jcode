# Upstream PR Prep: tool duration + tool timestamp

Status: PRs NOT sent upstream yet (owner requested parking here first).
Everything below is enough to resume in a fresh session.

## Upstream issues (already created)

- #1453 duration badge: https://github.com/1jehuang/jcode/issues/1453
- #1454 timestamp badge: https://github.com/1jehuang/jcode/issues/1454

CI requires every PR to reference an existing issue (`require-issue.yml`),
so PR bodies must contain `Closes #1453` / `Closes #1454`.

## PR #1: duration badge — branch `pr/tool-duration-wip` (pushed to this fork)

Base: upstream/master `b65931032`. Two commits:

- `92b654093` feat(protocol): carry tool duration_ms from agent loop to clients
  (wire field + emit sites + StoredMessage->RenderedMessage->DisplayMessage plumbing)
- `33e5540cd` feat(tui): duration badge on tool rows, colored by severity
  (renderer + severity_badge_color + tool_duration_severity + acceptance tests)

Validation already green: workspace `cargo check --all-targets` = 0 errors;
`cargo fmt --all` applied (extraneous fmt churn in unrelated files reverted);
tests: wire serde back-compat (1), core severity boundaries (1), TUI acceptance
`tool_duration` (5).

### Remaining for PR #1

1. **Live rows don't get the duration yet.** `server_events.rs:762` destructures
   `duration_ms` but drops it (`unused variable` clippy warning). Port the fork's
   behavior: `handle_tool_done(..., duration_ms)` in
   `crates/jcode-tui/src/tui/app/remote/server_event_handlers.rs` stamps the live
   row with `timestamp: Some(chrono::Utc::now())` and `tool_duration_ms:
   duration_ms` (see fork commit af602b07a for the exact diff).
2. Full `cargo test -p jcode-tui --lib` run; delete `target/debug` in the
   worktree afterwards (disk hygiene).
3. Push branch to this fork, then
   `gh pr create --repo 1jehuang/jcode --head svetlovtech:pr/tool-duration-wip`
   with `Closes #1453`.

## PR #2: timestamp badge — not started

Stacks ON TOP of the finished PR #1 branch (same test literals churn).
Create `pr/tool-timestamp` from `pr/tool-duration-wip` when #1 is final.

Fork commits to port (find with `git log upstream/master..svetlovtech/main`):

- `d339be3b8` — timestamp in RenderedMessage/DisplayMessage + HH:MM:SS render
  (NOTE: upstream `StoredMessage.timestamp` already exists and is recorded;
  only the render pipeline needs the field)
- `d665d6209` — `display.timestamp_tz` config (`crates/jcode-config-types/src/display.rs`,
  method `timestamp_fixed_offset_secs()`; accepts "UTC+3"/"utc-5"/"UTC+05:30"/"3";
  garbage falls back to local)
- `3bf5bcbd7` + `fde154fc1` — acceptance tests (`tool_time_badge.rs`)
- For the neutral-clock/severity-split polish (`9aa844dbc`) consider simplifying
  for upstream: keep the whole suffix neutral OR split spans; reviewer-friendly
  minimal version is neutral stamp + severity duration (matches #1454 wording).

## Mechanics

- Worktree: `cd /home/ubuntu/work/github/svetlovtech/jcode && git worktree add
  ../jcode-pr-duration pr/tool-duration-wip`
- Commit style: no hooks bypass needed in worktree (`-c core.hooksPath=/dev/null`
  was used for the fork's pre-commit; upstream worktree has no hooks configured).
- Disk: `rm -rf target/debug` in the worktree after test runs.
