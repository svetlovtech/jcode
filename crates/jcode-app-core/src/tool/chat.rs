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

/// Interactive questions are strictly one-at-a-time: the chat service keeps
/// a single active question per session and a parallel call would collide
/// with HTTP 500. Base tools are shared across sessions, so this guard is
/// process-wide - exactly the semantics a human expects from questions.
static ASK_USER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub struct AskUserTool;

impl AskUserTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize, Clone)]
struct AskOption {
    label: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct AskInput {
    /// Legacy single-question shape.
    #[serde(default)]
    question: Option<String>,
    #[serde(default)]
    header: Option<String>,
    #[serde(default)]
    options: Option<Vec<AskOption>>,
    /// Batch shape: 1-4 questions asked sequentially (Telegram renders each
    /// as a card; TUI prompts appear one by one). The agent receives all
    /// answers in order.
    #[serde(default)]
    questions: Option<Vec<AskQuestionSpec>>,
    /// Overrides [chat] timeout_secs per question (min 60 server-side).
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

#[derive(Deserialize, Clone)]
struct AskQuestionSpec {
    question: String,
    #[serde(default)]
    header: Option<String>,
    #[serde(default)]
    options: Option<Vec<AskOption>>,
}

struct QuestionSpec {
    header: String,
    text: String,
    options: Vec<(String, String)>,
}

/// Normalize the input into an ordered list of questions: either the batch
/// `questions` array or the legacy single-question fields.
fn question_specs(params: &AskInput) -> Result<Vec<QuestionSpec>> {
    if let Some(batch) = &params.questions {
        if batch.is_empty() {
            anyhow::bail!("questions must contain at least one item");
        }
        if batch.len() > 4 {
            anyhow::bail!("at most 4 questions per call");
        }
        return Ok(batch
            .iter()
            .map(|q| QuestionSpec {
                header: q.header.clone().unwrap_or_else(|| "Question".to_string()),
                text: q.question.trim().to_string(),
                options: q
                    .options
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|opt| (opt.label, opt.description.unwrap_or_default()))
                    .collect(),
            })
            .collect());
    }
    let question = params
        .question
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .ok_or_else(|| anyhow!("question is required"))?;
    let options = params
        .options
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|opt| (opt.label, opt.description.unwrap_or_default()))
        .collect();
    Ok(vec![QuestionSpec {
        header: "Question".to_string(),
        text: question.to_string(),
        options,
    }])
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user 1-4 questions via Telegram; blocks until each is answered. Returns all answers."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": {
                                "type": "string",
                                "description": "The complete question."
                            },
                            "header": {
                                "type": "string",
                                "description": "Short topic tag for this question."
                            },
                            "options": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": { "type": "string" },
                                        "description": { "type": "string" }
                                    },
                                    "required": ["label"]
                                },
                                "minItems": 1,
                                "maxItems": 4,
                                "description": "Optional answer options for this question."
                            }
                        },
                        "required": ["question"]
                    },
                    "minItems": 1,
                    "maxItems": 4,
                    "description": "Batch: 1-4 questions asked sequentially."
                },
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

        let timeout = params
            .timeout_seconds
            .unwrap_or_else(|| config().chat.resolved_timeout_secs());
        // Interactive questions are serialized process-wide.
        let _ask_guard = ASK_USER_LOCK.lock().await;

        let specs = question_specs(&params)?;
        let total = specs.len();
        let mut answers: Vec<String> = Vec::new();

        for (index, spec) in specs.iter().enumerate() {
            let header = if total > 1 {
                format!("{} ({} из {})", spec.header, index + 1, total)
            } else {
                spec.header.clone()
            };
            let mut prompt_text = format!("❓ {header}: {}\n", spec.text);
            if !spec.options.is_empty() {
                for (option_index, (label, description)) in spec.options.iter().enumerate() {
                    if description.trim().is_empty() {
                        prompt_text.push_str(&format!("  [{}] {}\n", option_index + 1, label));
                    } else {
                        prompt_text.push_str(&format!(
                            "  [{}] {} — {}\n",
                            option_index + 1,
                            label,
                            description
                        ));
                    }
                }
                prompt_text.push_str(
                    "\nОтветь цифрой варианта или своим текстом — ваше следующее сообщение станет ответом.",
                );
            } else {
                prompt_text.push_str(
                    "\nВаше следующее сообщение станет ответом (или ответьте в Telegram).\n",
                );
            }

            // Dual-surface: when the TUI stdin channel is available, surface the
            // question there AND deliver the Telegram card; first answer wins.
            if let Some(stdin_tx) = ctx.stdin_request_tx.as_ref() {
                use tokio::sync::oneshot;

                let request_id = format!(
                    "ask-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or(0)
                );
                let (answer_tx, answer_rx) = oneshot::channel::<String>();
                let _ = stdin_tx.send(super::StdinInputRequest {
                    request_id: request_id.clone(),
                    prompt: prompt_text,
                    is_password: false,
                    response_tx: answer_tx,
                });

                let tg_session = ctx.session_id.clone();
                let tg_question = spec.text.clone();
                let tg_options = spec.options.clone();
                let tg = {
                    let client = client.clone();
                    tokio::spawn(async move {
                        ask_with_stale_recovery(
                            &client,
                            &tg_session,
                            &header,
                            &tg_question,
                            &tg_options,
                            timeout,
                        )
                        .await
                    })
                };

                tokio::pin!(tg);
                let (answer, surface) = tokio::select! {
                    answer = answer_rx => match answer {
                        Ok(text) => (text, "TUI"),
                        Err(_) => {
                            // Stdin channel closed without an answer; fall back to Telegram.
                            match &mut tg {
                                tg_result => match (&mut **tg_result).await {
                                    Ok(Ok(text)) => (text, "Telegram"),
                                    Ok(Err(e)) => return Err(anyhow!("ask_user failed: {e}")),
                                    Err(e) => return Err(anyhow!("ask_user failed: {e}")),
                                },
                            }
                        }
                    },
                    tg_result = &mut tg => match tg_result {
                        Ok(Ok(text)) => (text, "Telegram"),
                        Ok(Err(e)) => return Err(anyhow!("ask_user failed: {e}")),
                        Err(e) => return Err(anyhow!("ask_user failed: {e}")),
                    },
                };

                let answer = answer.trim().to_string();
                if answer.is_empty() {
                    return Err(anyhow!(
                        "No answer arrived within {timeout}s. Continue with the best default                      assumption and say so, or ask again later."
                    ));
                }

                // The losing surface may still show a pending card/prompt: tell the
                // service to close it so the user sees the resolution.
                if surface == "TUI" {
                    let _ = client
                        .stop_question(&ctx.session_id, Some(&format!("Отвечено в TUI: {answer}")))
                        .await;
                }

                answers.push(answer.trim().to_string());
                continue;
            }

            // Headless / no TUI channel: Telegram card only.
            let answer = ask_with_stale_recovery(
                &client,
                &ctx.session_id,
                &header,
                &spec.text,
                &spec.options,
                timeout,
            )
            .await?;

            let answer = answer.trim().to_string();
            if answer.is_empty() {
                return Err(anyhow!(
                    "No answer arrived within {timeout}s (the question expired). Continue with the \
                 best default assumption and say so, or ask again later."
                ));
            }
        }

        let output = if answers.len() == 1 {
            format!("User answered: {}", answers[0])
        } else {
            let mut text = String::from("Ответы пользователя:\n");
            for (index, spec) in specs.iter().enumerate() {
                let value = answers
                    .get(index)
                    .map(String::as_str)
                    .unwrap_or("(без ответа)");
                text.push_str(&format!("{}. {} → {}\n", index + 1, spec.text, value));
            }
            text
        };

        Ok(ToolOutput::new(output).with_title(format!("ask_user: {total} ответ(ов)")))
    }
}

