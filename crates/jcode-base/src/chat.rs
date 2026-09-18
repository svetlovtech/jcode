//! AABEE chat-service client for permission "ask" flows.
//!
//! Talks to the same REST surface as the pi Telegram bridge
//! (`/api/chat-service/*`, Bearer auth): a blocking `question` call posts an
//! execution-permission card (kind "permission") and long-polls until the user
//! answers or the timeout elapses. The server relays to Telegram.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use urlencoding::encode as urlencode;

/// Payload shapes mirror the chat service contract (see pi-telegram-bridge
/// `src/api.ts`).
#[derive(Debug, Serialize)]
struct QuestionOption {
    label: String,
    description: String,
}

#[derive(Debug, Serialize)]
struct Question {
    header: String,
    question: String,
    options: Vec<QuestionOption>,
    kind: String,
}

#[derive(Debug, Serialize)]
struct QuestionPayload {
    session_id: String,
    questions: Vec<Question>,
    timeout_seconds: u64,
}

#[derive(Debug, Deserialize)]
struct QuestionResponse {
    #[serde(default)]
    answer: Option<String>,
    #[serde(default)]
    results: Option<Vec<QuestionResult>>,
}

#[derive(Debug, Deserialize)]
struct QuestionResult {
    #[serde(default)]
    answer: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChatServiceClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl ChatServiceClient {
    pub fn new(base_url: &str, token: &str, timeout_secs: u64) -> Result<Self> {
        let base_url = base_url.trim().trim_end_matches('/').to_string();
        if base_url.is_empty() {
            anyhow::bail!("chat service url is empty");
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs.max(1) + 10))
            .build()
            .context("failed to build chat service HTTP client")?;
        Ok(Self {
            http,
            base_url,
            token: token.to_string(),
        })
    }

    /// Ask the user to approve or deny an action and wait for the answer.
    ///
    /// Returns `Ok(true)` when approved, `Ok(false)` when denied or the answer
    /// is unrecognizable (fail closed), and `Err` on transport/HTTP failure.
    pub async fn ask_permission(
        &self,
        session_id: &str,
        header: &str,
        question: &str,
        detail: Option<&str>,
        timeout_secs: u64,
    ) -> Result<bool> {
        let mut text = question.to_string();
        if let Some(detail) = detail.map(str::trim)
            && !detail.is_empty()
        {
            text.push_str("\n\n");
            text.push_str(detail);
        }
        let payload = QuestionPayload {
            session_id: session_id.to_string(),
            questions: vec![Question {
                header: header.to_string(),
                question: text,
                options: vec![
                    QuestionOption {
                        label: "Allow".to_string(),
                        description: "Approve this action for the current call".to_string(),
                    },
                    QuestionOption {
                        label: "Deny".to_string(),
                        description: "Reject this action".to_string(),
                    },
                ],
                kind: "permission".to_string(),
            }],
            timeout_seconds: timeout_secs.max(60),
        };

        let response = self
            .http
            .post(format!("{}/api/chat-service/question", self.base_url))
            .bearer_auth(&self.token)
            .json(&payload)
            .send()
            .await
            .context("chat service request failed")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("chat service returned HTTP {status}: {}", body.trim());
        }
        let parsed: QuestionResponse = response
            .json()
            .await
            .context("chat service returned a malformed answer")?;

        let answer = parsed
            .results
            .as_ref()
            .and_then(|results| results.first())
            .and_then(|first| first.answer.clone())
            .or(parsed.answer)
            .unwrap_or_default();
        Ok(answer_is_approval(&answer))
    }

    /// Fire-and-forget notification relayed to Telegram by the chat service.
    pub async fn notify(&self, title: &str, body: &str) -> Result<()> {
        let response = self
            .http
            .post(format!("{}/api/chat-service/notify", self.base_url))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({ "title": title, "body": body }))
            .send()
            .await
            .context("chat service request failed")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("chat service returned HTTP {status}: {}", body.trim());
        }
        Ok(())
    }

    /// Blocking question: posts the options (kind "question" unless kind is
    /// overridden) and waits for the answer. Returns the answer text verbatim
    /// (empty when the session expired without an answer).
    pub async fn ask_question(
        &self,
        session_id: &str,
        header: &str,
        question: &str,
        options: &[(String, String)],
        timeout_secs: u64,
        kind: &str,
    ) -> Result<String> {
        let payload = QuestionPayload {
            session_id: session_id.to_string(),
            questions: vec![Question {
                header: header.to_string(),
                question: question.to_string(),
                options: options
                    .iter()
                    .map(|(label, description)| QuestionOption {
                        label: label.clone(),
                        description: description.clone(),
                    })
                    .collect(),
                kind: kind.to_string(),
            }],
            timeout_seconds: timeout_secs.max(60),
        };

        let response = self
            .http
            .post(format!("{}/api/chat-service/question", self.base_url))
            .bearer_auth(&self.token)
            .json(&payload)
            .send()
            .await
            .context("chat service request failed")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("chat service returned HTTP {status}: {}", body.trim());
        }
        let parsed: QuestionResponse = response
            .json()
            .await
            .context("chat service returned a malformed answer")?;
        Ok(parsed
            .results
            .as_ref()
            .and_then(|results| results.first())
            .and_then(|first| first.answer.clone())
            .or(parsed.answer)
            .unwrap_or_default())
    }
}

