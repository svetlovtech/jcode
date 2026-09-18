//! pi-style permission gate (opt-in via `[permissions]` in config.toml).
//!
//! Evaluated inside `ToolRegistry::execute` before any tool runs:
//!
//! 1. Find the first rule whose tool glob matches the tool name and whose
//!    optional value glob matches the call's primary value (bash command, file
//!    path, URL, MCP target).
//! 2. No match falls back to `default_action` (default: allow).
//! 3. `allow` proceeds, `deny` returns an error to the model, and `ask` asks a
//!    human over the configured chat integration (AABEE chat service, same
//!    REST surface as the pi Telegram bridge) and blocks until an answer
//!    arrives or the timeout elapses. Asks without a chat integration deny
//!    (fail closed).

use crate::config::config;
use crate::config::{PermissionRule, PermissionsConfig};
use serde_json::Value;

/// What the gate decided for one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateOutcome {
    /// Proceed with the call.
    Allow,
    /// Refuse the call; the String is shown to the model.
    Deny(String),
}

/// Evaluate the gate for a tool call. Cheap when disabled.
pub async fn check(session_id: &str, tool_name: &str, input: &Value) -> GateOutcome {
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
) -> GateOutcome {
    match action.to_ascii_lowercase().as_str() {
        "deny" | "blocked" => GateOutcome::Deny(format!(
            "Permission denied by permission {matched}: tool '{tool_name}' is not allowed."
        )),
        "ask" => ask_over_chat(session_id, tool_name, primary_value, permissions, matched).await,
        _ => GateOutcome::Allow,
    }
}

/// Blocking ask over the chat integration. Any transport failure or
/// unrecognized answer denies (fail closed) with an explanatory message.
async fn ask_over_chat(
    session_id: &str,
    tool_name: &str,
    primary_value: &str,
    permissions: &PermissionsConfig,
    matched: &str,
) -> GateOutcome {
    // Permission-scoped chat config wins; otherwise fall back to the global
    // [chat] integration so one section serves both tools and permission asks.
    let global_chat = &crate::config::config().chat;
    let (url, token, timeout_secs) = match permissions
        .chat
        .as_ref()
        .filter(|c| !c.url.trim().is_empty())
    {
        Some(chat) => (
            chat.url.clone(),
            permissions.chat_token(),
            permissions.chat_timeout_secs(),
        ),
        None if global_chat.is_configured() => (
            global_chat.url.clone(),
            global_chat.resolved_token(),
            global_chat.resolved_timeout_secs(),
        ),
        _ => {
            return GateOutcome::Deny(format!(
                "Permission denied: tool '{tool_name}' requires human approval (matched {matched}) \
                 but no chat integration is configured. Add [chat] or [permissions.chat] to \
                 config.toml, or the user can pre-approve this action with an allow rule."
            ));
        }
    };

    let Some(token) = token else {
        return GateOutcome::Deny(format!(
            "Permission denied: chat integration has no bearer token (set token_env) for tool \
             '{tool_name}'."
        ));
    };

    let client = match crate::chat::ChatServiceClient::new(&url, &token, timeout_secs) {
        Ok(client) => client,
        Err(e) => {
            return GateOutcome::Deny(format!(
                "Permission denied: chat integration unusable for tool '{tool_name}': {e}"
            ));
        }
    };

    let question = format!("Allow tool '{tool_name}'?");
    let detail = (!primary_value.is_empty()).then(|| primary_value.to_string());
    crate::logging::info(&format!(
        "Permissions: asking over chat for tool '{tool_name}' (matched {matched}), timeout {timeout_secs}s"
    ));
    match client
        .ask_permission(
            session_id,
            "Permission",
            &question,
            detail.as_deref(),
            timeout_secs,
        )
        .await
    {
        Ok(true) => {
            crate::logging::info(&format!("Permissions: chat approved tool '{tool_name}'"));
            GateOutcome::Allow
        }
        Ok(false) => GateOutcome::Deny(format!(
            "Permission denied: the user rejected tool '{tool_name}' (matched {matched}) over chat."
        )),
        Err(e) => GateOutcome::Deny(format!(
            "Permission denied: could not reach the chat integration for tool '{tool_name}': {e}. \
             Ask the user to answer the pending question or adjust [permissions]."
        )),
    }
}

/// First rule whose tool glob and (optional) value glob both match.
fn matching_rule<'a>(
    permissions: &'a PermissionsConfig,
    tool_name: &str,
    primary_value: &str,
) -> Option<&'a PermissionRule> {
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

/// The decision-relevant value of a call: the bash command, file path, URL, or
/// MCP target. Falls back to a compact JSON dump so pattern rules can still
/// match unusual tools.
fn primary_value(tool_name: &str, input: &Value) -> String {
    for key in ["command", "path", "file_path", "url", "target", "query"] {
        if let Some(value) = input.get(key).and_then(Value::as_str) {
            return value.trim().to_string();
        }
    }
    if tool_name.starts_with("mcp:") {
        return tool_name.to_string();
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

    #[tokio::test]
    async fn disabled_gate_allows_everything() {
        // The shared process config is loaded once; the default (and any real
        // dev config on this machine) may or may not enable permissions, but
        // `check` must never panic and must return a decision.
        let outcome = check("sess", "bash", &serde_json::json!({"command": "ls"})).await;
        assert!(matches!(outcome, GateOutcome::Allow | GateOutcome::Deny(_)));
    }
}