// ── chat_send ───────────────────────────────────────────────────────────────

/// Ask over the chat service with stale-session recovery: an interrupted turn
/// can leave an old question active for this session, and the service then
/// answers new questions with HTTP 500. Stop the stale question and retry once.
async fn ask_with_stale_recovery(
    client: &crate::chat::ChatServiceClient,
    session_id: &str,
    header: &str,
    question: &str,
    options: &[(String, String)],
    timeout: u64,
) -> Result<String> {
    let mut result = client
        .ask_question(session_id, header, question, options, timeout, "question")
        .await;
    if let Err(err) = &result
        && err.to_string().contains("500")
    {
        crate::logging::warn(&format!(
            "ask_user: HTTP 500 (stale question?) - stopping the stale question and retrying"
        ));
        let _ = client.stop_question(session_id, None).await;
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        result = client
            .ask_question(session_id, header, question, options, timeout, "question")
            .await;
    }
    result
}

pub struct ChatSendTool;

impl ChatSendTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct ChatSendInput {
    path: String,
    #[serde(default)]
    caption: Option<String>,
}

/// Image pixel dimensions for the formats Telegram photo delivery cares
/// about (PNG via IHDR, JPEG via SOF markers). None when unknown.
fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() >= 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") && &bytes[12..16] == b"IHDR" {
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return Some((width, height));
    }
    if bytes.len() >= 4 && bytes[0] == 0xFF && bytes[1] == 0xD8 {
        let mut i = 2;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = bytes[i + 1];
            if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
                let height = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                let width = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                return Some((width, height));
            }
            let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            i += 2 + len;
        }
    }
    None
}