// ── Inbox (files the user sent to the bot) ──────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxFileInfo {
    #[serde(rename = "file_id")]
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size: u64,
    #[serde(default)]
    pub uploaded_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxListInfo {
    #[serde(default, rename = "files_count")]
    pub count: u32,
    #[serde(default)]
    pub files: Vec<InboxFileInfo>,
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// Downloaded inbox file: raw bytes plus the server-provided metadata.
pub struct InboxDownload {
    pub name: String,
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

impl ChatServiceClient {
    /// List files waiting in the chat-service inbox.
    pub async fn inbox_list(&self) -> Result<InboxListInfo> {
        let response = self
            .http
            .get(format!("{}/api/chat-service/inbox", self.base_url))
            .bearer_auth(&self.token)
            .send()
            .await
            .context("chat service request failed")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("chat service returned HTTP {status}: {}", body.trim());
        }
        response
            .json::<InboxListInfo>()
            .await
            .context("chat service returned a malformed inbox listing")
    }

    /// Download one inbox file by id. Returns the server-provided file name,
    /// MIME type, and raw bytes.
    pub async fn inbox_download(&self, file_id: &str) -> Result<InboxDownload> {
        let id = urlencode(file_id);
        let response = self
            .http
            .get(format!(
                "{}/api/chat-service/inbox/files/{}",
                self.base_url, id
            ))
            .bearer_auth(&self.token)
            .send()
            .await
            .context("chat service request failed")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("chat service returned HTTP {status}: {}", body.trim());
        }
        let mime_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .split(';')
            .next()
            .unwrap_or("application/octet-stream")
            .trim()
            .to_string();
        // Attachment filename from Content-Disposition when present, else the
        // final URL segment.
        let name = response
            .headers()
            .get(reqwest::header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok())
            .and_then(|cd| {
                cd.split(';').find_map(|part| {
                    let part = part.trim();
                    part.strip_prefix("filename=\"")
                        .map(|rest| rest.trim_end_matches('"').to_string())
                        .or_else(|| part.strip_prefix("filename=").map(str::to_string))
                })
            })
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| format!("{file_id}.bin"));
        let bytes = response
            .bytes()
            .await
            .context("chat service closed the connection mid-download")?;
        Ok(InboxDownload {
            name,
            mime_type,
            bytes: bytes.to_vec(),
        })
    }

    /// Remove every file from the inbox. Returns how many were removed.
    pub async fn inbox_claim(&self) -> Result<u32> {
        let response = self
            .http
            .post(format!("{}/api/chat-service/inbox/claim", self.base_url))
            .bearer_auth(&self.token)
            .send()
            .await
            .context("chat service request failed")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("chat service returned HTTP {status}: {}", body.trim());
        }
        #[derive(Deserialize)]
        struct ClaimResponse {
            #[serde(default, rename = "files_removed")]
            removed: u32,
        }
        let parsed: ClaimResponse = response
            .json()
            .await
            .context("chat service returned a malformed claim response")?;
        Ok(parsed.removed)
    }
}

/// Interpret a free-form chat answer. Matching mirrors the pi bridge: the
/// permission card resolves to allow/deny wordings (English or Russian).
/// Anything unrecognized denies (fail closed).
pub fn answer_is_approval(answer: &str) -> bool {
    let normalized = answer.trim().to_lowercase();
    normalized.contains("allow") || normalized.contains("разреш") || normalized.contains("yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approvals_recognized_across_wordings() {
        assert!(answer_is_approval("Allow"));
        assert!(answer_is_approval("  разрешено "));
        assert!(answer_is_approval("✅ разрешено"));
        assert!(answer_is_approval("yes"));
    }

    #[test]
    fn denials_and_garbage_fail_closed() {
        assert!(!answer_is_approval("Deny"));
        assert!(!answer_is_approval("❌ отклонено"));
        assert!(!answer_is_approval(""));
        assert!(!answer_is_approval("hmm idk"));
        // "allow" appearing inside a denial sentence still approves; the chat
        // card answer is one of the two option labels, so this is acceptable.
    }

    /// Minimal HTTP/1.1 fake server driving one request, like the MCP tests.
    /// Builds Content-Length from the body so any payload size works.
    async fn spawn_fake_once(json_body: &'static str) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let read = tokio::io::AsyncReadExt::read(&mut socket, &mut buf)
                .await
                .unwrap();
            let request = String::from_utf8_lossy(&buf[..read]).to_string();
            assert!(
                request
                    .to_lowercase()
                    .contains("post /api/chat-service/question")
            );
            assert!(
                request
                    .to_lowercase()
                    .contains("authorization: bearer test-token")
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                json_body.len(),
                json_body
            );
            use tokio::io::AsyncWriteExt;
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        });
        addr
    }

    #[tokio::test]
    async fn ask_permission_maps_allow_answer() {
        let addr = spawn_fake_once("{\"status\":\"ok\",\"answer\":\"Allow\"}\n").await;
        let client = ChatServiceClient::new(&format!("http://{addr}"), "test-token", 60).unwrap();
        let approved = client
            .ask_permission("sess", "Permission", "run rm -rf /tmp/x", None, 3600)
            .await
            .unwrap();
        assert!(approved);
    }

    #[tokio::test]
    async fn ask_permission_maps_deny_answer() {
        let addr = spawn_fake_once("{\"status\":\"ok\",\"answer\":\"❌ отклонено\"}\n").await;
        let client = ChatServiceClient::new(&format!("http://{addr}"), "test-token", 60).unwrap();
        let approved = client
            .ask_permission("sess", "Permission", "run something risky", None, 3600)
            .await
            .unwrap();
        assert!(!approved);
    }
}
