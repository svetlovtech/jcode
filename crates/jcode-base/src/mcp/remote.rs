//! Remote (Streamable HTTP / legacy SSE) MCP transport.
//!
//! Everything wire-specific for non-stdio MCP servers lives here so
//! `client.rs` keeps its upstream shape: the stdio client plus one branch
//! point into this module per concern (connect / is_running / shutdown).

use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::{Mutex, mpsc, oneshot};

use super::client::{
    DEFAULT_MCP_REQUEST_TIMEOUT, McpHandle, correlate_pending, request_timeout_for,
};
use super::protocol::*;

#[cfg(test)]
use super::client::McpClient;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RemoteKind {
    /// Streamable HTTP (MCP 2024-11-05+): POST each message to `url`.
    StreamableHttp,
    /// Legacy HTTP+SSE: GET `url` for server->client frames; the `endpoint`
    /// event tells us where to POST requests.
    Sse,
}

/// State for a remote (HTTP/SSE) MCP connection, shared with the background
/// driver tasks and consulted by `is_running`/`shutdown`.
#[derive(Clone)]
pub(super) struct RemoteTransport {
    pub(super) kind: RemoteKind,
    pub(super) url: String,
    /// Set on shutdown; background tasks check it between reads.
    pub(super) closed: Arc<AtomicBool>,
    /// Streamable HTTP session id (`Mcp-Session-Id` from initialize).
    pub(super) session_id: Arc<std::sync::RwLock<Option<String>>>,
}
/// Remote connections stay alive until explicitly shut down.
pub(super) fn remote_is_running(transport: &RemoteTransport) -> bool {
    !transport.closed.load(Ordering::SeqCst)
}

/// Signal the background tasks to stop and, for streamable HTTP, send a
/// best-effort session teardown DELETE.
pub(super) async fn remote_shutdown(handle: &McpHandle, transport: &RemoteTransport) {
    transport.closed.store(true, Ordering::SeqCst);
    if transport.kind == RemoteKind::StreamableHttp {
        let session = transport
            .session_id
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let mut request = reqwest::Client::new().delete(&transport.url);
        if let Some(session) = &session {
            request = request.header("Mcp-Session-Id", session);
        }
        if let Ok(response) =
            tokio::time::timeout(std::time::Duration::from_secs(2), request.send()).await
        {
            if let Ok(response) = response {
                let _ = response.error_for_status();
            }
        }
    }
    let _ = handle;
}

pub(super) async fn connect_remote(
    name: String,
    config: &McpServerConfig,
) -> Result<(McpHandle, RemoteTransport)> {
    let url = config
        .url
        .clone()
        .filter(|url| !url.trim().is_empty())
        .context("remote MCP server requires a non-empty url")?;
    let kind = match config
        .transport
        .as_deref()
        .map(|t| t.trim().to_ascii_lowercase())
    {
        Some(t) if t == "sse" => RemoteKind::Sse,
        _ => RemoteKind::StreamableHttp,
    };
    crate::logging::info(&format!(
        "MCP: Connecting to '{}' over {} at {}",
        name,
        if kind == RemoteKind::Sse {
            "SSE"
        } else {
            "streamable HTTP"
        },
        url
    ));

    let http = reqwest::Client::builder()
        .timeout(request_timeout_for(config))
        .build()
        .context("failed to build HTTP client for MCP server")?;

    let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let (writer_tx, writer_rx) = mpsc::channel::<String>(32);
    let closed = Arc::new(AtomicBool::new(false));
    let session_id: Arc<std::sync::RwLock<Option<String>>> = Arc::new(std::sync::RwLock::new(None));
    let endpoint: Arc<std::sync::RwLock<Option<String>>> = Arc::new(std::sync::RwLock::new(None));

    if kind == RemoteKind::Sse {
        spawn_sse_stream_task(
            name.clone(),
            http.clone(),
            url.clone(),
            Arc::clone(&closed),
            Arc::clone(&endpoint),
            Arc::clone(&pending),
        );
    }

    spawn_http_driver_task(
        name.clone(),
        http,
        url.clone(),
        kind,
        config.headers.clone(),
        Arc::clone(&closed),
        Arc::clone(&session_id),
        Arc::clone(&endpoint),
        Arc::clone(&pending),
        writer_rx,
    );

    let handle = McpHandle {
        name: name.clone(),
        request_id: Arc::new(AtomicU64::new(1)),
        pending,
        writer_tx,
        server_info: Arc::new(std::sync::RwLock::new(None)),
        capabilities: Arc::new(std::sync::RwLock::new(ServerCapabilities::default())),
        tools: Arc::new(std::sync::RwLock::new(Vec::new())),
        request_timeout: request_timeout_for(config),
    };

    let transport = RemoteTransport {
        kind,
        url,
        closed,
        session_id,
    };

    Ok((handle, transport))
}

