//! Inbox tools: let the agent read files and images the user sent to the
//! chat-service inbox (pi-bridge parity: inbox list / read / claim).
//!
//! Files are downloaded into `<jcode-dir>/inbox/` so the path stays valid for
//! later bash/render tools. Images are additionally attached to the tool
//! result so vision-capable models see the picture directly.

use super::{Tool, ToolContext, ToolOutput};
use crate::config::config;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};

const TEXT_PREVIEW_CHARS: usize = 2000;

fn client_from_config() -> Result<crate::chat::ChatServiceClient> {
    let chat = &config().chat;
    if !chat.is_configured() {
        return Err(anyhow!(
            "Chat integration is not configured. Add a [chat] section to ~/.jcode/config.toml \
             with url and token_env (the AABEE chat service)."
        ));
    }
    let Some(token) = chat.resolved_token() else {
        return Err(anyhow!(
            "Chat integration has no bearer token: set chat.token_env in ~/.jcode/config.toml."
        ));
    };
    crate::chat::ChatServiceClient::new(&chat.url, &token, chat.resolved_timeout_secs())
}

fn inbox_dir() -> std::path::PathBuf {
    crate::storage::jcode_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("inbox")
}

fn safe_component(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() || clean == "." || clean == ".." {
        "file".to_string()
    } else {
        clean
    }
}

fn is_image(mime: &str) -> bool {
    mime.starts_with("image/")
}

fn is_textual(mime: &str, name: &str) -> bool {
    mime.starts_with("text/")
        || mime.contains("json")
        || mime.contains("xml")
        || mime.contains("yaml")
        || mime.contains("toml")
        || name.ends_with(".md")
        || name.ends_with(".rs")
        || name.ends_with(".py")
        || name.ends_with(".go")
        || name.ends_with(".ts")
        || name.ends_with(".js")
        || name.ends_with(".sh")
}

// ── inbox_list ──────────────────────────────────────────────────────────────

pub struct InboxListTool;

impl InboxListTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for InboxListTool {
    fn name(&self) -> &str {
        "inbox_list"
    }

    fn description(&self) -> &str {
        "List files and images the user sent to the chat inbox (Telegram). Use this first when the user says they sent you something. Returns file ids needed by inbox_read."
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn execute(&self, _input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let client = client_from_config()?;
        let info = client.inbox_list().await?;
        if info.files.is_empty() {
            return Ok(ToolOutput::new("Inbox is empty.".to_string()).with_title("inbox: empty"));
        }
        let mut text = format!("Inbox: {} file(s)\n", info.files.len());
        for file in &info.files {
            text.push_str(&format!(
                "- {} (file_id: {}, {}, {} bytes, uploaded {})\n",
                file.name, file.id, file.mime_type, file.size, file.uploaded_at
            ));
        }
        if let Some(expires) = &info.expires_at {
            text.push_str(&format!("Inbox expires at: {expires}\n"));
        }
        Ok(ToolOutput::new(text).with_title(format!("inbox: {} file(s)", info.files.len())))
    }
}

// ── inbox_read ──────────────────────────────────────────────────────────────

pub struct InboxReadTool;

impl InboxReadTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct InboxReadInput {
    file_id: String,
}

#[async_trait]
impl Tool for InboxReadTool {
    fn name(&self) -> &str {
        "inbox_read"
    }

    fn description(&self) -> &str {
        "Download one inbox file by file_id (from inbox_list). Images are attached to the result so you can see them; text files return a preview plus the saved local path. The file is saved under the jcode inbox directory for later use."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_id": {
                    "type": "string",
                    "description": "file_id from inbox_list."
                }
            },
            "required": ["file_id"]
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: InboxReadInput = serde_json::from_value(input)?;
        let client = client_from_config()?;
        let download = client.inbox_download(&params.file_id).await?;

        let dir = inbox_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| anyhow!("cannot create inbox dir {}: {e}", dir.display()))?;
        let file_name = safe_component(&download.name);
        let path = dir.join(format!("{}_{}", safe_component(&params.file_id), file_name));
        std::fs::write(&path, &download.bytes)
            .map_err(|e| anyhow!("cannot write {}: {e}", path.display()))?;

        let mut text = format!(
            "Saved: {} ({} bytes, {})\n",
            path.display(),
            download.bytes.len(),
            download.mime_type
        );

        let mut output = ToolOutput::new(text.clone())
            .with_title(format!("inbox: {}", download.name))
            .with_metadata(json!({
                "path": path.display().to_string(),
                "mime_type": download.mime_type,
                "size": download.bytes.len(),
            }));

        if is_image(&download.mime_type) {
            // Attach so vision models see the picture inline.
            let media_type = download
                .mime_type
                .split(';')
                .next()
                .unwrap_or("image/png")
                .trim()
                .to_string();
            output.images.push(jcode_tool_types::ToolImage {
                media_type,
                data: base64::engine::general_purpose::STANDARD.encode(&download.bytes),
                label: Some(download.name.clone()),
            });
            text.push_str("The image is attached to this result - you can see it directly.\n");
            output.output = text;
        } else if is_textual(&download.mime_type, &download.name) {
            let content = String::from_utf8_lossy(&download.bytes);
            let preview: String = content.chars().take(TEXT_PREVIEW_CHARS).collect();
            text.push_str(&format!(
                "\n--- preview (first {TEXT_PREVIEW_CHARS} chars) ---\n{preview}\n"
            ));
            if content.chars().count() > TEXT_PREVIEW_CHARS {
                text.push_str("... (truncated; read the saved file for the rest)\n");
            }
            output.output = text;
        } else {
            text.push_str("Binary file: use the saved path with bash or other tools.\n");
            output.output = text;
        }

        Ok(output)
    }
}

// ── inbox_claim ─────────────────────────────────────────────────────────────

pub struct InboxClaimTool;

impl InboxClaimTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for InboxClaimTool {
    fn name(&self) -> &str {
        "inbox_claim"
    }

    fn description(&self) -> &str {
        "Delete ALL files from the chat inbox (destructive). Only use after the user confirmed the files were processed or explicitly asked to clear the inbox. Returns how many files were removed."
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn execute(&self, _input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let client = client_from_config()?;
        let removed = client.inbox_claim().await?;
        Ok(
            ToolOutput::new(format!("Removed {removed} file(s) from the inbox."))
                .with_title(format!("inbox cleared ({removed})")),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_component_blocks_traversal() {
        // Dots survive (file extensions), slashes do not - no traversal.
        assert_eq!(safe_component("../../etc/passwd"), ".._.._etc_passwd");
        assert!(!safe_component("../../etc/passwd").contains('/'));
        assert_eq!(safe_component(".."), "file");
        assert_eq!(safe_component("report v2.pdf"), "report_v2.pdf");
    }

    #[test]
    fn textual_detection() {
        assert!(is_textual("text/plain", "a.txt"));
        assert!(is_textual("application/json", "b.json"));
        assert!(is_textual("application/octet-stream", "c.rs"));
        assert!(!is_textual("application/octet-stream", "d.bin"));
        assert!(is_image("image/png"));
        assert!(!is_image("text/plain"));
    }

    #[test]
    fn schemas_have_file_id() {
        assert!(
            InboxReadTool::new().parameters_schema()["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "file_id")
        );
    }
}
