//! Fork: session export payload model shared by the JSON and HTML writers.
//!
//! The export format is a superset of the stored session file: the original
//! `Session` JSON is preserved verbatim inside `session`, and viewer-oriented
//! metadata (`entries`, stats) is derived alongside it. Keeping the raw session
//! value intact means a JSON export round-trips: `jcode session export --format
//! json` output can be fed back to `jcode replay <file>` or resumed by hand.

use jcode_message_types::ContentBlock;
use jcode_session_types::{SessionStatus, StoredMessage};
use serde::Serialize;

/// One renderable unit in the exported transcript. Flattens the stored
/// message/content model into pi-style entries: user text, assistant text +
/// thinking, tool calls, tool results, and compaction notices.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExportEntry {
    User {
        id: String,
        timestamp: Option<String>,
        /// First non-empty user text block. Internal reminders are dropped.
        text: String,
        /// User-visible system display rows (background tasks, scheduler).
        #[serde(skip_serializing_if = "Option::is_none")]
        display_role: Option<String>,
        /// Owning session when exported together with related sessions
        /// (subagents). Absent in single-session exports.
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    Assistant {
        id: String,
        timestamp: Option<String>,
        /// Concatenated assistant text blocks (may be empty when the turn was
        /// tool-only; the sidebar hides those by default).
        text: String,
        /// Reasoning/thinking blocks, in stored order.
        thinking: Vec<String>,
        /// Tool calls issued by this message.
        tool_calls: Vec<ExportToolCall>,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    ToolResult {
        id: String,
        timestamp: Option<String>,
        tool_use_id: String,
        tool_name: String,
        content: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
        /// Wall-clock duration of the tool call, when the agent loop recorded it.
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    Compaction {
        id: String,
        timestamp: Option<String>,
        summary: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
}

/// Header-level session metadata shown by the HTML viewer.
#[derive(Debug, Clone, Serialize)]
pub struct ExportHeader {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_name: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    pub status: String,
}

/// Aggregate counters rendered in the viewer header.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ExportStats {
    pub user_messages: usize,
    pub assistant_messages: usize,
    pub tool_calls: usize,
    pub tool_results: usize,
    pub compactions: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

/// Metadata about a related (subagent) session in a multi-session export.
#[derive(Debug, Clone, Serialize)]
pub struct RelatedSession {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub is_primary: bool,
    pub message_count: usize,
}

/// Top-level payload embedded in the HTML viewer (and the shape of
/// `--format json`'s `viewer` sibling field).
#[derive(Debug, Clone, Serialize)]
pub struct ExportPayload {
    pub header: ExportHeader,
    pub entries: Vec<ExportEntry>,
    pub stats: ExportStats,
    /// Related sessions (subagents) included in a multi-session export.
    /// Empty (and skipped) for single-session exports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<RelatedSession>,
}

impl ExportHeader {
    pub fn from_session_meta(
        id: &str,
        parent_id: Option<&str>,
        title: Option<&str>,
        custom_title: Option<&str>,
        short_name: Option<&str>,
        created_at: Option<String>,
        updated_at: Option<String>,
        model: Option<&str>,
        provider_key: Option<&str>,
        working_dir: Option<&str>,
        status: &SessionStatus,
    ) -> Self {
        Self {
            id: id.to_string(),
            parent_id: parent_id.map(str::to_string),
            title: title.map(str::to_string),
            custom_title: custom_title.map(str::to_string),
            short_name: short_name.map(str::to_string),
            created_at,
            updated_at,
            model: model.map(str::to_string),
            provider_key: provider_key.map(str::to_string),
            working_dir: working_dir.map(str::to_string),
            status: status.display().to_string(),
        }
    }
}

fn timestamp_string(ts: Option<chrono::DateTime<chrono::Utc>>) -> Option<String> {
    ts.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

fn user_text_of(message: &StoredMessage) -> Option<String> {
    message.content.iter().find_map(|block| match block {
        ContentBlock::Text { text, .. } => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| text.to_string())
        }
        _ => None,
    })
}

fn assistant_text_of(message: &StoredMessage) -> String {
    let mut parts = Vec::new();
    for block in &message.content {
        if let ContentBlock::Text { text, .. } = block {
            if !text.trim().is_empty() {
                parts.push(text.as_str());
            }
        }
    }
    parts.join("\n\n")
}

fn thinking_of(message: &StoredMessage) -> Vec<String> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Reasoning { text }
            | ContentBlock::ReasoningTrace { text }
            | ContentBlock::AnthropicThinking { thinking: text, .. } => {
                (!text.trim().is_empty()).then(|| text.clone())
            }
            _ => None,
        })
        .collect()
}