/// Get a shareable handle to this client
fn transport_error_response(id: u64, message: String) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: Some(id),
        result: None,
        error: Some(JsonRpcError {
            code: -32603,
            message,
            data: None,
        }),
    }
}

/// Resolve a possibly-relative SSE `endpoint` value against the base URL.
fn resolve_sse_endpoint(base: &str, endpoint: &str) -> Option<String> {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        return Some(endpoint.to_string());
    }
    let base = reqwest::Url::parse(base).ok()?;
    base.join(endpoint).ok().map(|url| url.to_string())
}

/// Parse all complete SSE frames from `buffer`, returning the frames plus the
/// remaining (possibly incomplete) tail. A frame is a block of lines separated
/// by a blank line; only `event:` and `data:` lines are meaningful here.
fn parse_sse_frames(buffer: &str) -> (Vec<(Option<String>, Vec<String>)>, String) {
    let mut frames = Vec::new();
    let mut remainder = buffer;
    while let Some(pos) = remainder.find("\n\n") {
        let (block, rest) = remainder.split_at(pos);
        remainder = &rest[2..];
        let mut event = None;
        let mut data = Vec::new();
        for line in block.lines() {
            if let Some(value) = line.strip_prefix("event:") {
                event = Some(value.trim().to_string());
            } else if let Some(value) = line.strip_prefix("data:") {
                data.push(value.trim().to_string());
            }
        }
        if event.is_some() || !data.is_empty() {
            frames.push((event, data));
        }
    }
    (frames, remainder.to_string())
}

/// Legacy SSE: hold a long-lived GET to `url`, collecting server->client
/// frames. The `endpoint` event names where requests are POSTed; `data`
/// frames carrying JSON responses are correlated into `pending`.
#[allow(clippy::too_many_arguments)]
fn spawn_sse_stream_task(
    name: String,
    http: reqwest::Client,
    url: String,
    closed: Arc<AtomicBool>,
    endpoint: Arc<std::sync::RwLock<Option<String>>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
) {
    tokio::spawn(async move {
        let request = match http
            .get(&url)
            .header("Accept", "text/event-stream")
            .send()
            .await
        {
            Ok(response) => response,
            Err(e) => {
                crate::logging::warn(&format!("MCP [{}] SSE stream error: {}", name, e));
                return;
            }
        };
        let mut stream = request.bytes_stream();
        let mut buffer = String::new();
        use futures::StreamExt;
        loop {
            if closed.load(Ordering::SeqCst) {
                break;
            }
            // Poll with a timeout so shutdown is honored even on idle streams.
            match tokio::time::timeout(std::time::Duration::from_millis(500), stream.next()).await {
                Ok(Some(Ok(chunk))) => {
                    buffer.push_str(&String::from_utf8_lossy(&chunk));
                    let (frames, tail) = parse_sse_frames(&buffer);
                    buffer = tail;
                    for (event, data_lines) in frames {
                        if event.as_deref() == Some("endpoint")
                            && let Some(value) = data_lines.first()
                            && let Some(resolved) = resolve_sse_endpoint(&url, value)
                        {
                            *endpoint
                                .write()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(resolved);
                            continue;
                        }
                        for data in data_lines {
                            if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(&data) {
                                correlate_pending(&pending, response).await;
                            }
                        }
                    }
                }
                Ok(Some(Err(_))) | Ok(None) => break,
                Err(_) => continue, // poll timeout; re-check the closed flag
            }
        }
    });
}

