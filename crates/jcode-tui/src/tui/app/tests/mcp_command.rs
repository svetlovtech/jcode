// Fork: /mcp slash command tests.
//
// The command claims /mcp and its subcommands, rejects unknown actions, and
// blocks in SSH mode. Actual server connections need real binaries, so those
// paths are exercised through the manager's own tests in jcode-base.

fn mcp_command_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::storage::lock_test_env()
}

#[tokio::test]
async fn mcp_command_claims_root_and_subcommands() {
    let _lock = mcp_command_lock();
    // create_test_app internally block_on()s its own tiny runtime, which would
    // panic inside this test's runtime; build the app on a blocking thread.
    let mut app = tokio::task::spawn_blocking(create_test_app)
        .await
        .expect("app built");

    for input in ["/mcp", "/mcp list", "/mcp reload", "/mcp connect x", "/mcp disconnect y"] {
        app.input = input.to_string();
        app.cursor_pos = app.input.len();
        let handled = super::commands_dispatch::dispatch_local_command(&mut app, input);
        assert!(handled, "{input} should be claimed by the /mcp command");
        // Drain the pending operation so the next iteration starts clean.
        let _ = app.pending_mcp_command.take();
    }
}

#[test]
fn mcp_command_rejects_unknown_action() {
    let _lock = mcp_command_lock();
    let mut app = create_test_app();

    let handled = super::commands_dispatch::dispatch_local_command(&mut app, "/mcp frobnicate");
    assert!(handled, "unknown /mcp actions must still be claimed to show usage");
    assert!(app.pending_mcp_command.is_none(), "no background op for an unknown action");
    let last_system = app
        .display_messages
        .iter()
        .rev()
        .find(|message| message.role == "system")
        .map(|message| message.content.clone())
        .unwrap_or_default();
    assert!(
        last_system.contains("Unknown"),
        "usage error should surface as a system message; got {last_system:?}"
    );
}

#[test]
fn mcp_non_matching_input_is_not_claimed() {
    let _lock = mcp_command_lock();
    let mut app = create_test_app();

    for input in ["/mcpx", "mcp", "/model", "plain text"] {
        let handled = super::commands_dispatch::dispatch_local_command(&mut app, input);
        // /model IS claimed - by the model command. Only assert non-claim for ours.
        if input.starts_with("/mcp") || input == "mcp" {
            // "mcp" without slash and "/mcpx" must fall through; other handlers
            // may claim /model.
            if input == "/mcpx" || input == "mcp" {
                assert!(!handled, "{input} must not be claimed as /mcp");
            }
        }
    }
}

#[test]
fn mcp_command_appears_in_the_palette() {
    let _lock = mcp_command_lock();
    let app = create_test_app();
    let candidates = app.get_suggestions_for("/mcp");
    assert!(
        candidates.iter().any(|(cmd, _)| cmd == "/mcp"),
        "/mcp should be listed in the slash palette; got {candidates:?}"
    );
}
