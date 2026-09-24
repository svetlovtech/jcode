//! Fork: session export for jcode (JSON + self-contained HTML).
//!
//! Upstream jcode stores sessions at `~/.jcode/sessions/<id>.json` but has no
//! export surface. This crate adds one:
//!
//! * [`export_json`] - the full stored session plus a derived viewer payload,
//!   for archival/processing.
//! * [`export_html`] - a single self-contained HTML file modeled on the pi
//!   harness export (`pi --export`, MIT): sidebar transcript index with search
//!   and filters, collapsible thinking/tool outputs, markdown rendering
//!   (vendored marked, MIT) and syntax highlighting (vendored highlight.js,
//!   BSD-3-Clause), and a "download JSON" button. No network access: every
//!   asset is embedded.
//!
//! The HTML embeds the same JSON payload the CLI writes, base64-encoded, so
//! the file is self-describing and the viewer's JSON button round-trips.
use base64::Engine;
use jcode_session_types::{SessionStatus, StoredCompactionState, StoredMessage};
use serde::Serialize;

pub mod payload;

pub use payload::{
    build_payload, ExportEntry, ExportHeader, ExportPayload, ExportStats, ExportToolCall,
    RelatedSession, RelatedSessionInput,
};

const TEMPLATE_HTML: &str = include_str!("../assets/template.html");
const TEMPLATE_CSS: &str = include_str!("../assets/template.css");
const TEMPLATE_JS: &str = include_str!("../assets/template.js");
const MARKED_JS: &str = include_str!("../assets/marked.min.js");
const HIGHLIGHT_JS: &str = include_str!("../assets/highlight.min.js");

/// Output format for a session export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Json,
    Html,
}

impl ExportFormat {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "json" => Some(Self::Json),
            "html" => Some(Self::Html),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Html => "html",
        }
    }
}

/// Full JSON export: the stored session verbatim plus the viewer payload.
#[derive(Debug, Clone, Serialize)]
pub struct SessionJsonExport<'a> {
    /// The stored session file content, preserved as-is (raw `Session` JSON).
    pub session: &'a serde_json::Value,
    /// Derived, viewer-oriented payload (flattened entries + stats).
    pub viewer: &'a ExportPayload,
    /// Schema marker for future evolution.
    pub export_version: u32,
}

/// Everything the writers need about one session. Callers bridge from
/// `jcode_base::session::Session` (which this leaf crate must not depend on).
#[derive(Debug, Clone)]
pub struct SessionExportInput<'a> {
    pub id: &'a str,
    pub parent_id: Option<&'a str>,
    pub title: Option<&'a str>,
    pub custom_title: Option<&'a str>,
    pub short_name: Option<&'a str>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub model: Option<&'a str>,
    pub provider_key: Option<&'a str>,
    pub working_dir: Option<&'a str>,
    pub status: &'a SessionStatus,
    pub messages: &'a [StoredMessage],
    pub compaction: Option<&'a StoredCompactionState>,
    /// Related sessions (subagents) for a multi-session export. `None` =
    /// single-session export: entries carry no `session_id` and the payload's
    /// `sessions` stays empty.
    pub related: Option<&'a [payload::RelatedSessionInput]>,
    /// The session file content serialized as raw JSON (for `session` in the
    /// JSON export). `None` skips the wrapper and emits only the viewer data.
    pub raw_session_json: Option<&'a serde_json::Value>,
}

impl<'a> SessionExportInput<'a> {
    pub fn header(&self) -> ExportHeader {
        ExportHeader::from_session_meta(
            self.id,
            self.parent_id,
            self.title,
            self.custom_title,
            self.short_name,
            self.created_at.clone(),
            self.updated_at.clone(),
            self.model,
            self.provider_key,
            self.working_dir,
            self.status,
        )
    }

    pub fn payload(&self) -> ExportPayload {
        build_payload(
            self.header(),
            self.messages,
            self.compaction.map(|c| c.summary_text.as_str()),
            self.related,
        )
    }
}

