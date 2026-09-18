//! Goal contract tools (pi-style `/goal`): let the agent complete, block, or
//! wait on the session's active goal contract.
//!
//! State lives in `jcode_base::goal_contract`, keyed by session id — the same
//! registry the per-turn prompt injection reads. Each tool requires the
//! current `goal_id` so a stale contract reference can never mutate a
//! replacement goal.

use super::{Tool, ToolContext, ToolOutput};
use crate::goal_contract::{self, GoalContractStatus};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct GoalActionInput {
    goal_id: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    evidence: Option<String>,
    #[serde(default)]
    repeated_turns: Option<u32>,
    #[serde(default)]
    required_action: Option<String>,
    #[serde(default)]
    resume_after_ms: Option<u64>,
}

fn require<'a>(value: &'a Option<String>, field: &str) -> Result<&'a str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow::anyhow!("{field} is required"))
}

fn no_active_goal_error() -> anyhow::Error {
    anyhow::anyhow!(
        "No active goal contract for this session (or goal_id mismatch). The user sets a goal with /goal; call this tool only with the goal_id from the Goal Contract prompt block."
    )
}

/// Shared executor so the three tools differ only in status, schema, and the
/// guidance they return to the model.
async fn execute_goal_action<F>(
    ctx: &ToolContext,
    input: Value,
    tool_name: &str,
    validate: F,
) -> Result<ToolOutput>
where
    F: FnOnce(&GoalActionInput) -> Result<(GoalContractStatus, Option<String>, String)>,
{
    let parsed: GoalActionInput = serde_json::from_value(input)?;
    let (status, note, guidance) = validate(&parsed)?;
    match goal_contract::transition(&ctx.session_id, &parsed.goal_id, status, note) {
        Some(updated) => Ok(ToolOutput::new(format!(
            "{guidance}\n\nGoal {} is now {}.",
            updated.goal_id,
            updated.status.as_label()
        ))
        .with_title(format!("{tool_name} {}", updated.goal_id))),
        None => Err(no_active_goal_error()),
    }
}

pub struct GoalCompleteTool;

impl GoalCompleteTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GoalCompleteTool {
    fn name(&self) -> &str {
        "goal_complete"
    }

    fn description(&self) -> &str {
        "Mark the session's active goal as complete. Only call when EVERY requirement of the goal is verified with concrete evidence (tests run, commands succeeded, artifacts inspected). Pass the goal_id from the Goal Contract prompt block. After this call, end the turn and report the completion summary."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "goal_id": {
                    "type": "string",
                    "description": "Exact goal_id from the current Goal Contract."
                },
                "summary": {
                    "type": "string",
                    "description": "What was completed and what evidence verified it."
                }
            },
            "required": ["goal_id", "summary"]
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        execute_goal_action(&ctx, input, self.name(), |parsed| {
            let summary = require(&parsed.summary, "summary")?.to_string();
            Ok((
                GoalContractStatus::Completed,
                Some(summary.clone()),
                "Goal marked complete. End the turn now and give the user the completion summary; do not start unrelated work.".to_string(),
            ))
        })
        .await
    }
}

pub struct GoalBlockedTool;

impl GoalBlockedTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GoalBlockedTool {
    fn name(&self) -> &str {
        "goal_blocked"
    }

