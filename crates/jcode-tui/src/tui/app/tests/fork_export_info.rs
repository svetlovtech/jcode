// Fork: `/export` and `/info` dispatch + behavior tests (TUI side).
// Included from tests.rs; `create_test_app` comes from support_failover.

use crate::tui::app::commands_dispatch::dispatch_local_command;

#[test]
fn export_command_is_claimed_and_announces() {
    let mut app = create_test_app();
    let handled = dispatch_local_command(&mut app, "/export");
    assert!(handled, "/export must be handled locally");
    // Worker announcement appears immediately; the ready event lands later.
    let last = app.display_messages.last().unwrap();
    assert!(
        last.content.contains("Exporting"),
        "expected 'Exporting...' notice, got: {}",
        last.content
    );
}

#[test]
fn export_json_variant_is_claimed() {
    let mut app = create_test_app();
    let handled = dispatch_local_command(&mut app, "/export json");
    assert!(handled);
    assert!(app.display_messages.last().unwrap().content.contains("JSON"));
}

#[test]
fn export_with_explicit_path_is_claimed() {
    let mut app = create_test_app();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.json");
    let handled = dispatch_local_command(&mut app, &format!("/export json {}", path.display()));
    assert!(handled);
}

#[test]
fn export_ready_event_other_session_is_ignored() {
    let mut app = create_test_app();
    let event = crate::bus::SessionExportReady {
        session_id: "session_other_1".to_string(),
        result: Ok(std::path::PathBuf::from("/tmp/unused.html")),
    };
    app.handle_session_export_ready(event);
    // Different session id: no transcript output.
    assert!(
        !app
            .display_messages
            .iter()
            .any(|m| m.content.contains("Exported session to")),
        "events for other sessions must be dropped"
    );
}

#[test]
fn export_ready_event_current_session_announces_path() {
    let mut app = create_test_app();
    let event = crate::bus::SessionExportReady {
        session_id: app.session.id.clone(),
        result: Ok(std::path::PathBuf::from("/tmp/demo-session.html")),
    };
    app.handle_session_export_ready(event);
    let found = app
        .display_messages
        .iter()
        .any(|m| m.content.contains("/tmp/demo-session.html"));
    assert!(found, "export path must appear in the transcript");
}

#[test]
fn export_ready_event_error_is_surfaced() {
    let mut app = create_test_app();
    let event = crate::bus::SessionExportReady {
        session_id: app.session.id.clone(),
        result: Err("disk full".to_string()),
    };
    app.handle_session_export_ready(event);
    assert!(
        app.display_messages
            .iter()
            .any(|m| m.role == "error" && m.content.contains("disk full")),
        "export errors must surface in the transcript"
    );
}

#[test]
fn info_command_shows_tool_call_statistics() {
    let mut app = create_test_app();
    use crate::message::ToolCall;
    use jcode_tui_messages::DisplayMessage;

    let row = |name: &str| DisplayMessage {
        role: "tool".to_string(),
        content: String::new(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: format!("call_{name}_1"),
            name: name.to_string(),
            input: serde_json::json!({}),
            intent: None,
            thought_signature: None,
        }),
        timestamp: None,
        tool_duration_ms: None,
    };

    app.display_messages.push(row("shell_exec"));
    app.display_messages.push(row("shell_exec"));
    app.display_messages.push(row("file_read"));

    let handled = dispatch_local_command(&mut app, "/info");
    assert!(handled, "/info must be handled");
    let info = app.display_messages.last().unwrap();
    assert!(
        info.content.contains("Tool calls:"),
        "/info must contain a Tool calls section: {}",
        info.content
    );
    assert!(
        info.content.contains("bash: 2"),
        "shell_exec resolves to bash: {}",
        info.content
    );
    assert!(
        info.content.contains("read: 1"),
        "file_read resolves to read: {}",
        info.content
    );
}

#[test]
fn info_without_tools_has_no_tool_section() {
    let mut app = create_test_app();
    let handled = dispatch_local_command(&mut app, "/info");
    assert!(handled);
    let info = app.display_messages.last().unwrap();
    assert!(!info.content.contains("Tool calls:"));
}