/// Consume newline-terminated JSON messages from `writer_rx` and POST each to
/// the server. Messages are handled concurrently (one task per message) so a
/// long-running tools/call does not block unrelated requests.
#[allow(clippy::too_many_arguments)]
fn spawn_http_driver_task(
    name: String,
    http: reqwest::Client,
    url: String,
    kind: RemoteKind,
    headers: HashMap<String, String>,
    closed: Arc<AtomicBool>,
    session_id: Arc<std::sync::RwLock<Option<String>>>,
    endpoint: Arc<std::sync::RwLock<Option<String>>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    mut writer_rx: mpsc::Receiver<String>,
) {
    tokio::spawn(async move {
        while let Some(msg) = writer_rx.recv().await {
            if closed.load(Ordering::SeqCst) {
                break;
            }
            let task = HttpMessageTask {
                name: name.clone(),
                http: http.clone(),
                url: url.clone(),
                kind,
                headers: headers.clone(),
                closed: Arc::clone(&closed),
                session_id: Arc::clone(&session_id),
                endpoint: Arc::clone(&endpoint),
                pending: Arc::clone(&pending),
                message: msg,
            };
            tokio::spawn(async move {
                task.run().await;
            });
        }
    });
}

/// One outgoing message on a remote transport.
struct HttpMessageTask {
    name: String,
    http: reqwest::Client,
    url: String,
    kind: RemoteKind,
    headers: HashMap<String, String>,
    closed: Arc<AtomicBool>,
    session_id: Arc<std::sync::RwLock<Option<String>>>,
    endpoint: Arc<std::sync::RwLock<Option<String>>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    message: String,
}

impl HttpMessageTask {
    async fn run(self) {
        let body = self.message.trim().to_string();
        let parsed = serde_json::from_str::<Value>(&body).ok();
        let id = parsed
            .as_ref()
            .and_then(|v| v.get("id"))
            .and_then(Value::as_u64);
        let method = parsed
            .as_ref()
            .and_then(|v| v.get("method"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let is_initialize = method.as_deref() == Some("initialize");
        let session = self
            .session_id
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();

        // Legacy SSE requests go to the endpoint advertised by the `endpoint`
        // event; wait (up to the request timeout) for it to become known.
        let target = if self.kind == RemoteKind::Sse {
            let deadline = std::time::Instant::now() + DEFAULT_MCP_REQUEST_TIMEOUT;
            loop {
                if let Some(ep) = self
                    .endpoint
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone()
                {
                    break Some(ep);
                }
                if self.closed.load(Ordering::SeqCst) || std::time::Instant::now() >= deadline {
                    break None;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        } else {
            Some(self.url.clone())
        };
        let Some(target) = target else {
            if let Some(id) = id {
                correlate_pending(
                    &self.pending,
                    transport_error_response(
                        id,
                        "MCP endpoint was not announced before the request timeout".to_string(),
                    ),
                )
                .await;
            }
            return;
        };

        let mut request = self
            .http
            .post(&target)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        for (key, value) in &self.headers {
            request = request.header(key, value);
        }
        if let Some(session) = &session {
            request = request.header("Mcp-Session-Id", session);
        }
        if !is_initialize && session.is_some() {
            request = request.header("Mcp-Protocol-Version", "2024-11-05");
        }
        request = request.body(body);

        let response = match request.send().await {
            Ok(response) => response,
            Err(e) => {
                if let Some(id) = id {
                    correlate_pending(
                        &self.pending,
                        transport_error_response(id, format!("MCP HTTP request failed: {}", e)),
                    )
                    .await;
                }
                return;
            }
        };

        // Streamable HTTP: the initialize response carries the session id that
        // all subsequent requests must echo back.
        if is_initialize
            && let Some(sid) = response
                .headers()
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
        {
            *self
                .session_id
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sid.to_string());
        }

        // Notifications (no id) are fire-and-forget; never touch pending.
        let Some(id) = id else {
            return;
        };

        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let body = response.text().await.unwrap_or_default();

        if status == reqwest::StatusCode::ACCEPTED {
            correlate_pending(
                &self.pending,
                transport_error_response(id, "server returned 202 for a request".to_string()),
            )
            .await;
            return;
        }

        if content_type.contains("text/event-stream") {
            // A single POST response can arrive as multiple SSE data frames.
            let (frames, _) = parse_sse_frames(&format!("{}\n\n", body));
            for (_, data_lines) in frames {
                for data in data_lines {
                    if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(&data) {
                        correlate_pending(&self.pending, response).await;
                    }
                }
            }
        } else if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(&body) {
            correlate_pending(&self.pending, response).await;
        } else {
            crate::logging::debug(&format!(
                "MCP [{}]: non-JSON HTTP response for request id {} (status {}, {} bytes)",
                self.name,
                id,
                status,
                body.len()
            ));
        }
    }
}

#[cfg(all(test, unix))]
#[path = "client_http_tests.rs"]
mod client_http_tests;