/// Map tool names to their friendly display names, mirroring
/// `jcode-tui-tool-display::resolve_display_tool_name`. Duplicated here so
/// this leaf crate stays free of TUI dependencies.
fn display_tool_name(name: &str) -> &str {
    match name {
        "communicate" => "swarm",
        "discover_tools" => "integration_tools",
        "task" | "task_runner" => "subagent",
        "shell_exec" => "bash",
        "file_read" => "read",
        "file_write" => "write",
        "file_edit" => "edit",
        "file_glob" => "glob",
        "file_grep" => "grep",
        "todo_read" | "todo_write" | "todoread" | "todowrite" => "todo",
        other => other,
    }
}

/// Build the viewer payload from session parts. Kept free of `Session` so the
/// crate only depends on pure data types.
/// Input for one related session in a multi-session export.
#[derive(Debug, Clone)]
pub struct RelatedSessionInput {
    pub id: String,
    pub short_name: Option<String>,
    pub custom_title: Option<String>,
    pub model: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub is_primary: bool,
}

/// Owning session id for entries: `Some` only in multi-session exports,
/// taken from the primary (is_primary) related session.
fn owner_id(related: Option<&[RelatedSessionInput]>) -> Option<String> {
    related
        .and_then(|list| list.iter().find(|s| s.is_primary))
        .map(|s| s.id.clone())
}