    fn description(&self) -> &str {
        "Mark the session's active goal as blocked by an external impediment ONLY after the same user-action blocker has recurred for at least three consecutive turns. Ordinary difficulty, uncertainty, or recoverable failures do not qualify. After this call, end the turn."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "goal_id": {
                    "type": "string",
                    "description": "Exact goal_id from the current Goal Contract."
                },
                "reason": {
                    "type": "string",
                    "description": "The specific user or external action required to unblock the goal."
                },
                "evidence": {
                    "type": "string",
                    "description": "Concrete evidence from the repeated attempts proving the impasse."
                },
                "repeated_turns": {
                    "type": "integer",
                    "minimum": 3,
                    "description": "Number of separate turns spent hitting this same blocker (must be at least 3)."
                },
                "required_action": {
                    "type": "string",
                    "description": "Optional alias for reason; one of the two is required."
                }
            },
            "required": ["goal_id"]
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        execute_goal_action(&ctx, input, self.name(), |parsed| {
            let reason = match (parsed.reason.as_deref().map(str::trim).filter(|v| !v.is_empty()), parsed.required_action.as_deref().map(str::trim).filter(|v| !v.is_empty())) {
                (Some(r), _) => r.to_string(),
                (None, Some(a)) => a.to_string(),
                (None, None) => return Err(anyhow::anyhow!("reason is required")),
            };
            let evidence = parsed.evidence.as_deref().unwrap_or_default().trim();
            let turns = parsed.repeated_turns.unwrap_or(0);
            if turns < 3 {
                return Err(anyhow::anyhow!(
                    "goal_blocked requires the blocker to have recurred for at least 3 consecutive turns; got {turns}. Keep working or use goal_wait for arranged external waits."
                ));
            }
            let note = format!("{reason} (evidence: {evidence})");
            Ok((
                GoalContractStatus::Blocked,
                Some(note),
                "Goal marked blocked. End the turn now and tell the user exactly which external action is required; do not keep retrying.".to_string(),
            ))
        })
        .await
    }
}

pub struct GoalWaitTool;

impl GoalWaitTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GoalWaitTool {
    fn name(&self) -> &str {
        "goal_wait"
    }

    fn description(&self) -> &str {
        "Mark the session's active goal as waiting for an arranged external event (a wake message, a deployment, another process). Call goal_wait alone - never in parallel with other tools. End the turn immediately after."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "goal_id": {
                    "type": "string",
                    "description": "Exact goal_id from the current Goal Contract."
                },
                "reason": {
                    "type": "string",
                    "description": "Why the goal is waiting and which external event should wake it."
                },
                "resume_after_ms": {
                    "type": "integer",
                    "minimum": 10000,
                    "description": "Optional safety deadline in milliseconds after which the goal should be resumed if no wake message arrived."
                }
            },
            "required": ["goal_id", "reason"]
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        execute_goal_action(&ctx, input, self.name(), |parsed| {
            let reason = require(&parsed.reason, "reason")?.to_string();
            let deadline = parsed.resume_after_ms.map(|ms| {
                let clamped = ms.max(10_000);
                format!(" Safety deadline: resume after {clamped} ms if nothing wakes the goal.")
            }).unwrap_or_default();
            Ok((
                GoalContractStatus::Waiting,
                Some(format!("{reason}{deadline}")),
                format!("Goal marked waiting. End the turn now; wait for the external wake event.{deadline}"),
            ))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_require_goal_id() {
        for tool_schema in [
            GoalCompleteTool::new().parameters_schema(),
            GoalBlockedTool::new().parameters_schema(),
            GoalWaitTool::new().parameters_schema(),
        ] {
            let required = tool_schema["required"].as_array().expect("required array");
            assert!(required.iter().any(|v| v == "goal_id"));
        }
    }

    #[tokio::test]
    async fn complete_rejects_missing_summary() {
        let tool = GoalCompleteTool::new();
        let err = tool
            .execute(
                json!({"goal_id": "abc12345"}),
                ToolContext {
                    session_id: "s".into(),
                    message_id: "m".into(),
                    tool_call_id: "t".into(),
                    working_dir: None,
                    stdin_request_tx: None,
                    graceful_shutdown_signal: None,
                    execution_mode: super::super::ToolExecutionMode::AgentTurn,
                },
            )
            .await;
        assert!(err.is_err());
    }

    #[test]
    fn blocked_rejects_fewer_than_three_turns() {
        let parsed = GoalActionInput {
            goal_id: "abc12345".into(),
            summary: None,
            reason: Some("user must restart the server".into()),
            evidence: Some("three identical timeouts".into()),
            repeated_turns: Some(2),
            required_action: None,
            resume_after_ms: None,
        };
        let result = (|p: &GoalActionInput| {
            if p.repeated_turns.unwrap_or(0) < 3 {
                Err(anyhow::anyhow!("too few turns"))
            } else {
                Ok((GoalContractStatus::Blocked, None::<String>, String::new()))
            }
        })(&parsed);
        assert!(result.is_err());
    }
}
