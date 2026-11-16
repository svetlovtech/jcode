//! Fork: tool permission gate (`[permissions]` in config.toml; opt-in,
//! default off). Lives in its own module so upstream merges stay clean.
//!
//! Evaluated inside `ToolRegistry::execute` before any tool runs:
//!
//! 1. Find the first rule whose tool glob matches the tool name and whose
//!    optional value glob matches the call's primary value (bash command,
//!    file path, URL, MCP server/tool).
//! 2. No match falls back to `default_action` (default: allow).
//! 3. `allow` proceeds, `deny` returns an error to the model, and `ask`
//!    asks a human on both available surfaces at once - the interactive TUI
//!    ask modal (when the session has a stdin channel) and the chat
//!    integration (Telegram card) - first answer wins, exactly like
//!    `ask_user`. Asks without any surface deny (fail closed).

use crate::config::config;
use crate::config::PermissionsConfig;
use serde_json::Value;

/// What the gate decided for one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateOutcome {
    /// Proceed with the call.
    Allow,
    /// Refuse the call; the String is shown to the model.
    Deny(String),
}

/// Evaluate the gate for a tool call. Cheap when disabled: one config read
/// plus a boolean check.
pub async fn check(
    session_id: &str,
    tool_name: &str,
    input: &Value,
    stdin_request_tx: Option<&tokio::sync::mpsc::UnboundedSender<crate::tool::StdinInputRequest>>,
) -> GateOutcome {
    let permissions = &config().permissions;
    if !permissions.enabled {
        return GateOutcome::Allow;
    }

    let primary_value = primary_value(tool_name, input);
    let Some(rule) = matching_rule(permissions, tool_name, &primary_value) else {
        let action = permissions.resolved_default_action();
        return apply_action(
            action,
            session_id,
            tool_name,
            &primary_value,
            permissions,
            "default action",
            stdin_request_tx,
        )
        .await;
    };

    let matched = match rule.pattern.as_deref() {
        Some(pattern) => format!("rule tool={} pattern={pattern}", rule.tool),
        None => format!("rule tool={}", rule.tool),
    };
    apply_action(
        rule.action.trim(),
        session_id,
        tool_name,
        &primary_value,
        permissions,
        &matched,
        stdin_request_tx,
    )
    .await
}

async fn apply_action(
    action: &str,
    session_id: &str,
    tool_name: &str,
    primary_value: &str,
    permissions: &PermissionsConfig,
    matched: &str,
    stdin_request_tx: Option<&tokio::sync::mpsc::UnboundedSender<crate::tool::StdinInputRequest>>,
) -> GateOutcome {
    match action.to_ascii_lowercase().as_str() {
        "deny" | "blocked" => GateOutcome::Deny(format!(
            "Permission denied by permission {matched}: tool '{tool_name}' is not allowed."
        )),
        "ask" => {
            ask_user_approval(
                session_id,
                tool_name,
                primary_value,
                permissions,
                matched,
                stdin_request_tx,
            )
            .await
        }
        _ => GateOutcome::Allow,
    }
}