pub fn build_payload(
    header: ExportHeader,
    messages: &[StoredMessage],
    compaction_summary: Option<&str>,
    // `None` for a single-session export (no session_id on entries); `Some`
    // tags every entry with its owning session and populates `sessions`.
    related: Option<&[RelatedSessionInput]>,
) -> ExportPayload {
    let mut entries = Vec::new();
    let mut stats = ExportStats::default();

    if let Some(summary) = compaction_summary.filter(|s| !s.trim().is_empty()) {
        entries.push(ExportEntry::Compaction {
            id: "compaction".to_string(),
            timestamp: None,
            summary: summary.to_string(),
            session_id: owner_id(related),
        });
        stats.compactions += 1;
    }

    for (index, message) in messages.iter().enumerate() {
        let id = format!("m{index}");
        let ts = timestamp_string(message.timestamp);

        // Tool results travel inline on any message (usually the user turn
        // that follows the assistant's tool calls). They render attached to
        // their call via `tool_use_id`, so collect them first regardless of
        // whether the surrounding message itself is user-visible.
        for (result_index, block) in message.content.iter().enumerate() {
            if let ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } = block
            {
                stats.tool_results += 1;
                entries.push(ExportEntry::ToolResult {
                    id: format!("{id}-r{result_index}"),
                    timestamp: ts.clone(),
                    tool_use_id: tool_use_id.clone(),
                    tool_name: "tool".to_string(),
                    content: content.clone(),
                    is_error: is_error.unwrap_or(false),
                    duration_ms: message.tool_duration_ms,
                    session_id: owner_id(related),
                });
            }
        }

        match message.role {
            jcode_message_types::Role::User => {
                let is_internal_reminder = message.content.iter().any(|block| {
                    matches!(block, ContentBlock::Text { text, .. } if text.trim_start().starts_with("<system-reminder>"))
                });
                if is_internal_reminder && message.display_role.is_none() {
                    continue;
                }
                let Some(text) = user_text_of(message) else {
                    continue;
                };
                stats.user_messages += 1;
                entries.push(ExportEntry::User {
                    id,
                    timestamp: ts,
                    text,
                    session_id: owner_id(related),
                    display_role: message
                        .display_role
                        .as_ref()
                        .map(|role| match role {
                            jcode_session_types::StoredDisplayRole::System => "system",
                            jcode_session_types::StoredDisplayRole::BackgroundTask => {
                                "background task"
                            }
                        }
                        .to_string()),
                });
            }
            jcode_message_types::Role::Assistant => {
                let tool_calls: Vec<ExportToolCall> = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::ToolUse { id, name, input, .. } => {
                            let intent = jcode_message_types::ToolCall::intent_from_input(input);
                            Some(ExportToolCall {
                                id: id.clone(),
                                name: display_tool_name(name).to_string(),
                                arguments: input.clone(),
                                intent,
                            })
                        }
                        _ => None,
                    })
                    .collect();
                stats.assistant_messages += 1;
                stats.tool_calls += tool_calls.len();
                entries.push(ExportEntry::Assistant {
                    id,
                    timestamp: ts,
                    text: assistant_text_of(message),
                    thinking: thinking_of(message),
                    tool_calls,
                    session_id: owner_id(related),
                });
                if let Some(usage) = &message.token_usage {
                    stats.input_tokens += usage.input_tokens;
                    stats.output_tokens += usage.output_tokens;
                    stats.cache_read_tokens += usage.cache_read_input_tokens.unwrap_or(0);
                    stats.cache_creation_tokens += usage.cache_creation_input_tokens.unwrap_or(0);
                }
            }
        }
    }

    let sessions: Vec<RelatedSession> = related
        .map(|list| {
            list.iter()
                .map(|s| RelatedSession {
                    id: s.id.clone(),
                    short_name: s.short_name.clone(),
                    custom_title: s.custom_title.clone(),
                    model: s.model.clone(),
                    created_at: s.created_at.clone(),
                    updated_at: s.updated_at.clone(),
                    is_primary: s.is_primary,
                    message_count: if s.is_primary { messages.len() } else { 0 },
                })
                .collect()
        })
        .unwrap_or_default();

    ExportPayload {
        header,
        entries,
        stats,
        sessions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_message_types::{ContentBlock, Role};
    use jcode_session_types::StoredMessage;

    fn stored(role: Role, content: Vec<ContentBlock>) -> StoredMessage {
        StoredMessage {
            id: format!("msg_{role:?}"),
            role,
            content,
            display_role: None,
            timestamp: Some(chrono::Utc::now()),
            tool_duration_ms: None,
            token_usage: None,
        }
    }

    #[test]
    fn internal_reminders_are_hidden() {
        let messages = vec![stored(
            Role::User,
            vec![ContentBlock::Text {
                text: "<system-reminder>\n# Session Context\nDate...\n</system-reminder>".into(),
                cache_control: None,
            }],
        )];
        let payload = build_payload(
            ExportHeader::from_session_meta(
                "s",
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                &SessionStatus::Active,
            ),
            &messages,
            None,
            None,
        );
        assert!(payload.entries.is_empty());
        assert_eq!(payload.stats.user_messages, 0);
    }

    #[test]
    fn tool_roundtrip_pairs_call_and_result() {
        let messages = vec![
            stored(
                Role::Assistant,
                vec![
                    ContentBlock::Text {
                        text: "Running tests".into(),
                        cache_control: None,
                    },
                    ContentBlock::ToolUse {
                        id: "call_1".into(),
                        name: "shell_exec".into(),
                        input: serde_json::json!({"command": "cargo test", "intent": "Run tests"}),
                        thought_signature: None,
                    },
                ],
            ),
            stored(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".into(),
                    content: "ok".into(),
                    is_error: Some(false),
                }],
            ),
        ];
        let payload = build_payload(
            ExportHeader::from_session_meta(
                "s",
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                &SessionStatus::Active,
            ),
            &messages,
            None,
            None,
        );
        assert_eq!(payload.stats.user_messages, 0);
        assert_eq!(payload.stats.assistant_messages, 1);
        assert_eq!(payload.stats.tool_calls, 1);
        assert_eq!(payload.stats.tool_results, 1);
        // shell_exec is displayed as bash.
        let Some(ExportEntry::Assistant { tool_calls, .. }) = payload.entries.first() else {
            panic!("expected assistant entry");
        };
        assert_eq!(tool_calls[0].name, "bash");
    }
}
