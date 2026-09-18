//! AABEE chat integration tools: let the agent send Telegram notifications
//! and ask the user blocking questions with options (pi-bridge parity).
//!
//! Configured via `[chat]` in config.toml (url + token_env). With no config
//! both tools return an explanatory error instead of silently doing nothing.

use super::{Tool, ToolContext, ToolOutput};
use crate::config::config;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

/// Resolve a client from the global `[chat]` section, or explain what to add.
fn client_from_config() -> Result<crate::chat::ChatServiceClient> {
    let chat = &config().chat;
    if !chat.is_configured() {
        return Err(anyhow!(
            "Chat integration is not configured. Ask the user to add a [chat] section to \
             ~/.jcode/config.toml with url and token_env (the AABEE chat service)."
        ));
    }
    let Some(token) = chat.resolved_token() else {
        return Err(anyhow!(
            "Chat integration has no bearer token: set chat.token_env (e.g. \
             OPENCODE_CHAT_SERVICE_TOKEN) in ~/.jcode/config.toml."
        ));
    };
    crate::chat::ChatServiceClient::new(&chat.url, &token, chat.resolved_timeout_secs())
}

// ── chat_notify ─────────────────────────────────────────────────────────────

pub struct ChatNotifyTool;

impl ChatNotifyTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct NotifyInput {
    title: String,
    #[serde(default)]
    body: Option<String>,
}

#[async_trait]
impl Tool for ChatNotifyTool {
    fn name(&self) -> &str {
        "chat_notify"
    }

    fn description(&self) -> &str {
        "Send a notification to the user's Telegram through the chat integration. Use for progress updates, alerts, and completion messages - never for questions that need an answer (use ask_user for those)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short notification title."
                },
                "body": {
                    "type": "string",
                    "description": "Notification text (plain text, no markdown escaping needed)."
                }
            },
            "required": ["title"]
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: NotifyInput = serde_json::from_value(input)?;
        let client = client_from_config()?;
        let body = params.body.unwrap_or_default();
        client.notify(&params.title, &body).await?;
        Ok(ToolOutput::new("Notification delivered.".to_string())
            .with_title(format!(
                "notified: {}",
                params.title.chars().take(40).collect::<String>()
            ))
            .with_metadata(serde_json::json!({ "session_id": ctx.session_id })))
    }
}

// ── ask_user ────────────────────────────────────────────────────────────────

pub struct AskUserTool;

impl AskUserTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct AskOption {
    label: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct AskInput {
    question: String,
    #[serde(default)]
    header: Option<String>,
    #[serde(default)]
    options: Option<Vec<AskOption>>,
    /// Overrides [chat] timeout_secs for this question (min 60 server-side).
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user a question with answer options through the chat integration (Telegram) and block until they answer. Use when a decision is needed to proceed. Without options the user can type a free-form answer. The user may not answer for minutes; the call waits up to the timeout."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The complete question, specific and ending with a question mark."
                },
                "header": {
                    "type": "string",
                    "description": "Very short topic tag shown next to the question (max ~16 chars)."
                },
                "options": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "label": {
                                "type": "string",
                                "description": "Concise option text (1-5 words)."
                            },
                            "description": {
                                "type": "string",
                                "description": "What this option means / its trade-offs."
                            }
                        },
                        "required": ["label"]
                    },
                    "minItems": 1,
                    "maxItems": 4,
                    "description": "1-4 mutually exclusive options. Omit for a free-form answer."
                },
                "timeout_seconds": {
                    "type": "integer",
                    "minimum": 60,
                    "description": "How long to wait for the answer. Defaults to chat.timeout_secs (600)."
                }
            },
            "required": ["question"]
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: AskInput = serde_json::from_value(input)?;
        let client = client_from_config()?;

        let options: Vec<(String, String)> = params
            .options
            .unwrap_or_default()
            .into_iter()
            .map(|opt| (opt.label, opt.description.unwrap_or_default()))
            .collect();
        if options.len() > 4 {
            return Err(anyhow!("at most 4 options are supported"));
        }

        let timeout = params
            .timeout_seconds
            .unwrap_or_else(|| config().chat.resolved_timeout_secs());
        let header = params
            .header
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .unwrap_or("Question");

        let answer = client
            .ask_question(
                &ctx.session_id,
                header,
                &params.question,
                &options,
                timeout,
                "question",
            )
            .await?;

        if answer.trim().is_empty() {
            return Err(anyhow!(
                "No answer arrived within {timeout}s (the question expired). Continue with the \
                 best default assumption and say so, or ask again later."
            ));
        }

        Ok(ToolOutput::new(format!("User answered: {answer}")).with_title("ask_user answered"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_shape() {
        let notify = ChatNotifyTool::new().parameters_schema();
        assert!(
            notify["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "title")
        );

        let ask = AskUserTool::new().parameters_schema();
        assert!(
            ask["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "question")
        );
        assert_eq!(ask["properties"]["options"]["maxItems"], 4);
    }

    #[tokio::test]
    async fn ask_rejects_more_than_four_options() {
        let tool = AskUserTool::new();
        let input = json!({
            "question": "pick one",
            "options": [
                {"label": "a"}, {"label": "b"}, {"label": "c"}, {"label": "d"}, {"label": "e"}
            ]
        });
        let ctx = ToolContext {
            session_id: "s".into(),
            message_id: "m".into(),
            tool_call_id: "t".into(),
            working_dir: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: super::super::ToolExecutionMode::AgentTurn,
        };
        let err = tool.execute(input, ctx).await;
        assert!(err.is_err());
    }
}