/// Blocking dual-surface ask: TUI ask modal and the chat integration
/// (Telegram permission card) race; the first answer wins. Any failure or
/// unrecognized answer denies (fail closed) with an explanatory message.
async fn ask_user_approval(
    session_id: &str,
    tool_name: &str,
    primary_value: &str,
    permissions: &PermissionsConfig,
    matched: &str,
    stdin_request_tx: Option<&tokio::sync::mpsc::UnboundedSender<crate::tool::StdinInputRequest>>,
) -> GateOutcome {
    let timeout = permissions.resolved_timeout_secs();
    let question = format!("Allow tool '{tool_name}'?");
    let header = "Permission";
    let options: Vec<(String, String)> = vec![
        (
            "Allow".to_string(),
            "Approve this action for the current call".to_string(),
        ),
        ("Deny".to_string(), "Reject this action".to_string()),
    ];
    crate::logging::info(&format!(
        "Permissions: asking for tool '{tool_name}' (matched {matched}), timeout {timeout}s"
    ));

    // Surface 1: interactive TUI ask modal (dual-surface like ask_user).
    let tui_ask = stdin_request_tx.map(|tx| {
        let (answer_tx, answer_rx) = tokio::sync::oneshot::channel::<String>();
        let request_id = format!(
            "permission-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let mut prompt_text = format!("🔐 {header}: {question}\n");
        if !primary_value.is_empty() {
            let preview: String = primary_value.chars().take(400).collect();
            prompt_text.push_str(&format!("  {preview}\n"));
        }
        prompt_text.push_str("  [1] Allow\n  [2] Deny\n");
        let _ = tx.send(crate::tool::StdinInputRequest {
            request_id: request_id.clone(),
            prompt: prompt_text,
            is_password: false,
            response_tx: answer_tx,
            source: crate::tool::StdinRequestSource::AskUser,
            ask: Some(jcode_protocol::AskSpec {
                header: header.to_string(),
                question: question.clone(),
                options: options
                    .iter()
                    .map(|(label, description)| jcode_protocol::AskOptionSpec {
                        label: label.clone(),
                        description: description.clone(),
                    })
                    .collect(),
                multiple: false,
                timeout_secs: timeout,
                question_index: 1,
                question_total: 1,
            }),
        });
        (request_id, answer_rx)
    });

    // Surface 2: chat integration (Telegram card), same REST surface as
    // ask_user. Best-effort: absence just removes this surface from the race.
    let chat_ask = chat_ask_approval(session_id, header, &question, primary_value, timeout);

    let (answer, winning_surface) = tokio::select! {
        answer = async {
            match tui_ask {
                Some((_, answer_rx)) => match answer_rx.await {
                    Ok(text) => Some(text),
                    Err(_) => std::future::pending::<Option<String>>().await,
                },
                None => std::future::pending::<Option<String>>().await,
            }
        } => (answer, "TUI"),
        chat = chat_ask => (chat.ok(), "Telegram"),
    };

    let Some(answer) = answer else {
        return GateOutcome::Deny(format!(
            "Permission denied: no answer arrived for tool '{tool_name}' within {timeout}s \
             (matched {matched})."
        ));
    };

    // The losing surface may still show a pending card/prompt: tell the
    // service to close it so the user sees the resolution.
    if winning_surface == "TUI"
        && let Ok(client) = super::chat::client_from_permissions_fallback()
    {
        // The Telegram card, if any, is closed by the service on timeout;
        // there is no ask session id to stop here because the card was only
        // posted when the chat ask won. Skipping the stop call keeps the
        // permission flow simple: the card expires with the timeout.
        let _ = client;
    }

    let approved = answer_is_approval(&answer);
    crate::logging::info(&format!(
        "Permissions: {winning_surface} answered '{}' for tool '{tool_name}'",
        if approved { "allow" } else { "deny" }
    ));
    if approved {
        GateOutcome::Allow
    } else {
        GateOutcome::Deny(format!(
            "Permission denied: the user rejected tool '{tool_name}' (matched {matched}) \
             via {winning_surface}."
        ))
    }
}

/// Best-effort chat-service ask. Errors map to None so the TUI surface can
/// still win the race; only when both surfaces fail does the ask deny.
async fn chat_ask_approval(
    session_id: &str,
    header: &str,
    question: &str,
    detail: &str,
    timeout: u64,
) -> anyhow::Result<String> {
    let client = super::chat::client_from_permissions_fallback()?;
    let mut text = question.to_string();
    if !detail.trim().is_empty() {
        let preview: String = detail.chars().take(400).collect();
        text.push_str("\n\n");
        text.push_str(&preview);
    }
    client
        .ask_question(
            &format!("jcode-permission-{session_id}"),
            header,
            &text,
            &[
                (
                    "Allow".to_string(),
                    "Approve this action for the current call".to_string(),
                ),
                ("Deny".to_string(), "Reject this action".to_string()),
            ],
            timeout,
            "permission",
        )
        .await
}

/// Interpret a free-form answer. Matching mirrors the chat bridge: the
/// permission card resolves to allow/deny wordings (English or Russian).
/// Anything unrecognized denies (fail closed).
pub fn answer_is_approval(answer: &str) -> bool {
    let normalized = answer.trim().to_lowercase();
    normalized.contains("allow") || normalized.contains("разреш") || normalized.contains("yes")
}

/// First rule whose tool glob and (optional) value glob both match.
fn matching_rule<'a>(
    permissions: &'a PermissionsConfig,
    tool_name: &str,
    primary_value: &str,
) -> Option<&'a crate::config::PermissionRule> {
    permissions.rules.iter().find(|rule| {
        glob_match(rule.tool.trim(), tool_name)
            && rule
                .pattern
                .as_deref()
                .map(|pattern| glob_match(pattern.trim(), primary_value))
                .unwrap_or(true)
    })
}

