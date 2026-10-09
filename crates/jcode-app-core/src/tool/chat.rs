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
        "Send a Telegram notification to the user. For updates and alerts, not questions."
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
    /// Accepted from the model for backward compatibility but not rendered:
    /// the chat service always supplies its own header. Kept so serde does
    /// not reject legacy inputs that carry it.
    #[serde(default)]
    #[allow(dead_code)]
    header: Option<String>,
    #[serde(default)]
    options: Option<Vec<AskOption>>,
    // Fork: multi-select mode (checkboxes) for legacy single-question shape.
    #[serde(default)]
    multiple: bool,
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
    // Fork: multi-select mode (checkboxes).
    #[serde(default)]
    multiple: bool,
}

struct QuestionSpec {
    header: String,
    text: String,
    options: Vec<(String, String)>,
    // Fork: multi-select mode (checkboxes).
    multiple: bool,
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
                // Fork: carry multi-select flag.
                multiple: q.multiple,
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
        // Fork: carry multi-select flag.
        multiple: params.multiple,
    }])
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user 1-4 questions via Telegram; returns all answers."
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
                            "multiple": {
                                "type": "boolean",
                                "description": "When true, the user may select several options at once (checkboxes); separate labels with '; ' in the answer."
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
                "multiple": {
                    "type": "boolean",
                    "description": "When true, the user may select several options at once (checkboxes); separate labels with '; ' in the answer."
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
        let specs = question_specs(&params)?;
        for spec in &specs {
            if spec.options.len() > 4 {
                return Err(anyhow!("at most 4 options per question are supported"));
            }
        }
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
            // Fresh unique chat-session id per question: rows in
            // chat_question_sessions are never deleted, so reusing an id
            // collides with the unique constraint (issue seen as HTTP 500).
            let ask_session = format!(
                "jcode-ask-{}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
                index
            );
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
                prompt_text.push_str(if spec.multiple {
                    "\nОтветьте цифрами через пробел или своим текстом — ваше следующее сообщение станет ответом."
                } else {
                    "\nОтветь цифрой варианта или своим текстом — ваше следующее сообщение станет ответом."
                });
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
                // Fork: structured ask spec so clients can render native option UIs.
                let ask_spec = jcode_protocol::AskSpec {
                    header: header.clone(),
                    question: spec.text.clone(),
                    options: spec
                        .options
                        .iter()
                        .map(|(label, description)| jcode_protocol::AskOptionSpec {
                            label: label.clone(),
                            description: description.clone(),
                        })
                        .collect(),
                    multiple: spec.multiple,
                    timeout_secs: timeout,
                    question_index: index + 1,
                    question_total: total,
                };
                let _ = stdin_tx.send(crate::tool::StdinInputRequest {
                    request_id: request_id.clone(),
                    prompt: prompt_text,
                    is_password: false,
                    response_tx: answer_tx,
                    source: crate::tool::StdinRequestSource::AskUser,
                    // Fork: carry structured ask spec.
                    ask: Some(ask_spec),
                });

                let tg_session = ask_session.clone();
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
                        .stop_question(&ask_session, Some(&format!("Отвечено в TUI: {answer}")))
                        .await;
                } else {
                    // Fork: Telegram answered first — tell local clients to close
                    // the interactive ask modal and drop the pending stdin
                    // interception; otherwise the dead question stays on screen
                    // until its timeout.
                    crate::bus::Bus::global().publish(crate::bus::BusEvent::AskQuestionResolved {
                        request_id: request_id.clone(),
                        answer: answer.clone(),
                    });
                }

                answers.push(answer.trim().to_string());
                continue;
            }

            // Headless / no TUI channel: Telegram card only.
            let answer = ask_with_stale_recovery(
                &client,
                &ask_session,
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
        "Send a local file or image to the user's Telegram. Images go as photos."
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

    // ── Acceptance-path tests (fork) ─────────────────────────────────────────
    // These drive the REAL AskUserTool::execute through the same dual-surface
    // flow the daemon uses (stdin channel + chat service), with only the
    // external AABEE HTTP endpoint stubbed on a real TCP socket. That stub is
    // the honest external constraint: the production chat service is not
    // reachable from tests.
    //
    // What is real here: AskUserTool::execute (spec building, ASK_USER_LOCK,
    // request_id generation, dual-surface select!, losing-surface handling),
    // the unbounded stdin channel + StdinInputRequest + oneshot reply (the
    // exact wire the daemon forwards to TUI clients), the reqwest HTTP client
    // and QuestionResponse parsing, and the wire AskSpec shape the TUI modal
    // renders. Not covered by these tests: the daemon's socket framing and
    // the TUI render loop (covered by handle_client socket tests and
    // fork_ask_state seam tests respectively).

    /// Serve /api/chat-service/* on a real TCP socket. For "question",
    /// replies `{"answer":"<tg answer>"}`; records every request line.
    /// For other endpoints replies 200 `{"stopped":true}`.
    async fn chat_service_stub()
    -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_task = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let seen = seen_task.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 16384];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]).to_string();
                    seen.lock().unwrap().push(request.clone());
                    let first_line = request.lines().next().unwrap_or("").to_string();
                    let body = if first_line.contains("/question/") {
                        // stop endpoint
                        "{\"stopped\":true}".to_string()
                    } else if first_line.contains("POST /api/chat-service/question") {
                        "{\"answer\":\"из Telegram\"}".to_string()
                    } else {
                        "{}".to_string()
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        (format!("http://{addr}"), seen)
    }

    struct ChatEnvGuard {
        prev_home: Option<std::ffi::OsString>,
        /// Keeps the temp JCODE_HOME alive until drop; the path itself is read
        // only during construction.
        #[allow(dead_code)]
        home: Option<tempfile::TempDir>,
    }
    impl ChatEnvGuard {
        fn new(chat_url: &str) -> Self {
            let prev_home = std::env::var_os("JCODE_HOME");
            let home = tempfile::TempDir::new().expect("temp home");
            std::fs::write(
                home.path().join("config.toml"),
                format!("[chat]\nurl = \"{chat_url}\"\ntoken = \"test-token\"\n"),
            )
            .expect("write config");
            crate::env::set_var("JCODE_HOME", home.path());
            // jcode-base is compiled without cfg(test) here; force the config
            // cache to reload so [chat] pointing at the stub is visible now.
            crate::config::invalidate_config_cache();
            Self {
                prev_home,
                home: Some(home),
            }
        }
    }
    impl Drop for ChatEnvGuard {
        fn drop(&mut self) {
            if let Some(prev) = self.prev_home.take() {
                crate::env::set_var("JCODE_HOME", prev);
            } else {
                crate::env::remove_var("JCODE_HOME");
            }
            crate::config::invalidate_config_cache();
        }
    }

    fn ask_ctx(
        stdin_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::tool::StdinInputRequest>>,
    ) -> ToolContext {
        ToolContext {
            session_id: "session_ask_acceptance".into(),
            message_id: "m".into(),
            tool_call_id: "t".into(),
            working_dir: None,
            stdin_request_tx: stdin_tx,
            graceful_shutdown_signal: None,
            execution_mode: super::super::ToolExecutionMode::AgentTurn,
        }
    }

    #[tokio::test]
    async fn ask_user_tui_answer_wins_and_closes_the_telegram_card() {
        let _env_lock = crate::storage::lock_test_env();
        let (url, seen) = chat_service_stub().await;
        let _guard = ChatEnvGuard::new(&url);

        // Real daemon wiring: tool -> unbounded channel -> (here) the test
        // plays the TUI client: receive the StdinInputRequest, verify the wire
        // ask spec, and reply through the oneshot exactly like
        // Request::StdinResponse would after handle_stdin_response.
        let (stdin_tx, mut stdin_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::tool::StdinInputRequest>();
        let tool = AskUserTool::new();
        let input = json!({
            "question": "Какой вариант部署?",
            "header": "Deploy",
            "options": [
                {"label": "Staging", "description": "test"},
                {"label": "Production"}
            ],
            "timeout_seconds": 60
        });

        let tool_handle = tokio::spawn(async move { tool.execute(input, ask_ctx(Some(stdin_tx))).await });

        let req = tokio::time::timeout(std::time::Duration::from_secs(10), stdin_rx.recv())
            .await
            .expect("stdin request must arrive (TUI surface armed)")
            .expect("stdin channel closed");
        assert_eq!(req.source, crate::tool::StdinRequestSource::AskUser);
        assert!(!req.is_password);
        assert!(req.request_id.starts_with("ask-"), "id was {}", req.request_id);
        let spec = req.ask.expect("structured ask spec must accompany the request");
        assert_eq!(spec.question, "Какой вариант部署?");
        assert_eq!(spec.options.len(), 2);
        assert_eq!(spec.options[0].label, "Staging");
        assert_eq!(spec.timeout_secs, 60);
        assert_eq!(spec.question_total, 1);

        // The user types a free-form answer in the TUI modal.
        req.response_tx
            .send("свой вариант".to_string())
            .expect("oneshot reply");

        let result = tokio::time::timeout(std::time::Duration::from_secs(10), tool_handle)
            .await
            .expect("tool timed out")
            .expect("tool panicked")
            .expect("tool errored");
        assert!(
            result.output.contains("свой вариант"),
            "tool output must carry the typed answer: {}",
            result.output
        );

        // Losing surface: the Telegram card must be closed (stop_question hit).
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let requests = seen.lock().unwrap().clone();
        assert!(
            requests.iter().any(|r| r.contains("/question/stop")),
            "TUI win must close the Telegram card; requests seen: {requests:?}"
        );
    }

    #[tokio::test]
    async fn ask_user_telegram_answer_wins_and_resolves_the_bus() {
        let _env_lock = crate::storage::lock_test_env();
        let (url, _seen) = chat_service_stub().await;
        let _guard = ChatEnvGuard::new(&url);

        let mut bus_rx = crate::bus::Bus::global().subscribe();
        let (stdin_tx, mut stdin_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::tool::StdinInputRequest>();
        let tool = AskUserTool::new();
        let input = json!({
            "question": "Продолжить?",
            "options": [{"label": "Да"}, {"label": "Нет"}],
            "timeout_seconds": 60
        });

        let tool_handle =
            tokio::spawn(async move { tool.execute(input, ask_ctx(Some(stdin_tx))).await });

        // The TUI surface is armed but never answers.
        let req = tokio::time::timeout(std::time::Duration::from_secs(10), stdin_rx.recv())
            .await
            .expect("stdin request must arrive")
            .expect("stdin channel closed");
        assert_eq!(req.source, crate::tool::StdinRequestSource::AskUser);

        // Telegram answers immediately (stub returns "из Telegram").
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), tool_handle)
            .await
            .expect("tool timed out")
            .expect("tool panicked")
            .expect("tool errored");
        assert!(
            result.output.contains("из Telegram"),
            "Telegram answer must win the race: {}",
            result.output
        );

        // The daemon path then publishes AskQuestionResolved so wire clients
        // close their modal (fork fix ad8498c5c). Verify the bus event fires
        // with the winning answer and the TUI request id.
        let resolved = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match bus_rx.recv().await {
                    Ok(crate::bus::BusEvent::AskQuestionResolved { request_id, answer }) => {
                        return (request_id, answer)
                    }
                    Ok(_) => continue,
                    Err(e) => panic!("bus error: {e}"),
                }
            }
        })
        .await
        .expect("AskQuestionResolved must be published when Telegram wins");
        assert_eq!(resolved.0, req.request_id);
        assert_eq!(resolved.1, "из Telegram");
    }
}
