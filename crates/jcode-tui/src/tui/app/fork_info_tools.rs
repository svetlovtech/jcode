//! Fork: `/info` tool-call statistics.
//!
//! Fork-only logic for the `/info` command: counts tool calls in the visible
//! display history, grouped by display tool name. Lives here so the shared
//! `state_ui.rs` only carries a small `// Fork:` call site.

use super::App;
use std::collections::BTreeMap;

/// Summarize tool calls from the visible display history as
/// "  12 bash\n   8 read\n..." (descending count, then name). Empty when the
/// session has no tool rows yet.
pub(super) fn summarize_tool_calls(app: &App) -> String {
    let counts = tool_call_counts(app);
    if counts.is_empty() {
        return String::new();
    }
    let total: usize = counts.values().sum();
    let mut sorted: Vec<(&String, &usize)> = counts.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

    let mut out = format!("  total: {}\n", total);
    for (name, count) in sorted {
        out.push_str(&format!("  {}: {}\n", name, count));
    }
    // Replace the trailing newline: callers append their own sections.
    out.pop();
    out
}

/// Count tool rows in `display_messages` by display tool name. Uses the same
/// resolved display names the transcript rows render, so `/info` numbers match
/// what the user sees.
fn tool_call_counts(app: &App) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for message in &app.display_messages {
        let Some(tool) = message.tool_data.as_ref() else {
            continue;
        };
        let name = crate::tui::ui::tools_ui::resolve_display_tool_name(&tool.name);
        *counts.entry(name.to_string()).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ToolCall;
    use jcode_tui_messages::DisplayMessage;

    fn tool_row(name: &str) -> DisplayMessage {
        DisplayMessage {
            role: "tool".to_string(),
            content: String::new(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: Some(ToolCall {
                id: format!("call_{name}"),
                name: name.to_string(),
                input: serde_json::json!({}),
                intent: None,
                thought_signature: None,
            }),
            timestamp: None,
            tool_duration_ms: None,
        }
    }

    fn text_row(text: &str) -> DisplayMessage {
        DisplayMessage {
            role: "assistant".to_string(),
            content: text.to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
            timestamp: None,
            tool_duration_ms: None,
        }
    }

    #[test]
    fn empty_session_has_empty_summary() {
        let app = crate::tui::app::tests::create_test_app();
        assert_eq!(summarize_tool_calls(&app), "");
    }

    #[test]
    fn counts_grouped_and_sorted_by_display_name() {
        let mut app = crate::tui::app::tests::create_test_app();
        // shell_exec displays as bash; two of those, one read, plus non-tool rows.
        app.display_messages.push(text_row("hi"));
        app.display_messages.push(tool_row("shell_exec"));
        app.display_messages.push(tool_row("shell_exec"));
        app.display_messages.push(tool_row("file_read"));

        let summary = summarize_tool_calls(&app);
        let lines: Vec<&str> = summary.split('\n').collect();
        assert_eq!(lines[0], "  total: 3");
        assert_eq!(lines[1], "  bash: 2");
        assert_eq!(lines[2], "  read: 1");
    }
}