/// Telegram rejects photos whose width+height exceed 10000 or whose size
/// exceeds 10 MB; documents allow 50 MB with no dimension limits. Such
/// images are routed as documents so delivery actually succeeds.
fn should_send_as_document(mime_image: bool, bytes: &[u8]) -> bool {
    if !mime_image {
        return true;
    }
    if bytes.len() > 10 * 1024 * 1024 {
        return true;
    }
    matches!(image_dimensions(bytes), Some((w, h)) if w as u64 + h as u64 > 10_000)
}

#[async_trait]
impl Tool for ChatSendTool {
    fn name(&self) -> &str {
        "chat_send"
    }

    fn description(&self) -> &str {
        "Send a local file or image to the user's Telegram through the chat integration. Images (png/jpg/webp/gif) are delivered as photos, everything else as documents. Use for delivering reports, screenshots, archives and other artifacts the user asked for. Never read credential files to send things manually - this tool already handles delivery."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Local file path (relative paths resolve against the session working directory)."
                },
                "caption": {
                    "type": "string",
                    "description": "Optional caption shown with the file."
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: ChatSendInput = serde_json::from_value(input)?;
        let client = client_from_config()?;

        let path = ctx.resolve_path(std::path::Path::new(&params.path));
        if !path.is_file() {
            return Err(anyhow!("file not found: {}", path.display()));
        }
        let size = std::fs::metadata(&path)?.len();
        if size > 50 * 1024 * 1024 {
            return Err(anyhow!(
                "file is {} MB; the chat service accepts attachments up to 50 MB",
                size / (1024 * 1024)
            ));
        }

        let lower = path.to_string_lossy().to_lowercase();
        let looks_image = ["png", "jpg", "jpeg", "webp", "gif"]
            .iter()
            .any(|ext| lower.ends_with(&format!(".{ext}")));
        let bytes = std::fs::read(&path)?;
        let as_document = should_send_as_document(looks_image, &bytes);
        let endpoint = if looks_image && !as_document {
            "send-photo"
        } else {
            "send-file"
        };

        client
            .send_attachment(endpoint, &path, params.caption.as_deref())
            .await?;
        let note = if as_document && looks_image {
            " (sent as document: exceeds Telegram photo limits)"
        } else {
            ""
        };
        Ok(ToolOutput::new(format!(
            "Uploaded {} to the chat service; queued for Telegram delivery.{note}",
            path.display()
        ))
        .with_title(
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ))
    }
}

/// (tests)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_dimensions_parsed() {
        // Синтетический PNG: подпись + IHDR с width=10664, height=1340.
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 13]); // IHDR length
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&10664u32.to_be_bytes());
        bytes.extend_from_slice(&1340u32.to_be_bytes());
        assert_eq!(image_dimensions(&bytes), Some((10664, 1340)));
        // Ширина+высота > 10000 -> документом.
        assert!(should_send_as_document(true, &bytes));
        // Обычный размер -> фото.
        let mut small = bytes.clone();
        small[16..20].copy_from_slice(&4000u32.to_be_bytes());
        assert!(!should_send_as_document(true, &small));
        // Не-картинка -> всегда документ.
        assert!(should_send_as_document(false, &bytes));
    }

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