/// Serialize the JSON export.
pub fn export_json(input: &SessionExportInput) -> anyhow::Result<String> {
    let payload = input.payload();
    let value = match input.raw_session_json {
        Some(raw) => serde_json::to_string_pretty(&SessionJsonExport {
            session: raw,
            viewer: &payload,
            export_version: 1,
        })?,
        None => serde_json::to_string_pretty(&payload)?,
    };
    Ok(value)
}

/// Render the self-contained HTML export.
pub fn export_html(input: &SessionExportInput) -> anyhow::Result<String> {
    let payload = input.payload();
    let json = serde_json::to_string(&payload)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
    let html = TEMPLATE_HTML
        .replace("{{CSS}}", TEMPLATE_CSS)
        .replace("{{JS}}", TEMPLATE_JS)
        .replace("{{MARKED_JS}}", MARKED_JS)
        .replace("{{HIGHLIGHT_JS}}", HIGHLIGHT_JS)
        .replace("{{SESSION_DATA}}", &encoded);
    Ok(html)
}

/// Default output path for an export: `<input>.export.<ext>` next to the
/// source file, or `jcode-session-<id>.<ext>` in the current directory when
/// no source path is known.
pub fn default_output_path(
    source_path: Option<&std::path::Path>,
    id: &str,
    format: ExportFormat,
) -> std::path::PathBuf {
    match source_path.and_then(|p| p.parent()) {
        Some(dir) if dir.exists() => dir.join(format!("{}.export.{}", id, format.extension())),
        _ => std::path::PathBuf::from(format!("jcode-session-{}.{}", id, format.extension())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_message_types::{ContentBlock, Role};
    use jcode_session_types::StoredMessage;

    fn sample_input(raw: Option<&serde_json::Value>) -> SessionExportInput<'_> {
        // All `&'a str` fields below are literals ('static); `messages` and
        // `status` borrow from leaked allocations so the test input can use a
        // non-static lifetime parameter exactly like production callers.
        SessionExportInput {
            id: "session_test_1",
            parent_id: None,
            title: None,
            custom_title: Some("Test session"),
            short_name: Some("test"),
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
            updated_at: Some("2026-01-01T00:10:00Z".to_string()),
            model: Some("test-model"),
            provider_key: Some("openai"),
            working_dir: Some("/tmp"),
            status: leaked_status(),
            messages: leaked_messages(),
            compaction: None,
        related: None,
            raw_session_json: raw,
        }
    }

    fn leaked_status() -> &'static SessionStatus {
        Box::leak(Box::new(SessionStatus::Closed))
    }

    fn leaked_messages() -> &'static [StoredMessage] {
        Box::leak(Box::new(vec![
            StoredMessage {
                id: "m1".into(),
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Hello **world**".into(),
                    cache_control: None,
                }],
                display_role: None,
                timestamp: Some(chrono::Utc::now()),
                tool_duration_ms: None,
                token_usage: None,
            },
            StoredMessage {
                id: "m2".into(),
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Text {
                        text: "Hi!".into(),
                        cache_control: None,
                    },
                    ContentBlock::ToolUse {
                        id: "call_1".into(),
                        name: "shell_exec".into(),
                        input: serde_json::json!({"command": "echo hi"}),
                        thought_signature: None,
                    },
                ],
                display_role: None,
                timestamp: Some(chrono::Utc::now()),
                tool_duration_ms: Some(1500),
                token_usage: None,
            },
        ]))
    }

    #[test]
    fn json_export_wraps_raw_session() {
        let raw = serde_json::json!({"id": "session_test_1", "messages": []});
        let input = sample_input(Some(&raw));
        let json = export_json(&input).unwrap();
        assert!(json.contains("\"export_version\": 1"));
        assert!(json.contains("\"session\""));
        assert!(json.contains("\"viewer\""));
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["session"]["id"], "session_test_1");
        assert_eq!(parsed["viewer"]["stats"]["tool_calls"], 1);
    }

    #[test]
    fn html_export_is_self_contained() {
        let input = sample_input(None);
        let html = export_html(&input).unwrap();
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(!html.contains("{{CSS}}"));
        assert!(!html.contains("{{JS}}"));
        assert!(!html.contains("{{SESSION_DATA}}"));
        assert!(!html.contains("{{MARKED_JS}}"));
        assert!(!html.contains("{{HIGHLIGHT_JS}}"));
        assert!(html.contains("marked v18"));
        assert!(html.contains("Highlight.js v11"));
        // base64 payload present and decodable
        let marker = "<script id=\"session-data\" type=\"application/json\">";
        let start = html.find(marker).unwrap() + marker.len();
        let end = html[start..].find("</script>").unwrap() + start;
        let encoded = html[start..end].trim();
        use base64::Engine;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded.as_bytes())
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(payload["header"]["id"], "session_test_1");
        assert_eq!(payload["entries"].as_array().unwrap().len(), 2);
    }


    #[test]
    fn related_sessions_tag_entries_and_payload() {
        let primary = RelatedSessionInput {
            id: "session_main".into(),
            short_name: Some("main".into()),
            custom_title: None,
            model: Some("m1".into()),
            created_at: None,
            updated_at: None,
            is_primary: true,
            message_count: 0,
        };
        let sub = RelatedSessionInput {
            id: "session_sub_1".into(),
            short_name: Some("sloth".into()),
            custom_title: None,
            model: Some("m2".into()),
            created_at: None,
            updated_at: None,
            is_primary: false,
            message_count: 57,
        };
        let input = SessionExportInput {
            id: "session_main",
            parent_id: None,
            title: None,
            custom_title: None,
            short_name: None,
            created_at: None,
            updated_at: None,
            model: None,
            provider_key: None,
            working_dir: None,
            status: Box::leak(Box::new(SessionStatus::Active)),
            messages: leaked_messages(),
            compaction: None,
            related: Some(&[primary, sub]),
            raw_session_json: None,
        };
        let payload = input.payload();
        assert_eq!(payload.sessions.len(), 2);
        assert!(payload.sessions[0].is_primary);
        assert_eq!(payload.sessions[1].short_name.as_deref(), Some("sloth"));
        assert_eq!(payload.sessions[1].message_count, 57);
        assert_eq!(payload.sessions[0].message_count, leaked_messages().len());
        // All entries carry the primary session id.
        for entry in &payload.entries {
            let sid = match entry {
                ExportEntry::User { session_id, .. }
                | ExportEntry::Assistant { session_id, .. }
                | ExportEntry::ToolResult { session_id, .. }
                | ExportEntry::Compaction { session_id, .. } => session_id,
            };
            assert_eq!(sid.as_deref(), Some("session_main"));
        }
    }

    #[test]
    fn html_escapes_session_content() {
        // A session containing HTML/script must not execute it: marked's HTML
        // tokenizer is disabled and content goes through escapeHtml.
        let messages = Box::leak(Box::new(vec![StoredMessage {
            id: "m1".into(),
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "<script>alert(1)</script> and <img src=x onerror=alert(2)>".into(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: Some(chrono::Utc::now()),
            tool_duration_ms: None,
            token_usage: None,
        }]));
        let input = SessionExportInput {
            id: "session_xss",
            parent_id: None,
            title: None,
            custom_title: None,
            short_name: None,
            created_at: None,
            updated_at: None,
            model: None,
            provider_key: None,
            working_dir: None,
            status: Box::leak(Box::new(SessionStatus::Active)),
            messages,
            compaction: None,
            related: None,
            raw_session_json: None,
        };
        let html = export_html(&input).unwrap();
        // The literal payload is base64 so the raw text never appears, and the
        // viewer escapes before inserting.
        assert!(!html.contains("<script>alert(1)"));
        assert!(!html.contains("onerror=alert(2)"));
    }
}