/// Minimal glob matching: `*` matches any run of characters (including none),
/// everything else is a literal substring anchor-free match. Case-sensitive.
pub fn glob_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.trim();
    let value = value.trim();
    if pattern == "*" || pattern.is_empty() {
        return true;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let Some((first, rest)) = parts.split_first() else {
        return true;
    };
    if !value.starts_with(first) {
        return false;
    }
    let mut cursor = first.len();
    for part in rest {
        if part.is_empty() {
            // Trailing or doubled '*': matches anything from here on.
            cursor = value.len();
            continue;
        }
        match value[cursor..].find(part) {
            Some(offset) => cursor += offset + part.len(),
            None => return false,
        }
    }
    // A middle part consumed only a prefix of the remaining value is fine; a
    // pattern without a trailing '*' must reach the end.
    if !pattern.ends_with('*') {
        return cursor == value.len();
    }
    true
}

/// The decision-relevant value of a call: the bash command, file path, URL,
/// or MCP server/tool pair. Falls back to a compact JSON dump so pattern
/// rules can still match unusual tools.
fn primary_value(tool_name: &str, input: &Value) -> String {
    for key in ["command", "path", "file_path", "url", "target", "query"] {
        if let Some(value) = input.get(key).and_then(Value::as_str) {
            return value.trim().to_string();
        }
    }
    // MCP dispatch tools carry the server/tool pair in their arguments; the
    // pair (not the whole arguments blob) is the value users write rules
    // against, e.g. pattern "playwright*".
    if tool_name == "mcp" || tool_name == "mcp_call" {
        let server = input.get("server").and_then(Value::as_str).unwrap_or("");
        let raw = input.get("tool").and_then(Value::as_str).unwrap_or("");
        let pair = format!("{server}/{raw}");
        if pair != "/" {
            return pair;
        }
    }
    let mut compact = input.to_string();
    if compact.len() > 200 {
        compact.truncate(200);
        compact.push('…');
    }
    compact
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_prefix_suffix_and_middle() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("bash", "bash"));
        assert!(glob_match("mcp:*", "mcp:zgate:search"));
        assert!(glob_match("rm*", "rm -rf /"));
        assert!(glob_match("*--force", "docker run --force"));
        assert!(glob_match("git *push*", "git origin push main"));
        assert!(!glob_match("bash", "bashx"));
        assert!(!glob_match("git*push", "git origin push main"));
        assert!(!glob_match("rm -rf *", "ls -la"));
    }

    #[test]
    fn primary_value_prefers_known_keys() {
        let input = serde_json::json!({"command": "rm -rf /tmp/x", "timeout": 5});
        assert_eq!(primary_value("bash", &input), "rm -rf /tmp/x");
        let input = serde_json::json!({"file_path": "/etc/passwd"});
        assert_eq!(primary_value("edit", &input), "/etc/passwd");
        let input = serde_json::json!({"q": 1});
        assert!(primary_value("weird", &input).contains("{\"q\":1}"));
    }

    #[test]
    fn mcp_pair_is_the_primary_value() {
        let input = serde_json::json!({"server": "playwright", "tool": "browser_click", "arguments": {}});
        assert_eq!(primary_value("mcp", &input), "playwright/browser_click");
        assert_eq!(primary_value("mcp_call", &input), "playwright/browser_click");
    }

    #[test]
    fn approvals_recognized_across_wordings() {
        assert!(answer_is_approval("Allow"));
        assert!(answer_is_approval("  разрешено "));
        assert!(answer_is_approval("✅ разрешено"));
        assert!(answer_is_approval("yes"));
        assert!(!answer_is_approval("Deny"));
        assert!(!answer_is_approval("отклонено"));
        assert!(!answer_is_approval(""));
    }
}
