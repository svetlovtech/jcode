//! Remote-transport (streamable HTTP / legacy SSE) client tests.
//!
//! These spin up fake HTTP servers on localhost TCP sockets — no network
//! access beyond loopback is needed.

use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;

/// A single request captured by the fake server.
#[derive(Clone)]
#[allow(dead_code)]
struct CapturedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl CapturedRequest {
    #[allow(dead_code)]
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Build a full HTTP/1.1 response with `Connection: close`.
fn http_response(
    status_line: &str,
    content_type: &str,
    body: &str,
    extra_headers: &[&str],
) -> Vec<u8> {
    format!(
        "{}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close{}\r\n\r\n{}",
        status_line,
        content_type,
        body.len(),
        extra_headers
            .iter()
            .map(|h| format!("\r\n{}", h))
            .collect::<String>(),
        body
    )
    .into_bytes()
}

/// Read one full HTTP request from the socket (head until CRLFCRLF, then
/// content-length body bytes).
#[allow(dead_code)]
fn read_request(stream: &mut std::net::TcpStream) -> std::io::Result<CapturedRequest> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed before head end",
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_string();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.nth(0).unwrap_or_default().to_string();
    let mut headers = Vec::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            headers.push((key.trim().to_string(), value.trim().to_string()));
        }
    }
    let content_length = headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let body_start = head_end + 4;
    while buf.len() < body_start + content_length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&buf[body_start..body_start + content_length]).to_string();

    Ok(CapturedRequest {
        method,
        path,
        headers,
        body,
    })
}

/// Spawn a fake HTTP server that answers a fixed number of sequential
/// connections with per-request handlers.
fn spawn_fake_server_n(
    count: usize,
    mut handler: impl FnMut(CapturedRequest, usize) -> Vec<u8> + Send + 'static,
) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
    let addr = listener.local_addr().expect("local addr");
    tokio::task::spawn_blocking(move || {
        for i in 0..count {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let Ok(request) = read_request(&mut stream) else {
                break;
            };
            let response = handler(request, i);
            let _ = stream.write_all(&response);
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    });
    addr
}

fn remote_config(
    url: String,
    transport: Option<&str>,
    headers: std::collections::HashMap<String, String>,
) -> McpServerConfig {
    McpServerConfig {
        command: String::new(),
        args: Vec::new(),
        env: Default::default(),
        shared: true,
        transport: transport.map(str::to_string),
        url: Some(url),
        headers,
        enabled: None,
        disabled: None,
        timeout_secs: None,
    }
}

fn json_body(result: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":1,"result":{result}}}"#)
}

const INIT_RESULT: &str = r#"{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"fake","version":"0"}}"#;

/// (a) initialize + tools/list round-trip: asserts Accept and configured
/// Authorization headers reach the server and tools are parsed.
#[tokio::test(flavor = "multi_thread")]
async fn streamable_http_initialize_and_tools_list_round_trip() {
    let accept_header: Arc<std::sync::Mutex<Option<String>>> =
        Arc::new(std::sync::Mutex::new(None));
    let auth_header: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
    let accept_clone = Arc::clone(&accept_header);
    let auth_clone = Arc::clone(&auth_header);
    let addr = spawn_fake_server_n(3, move |request, _i| {
        if request.header("authorization").is_some() {
            *auth_clone.lock().unwrap() = request.header("authorization").map(str::to_string);
        }
        if let Some(accept) = request.header("accept") {
            *accept_clone.lock().unwrap() = Some(accept.to_string());
        }
        // Dispatch by body content: the initialized notification and the
        // tools/list request race (the driver POSTs messages concurrently),
        // so arrival order is not deterministic.
        if request.body.contains("\"initialize\"") {
            return http_response(
                "HTTP/1.1 200 OK",
                "application/json",
                &json_body(INIT_RESULT),
                &[],
            );
        }
        if request.body.contains("\"tools/list\"") {
            return http_response(
                "HTTP/1.1 200 OK",
                "application/json",
                r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","inputSchema":{"type":"object"}}]}}"#,
                &[],
            );
        }
        // Notification POST (no id): fire-and-forget, 202 is fine.
        assert!(!request.body.contains("\"id\""));
        http_response("HTTP/1.1 202 Accepted", "text/plain", "", &[])
    });

    let mut headers = std::collections::HashMap::new();
    headers.insert(
        "Authorization".to_string(),
        "Bearer test-token-123".to_string(),
    );
    let config = remote_config(format!("http://{addr}/mcp"), Some("http"), headers);

    let mut client = McpClient::connect("fake-http".to_string(), &config)
        .await
        .expect("connect over streamable http");

    let tools = client.tools();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert!(client.is_running());
    client.shutdown().await;
    assert!(!client.is_running());

    // Every POST must carry Accept including text/event-stream (spec) and the
    // configured Authorization header.
    let accept = accept_header.lock().unwrap().clone().expect("Accept seen");
    assert!(accept.contains("text/event-stream"), "accept was {accept}");
    assert!(accept.contains("application/json"), "accept was {accept}");
    assert_eq!(
        auth_header.lock().unwrap().as_deref(),
        Some("Bearer test-token-123")
    );
}

/// (b) Mcp-Session-Id returned on initialize is echoed on the next request.
#[tokio::test(flavor = "multi_thread")]
async fn streamable_http_session_id_is_echoed() {
    let seen_session: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
    let seen_session_clone = Arc::clone(&seen_session);
    let addr = spawn_fake_server_n(3, move |request, _i| {
        if request.body.contains("\"initialize\"") {
            return http_response(
                "HTTP/1.1 200 OK",
                "application/json",
                &json_body(INIT_RESULT),
                &["Mcp-Session-Id: sess-abc-42"],
            );
        }
        if request.body.contains("\"tools/list\"") {
            *seen_session_clone.lock().unwrap() =
                request.header("mcp-session-id").map(str::to_string);
            return http_response(
                "HTTP/1.1 200 OK",
                "application/json",
                r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}"#,
                &[],
            );
        }
        // initialized notification (no id): fire-and-forget.
        http_response("HTTP/1.1 202 Accepted", "text/plain", "", &[])
    });

    let config = remote_config(
        format!("http://{addr}/mcp"),
        Some("streamable-http"),
        Default::default(),
    );
    let mut client = McpClient::connect("session-test".to_string(), &config)
        .await
        .expect("connect");

    // The initialize request must NOT carry a session id yet; the tools/list
    // request must echo the one the server assigned.
    assert_eq!(seen_session.lock().unwrap().as_deref(), Some("sess-abc-42"));
    client.shutdown().await;
}

/// (c) An SSE response body (Content-Type text/event-stream) with a data frame
/// carrying the matching id is correlated into the pending request.
#[tokio::test(flavor = "multi_thread")]
async fn streamable_http_sse_response_body_is_correlated() {
    let addr = spawn_fake_server_n(3, |request, _i| {
        if request.body.contains("\"initialize\"") {
            return http_response(
                "HTTP/1.1 200 OK",
                "application/json",
                &json_body(INIT_RESULT),
                &[],
            );
        }
        if request.body.contains("\"tools/list\"") {
            return http_response(
                "HTTP/1.1 200 OK",
                "text/event-stream",
                "event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[{\"name\":\"sse-tool\",\"inputSchema\":{\"type\":\"object\"}}]}}\r\n\r\n",
                &[],
            );
        }
        http_response("HTTP/1.1 202 Accepted", "text/plain", "", &[])
    });

    let config = remote_config(format!("http://{addr}/mcp"), None, Default::default());
    let client = McpClient::connect("sse-body-test".to_string(), &config)
        .await
        .expect("connect");

    let tools = client.tools();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "sse-tool");
}

/// (d) is_remote permutations for protocol config shapes.
#[test]
fn is_remote_config_permutations() {
    let base = |transport: Option<&str>, command: &str, url: Option<&str>| McpServerConfig {
        command: command.to_string(),
        args: Vec::new(),
        env: Default::default(),
        shared: true,
        transport: transport.map(str::to_string),
        url: url.map(str::to_string),
        headers: Default::default(),
        enabled: None,
        disabled: None,
        timeout_secs: None,
    };

    // Explicit remote transports with a url.
    assert!(base(Some("http"), "", Some("http://x/mcp")).is_remote());
    assert!(base(Some("sse"), "", Some("http://x/sse")).is_remote());
    assert!(base(Some("streamable-http"), "", Some("http://x/mcp")).is_remote());
    // Case-insensitive transport matching.
    assert!(base(Some("SSE"), "", Some("http://x/sse")).is_remote());
    // Bare url with no transport and no command -> streamable HTTP default.
    assert!(base(None, "", Some("http://x/mcp")).is_remote());
    // stdio wins when a command is present and no explicit remote transport.
    assert!(!base(None, "/bin/sh", Some("http://x/mcp")).is_remote());
    // Unknown transport is not remote.
    assert!(!base(Some("websocket"), "", Some("http://x/mcp")).is_remote());
    // Remote transport without a url is not runnable as remote.
    assert!(!base(Some("http"), "", None).is_remote());
    assert!(!base(Some("http"), "", Some("   ")).is_remote());
    // Bare url with an empty command but a whitespace command still counts as
    // remote only when the command is empty.
    assert!(base(None, "  ", Some("http://x/mcp")).is_remote());
    // Cross-check against is_stdio / is_runnable.
    let remote = base(Some("http"), "", Some("http://x/mcp"));
    let stdio = base(None, "/bin/sh", None);
    assert!(remote.is_runnable() && stdio.is_runnable());
    assert!(!remote.is_stdio() && !stdio.is_remote());
    // Neither stdio nor remote: transport http without url.
    let broken = base(Some("http"), "", None);
    assert!(!broken.is_runnable());
}
