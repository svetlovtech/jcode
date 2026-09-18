//! Per-session goal contracts (pi-style `/goal`).
//!
//! A goal contract is a single active objective per session that is injected
//! into the system prompt on every turn until it is completed, blocked, or
//! cleared. This is deliberately separate from [`crate::goal`] (the
//! initiatives feature): a contract is lightweight, at most one per session,
//! and its entire purpose is prompt injection plus completion tooling.
//!
//! State lives in a global registry keyed by session id and is mirrored to
//! `<jcode-dir>/goals/<session_id>.json` so a goal survives daemon restarts.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;

/// Lifecycle of a goal contract. Mirrors the pi goal states that matter for
/// prompt injection: the agent completes, blocks, or waits on the goal; the
/// user clears it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalContractStatus {
    Active,
    Waiting,
    Blocked,
    Completed,
}

impl GoalContractStatus {
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Waiting => "WAITING",
            Self::Blocked => "BLOCKED",
            Self::Completed => "COMPLETED",
        }
    }
}

/// The full contract for one session's active goal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalContract {
    /// Short id used by the agent to reference the goal in tool calls.
    pub goal_id: String,
    /// The objective exactly as the user stated it.
    pub objective: String,
    pub status: GoalContractStatus,
    /// Latest agent-supplied note (completion summary, blocker reason, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Default)]
struct Registry {
    goals: HashMap<String, GoalContract>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

/// Test-only redirect for the goals directory so tests never touch the real
/// `~/.jcode`. Written once per test-binary run (tests then use unique session
/// ids inside the shared directory).
#[cfg(test)]
static TEST_GOALS_DIR: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);

fn goals_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(dir) = TEST_GOALS_DIR
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
    {
        return dir.clone();
    }
    crate::storage::jcode_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("goals")
}

/// Filesystem-safe name for a session id (session ids are opaque strings).
fn session_file_stem(session_id: &str) -> String {
    let clean: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() {
        "session".to_string()
    } else {
        clean
    }
}

fn contract_path(session_id: &str) -> PathBuf {
    goals_dir().join(format!("{}.json", session_file_stem(session_id)))
}

fn persist(contract: &GoalContract, session_id: &str) {
    let path = contract_path(session_id);
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        crate::logging::warn(&format!(
            "Goal contract: cannot create {}: {}",
            parent.display(),
            e
        ));
        return;
    }
    if let Err(e) = crate::storage::write_json(&path, contract) {
        crate::logging::warn(&format!("Goal contract: persist failed: {}", e));
    }
}

fn load_persisted(session_id: &str) -> Option<GoalContract> {
    let path = contract_path(session_id);
    let json = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<GoalContract>(&json) {
        Ok(contract) => Some(contract),
        Err(e) => {
            crate::logging::warn(&format!("Goal contract: corrupt file for session: {}", e));
            None
        }
    }
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn new_goal_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

/// Set (or replace) the session's goal contract. Replacing an existing goal
/// generates a new goal id, matching pi semantics where each /goal is a fresh
/// contract.
pub fn set_goal(session_id: &str, objective: &str) -> GoalContract {
    let contract = GoalContract {
        goal_id: new_goal_id(),
        objective: objective.trim().to_string(),
        status: GoalContractStatus::Active,
        note: None,
        created_at: now_iso(),
        updated_at: now_iso(),
    };
    persist(&contract, session_id);
    registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .goals
        .insert(session_id.to_string(), contract.clone());
    contract
}

/// Get the session's goal contract, lazily restoring from disk.
pub fn get_goal(session_id: &str) -> Option<GoalContract> {
    let mut guard = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(contract) = guard.goals.get(session_id) {
        return Some(contract.clone());
    }
    let restored = load_persisted(session_id);
    if let Some(contract) = &restored {
        guard.goals.insert(session_id.to_string(), contract.clone());
    }
    restored
}

/// Transition the contract's status with an optional note. Returns the updated
/// contract, or None when the session has no goal or `goal_id` does not match
/// the active one (stale-guard: an agent referencing a replaced goal must not
/// mutate the new contract).
pub fn transition(
    session_id: &str,
    goal_id: &str,
    status: GoalContractStatus,
    note: Option<String>,
) -> Option<GoalContract> {
    let mut guard = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let contract = guard.goals.get_mut(session_id)?;
    if contract.goal_id != goal_id {
        return None;
    }
    contract.status = status;
    contract.note = note.filter(|n| !n.trim().is_empty());
    contract.updated_at = now_iso();
    let snapshot = contract.clone();
    drop(guard);
    persist(&snapshot, session_id);
    Some(snapshot)
}

/// Remove the contract (user-initiated `/goal clear`). Returns true when a
/// goal existed.
pub fn clear_goal(session_id: &str) -> bool {
    let removed = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .goals
        .remove(session_id)
        .is_some();
    let path = contract_path(session_id);
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    removed
}

/// Render the per-turn prompt injection block for an active contract.
///
/// The wording encodes the pi goal rules the agent is held to: keep working
/// across turns, verify before completing, distinguish blocked from hard work,
/// and never call the completion tools speculatively.
pub fn prompt_block(contract: &GoalContract) -> String {
    let mut block = String::new();
    block.push_str("# Goal Contract\n\n");
    block.push_str(&format!(
        "Goal mode is active for this session (goal_id: {}). Work on this goal fully; do not redefine it narrower or stop early.\n\n",
        contract.goal_id
    ));
    block.push_str("<goal_objective>\n");
    block.push_str(contract.objective.trim());
    block.push_str("\n</goal_objective>\n\n");
    block.push_str(&format!("Status: {}\n", contract.status.as_label()));
    if let Some(note) = &contract.note {
        block.push_str(&format!("Latest note: {note}\n"));
    }
    block.push_str(
        "Rules:\n\
         - Preserve the full objective across turns; treat current worktree, command output, and tests as the only proof of progress.\n\
         - Keep working end-to-end; do not stop at analysis, plans, or partial fixes. If a tool fails, try reasonable alternatives.\n\
         - Call goal_complete only when every requirement is verified with concrete evidence; pass this goal_id and a completion summary.\n\
         - Call goal_blocked only when the same external (user-action) blocker has recurred for at least three consecutive turns; include evidence and the required action.\n\
         - Call goal_wait only when progress depends on an arranged external wake event; include the reason and an optional resume_after_ms.\n\
         - After any goal tool call, end the turn; do not call other tools in parallel with it.\n",
    );
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir() -> PathBuf {
        static INIT: OnceLock<PathBuf> = OnceLock::new();
        INIT.get_or_init(|| {
            let dir = std::env::temp_dir()
                .join(format!("jcode-goal-contract-tests-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create test goals dir");
            dir
        })
        .clone()
    }

    fn with_test_dir<T>(f: impl FnOnce() -> T) -> T {
        *TEST_GOALS_DIR.write().unwrap_or_else(|p| p.into_inner()) = Some(test_dir());
        let result = f();
        *TEST_GOALS_DIR.write().unwrap_or_else(|p| p.into_inner()) = None;
        result
    }

    #[test]
    fn set_get_transition_roundtrip() {
        with_test_dir(|| {
            let session = format!("test-session-{}", uuid::Uuid::new_v4());
            assert!(get_goal(&session).is_none());

            let contract = set_goal(&session, "ship the feature");
            assert_eq!(contract.status, GoalContractStatus::Active);
            assert_eq!(contract.objective, "ship the feature");
            assert_eq!(contract.goal_id.len(), 8);

            let fetched = get_goal(&session).expect("goal present");
            assert_eq!(fetched.goal_id, contract.goal_id);

            // Stale goal_id must not mutate the contract.
            assert!(
                transition(&session, "deadbeef", GoalContractStatus::Completed, None).is_none()
            );
            assert_eq!(
                get_goal(&session).unwrap().status,
                GoalContractStatus::Active
            );

            let done = transition(
                &session,
                &contract.goal_id,
                GoalContractStatus::Completed,
                Some("all tests green".into()),
            )
            .expect("transition");
            assert_eq!(done.status, GoalContractStatus::Completed);
            assert_eq!(done.note.as_deref(), Some("all tests green"));

            assert!(clear_goal(&session));
            assert!(get_goal(&session).is_none());
            assert!(!clear_goal(&session));
        });
    }

    #[test]
    fn persistence_survives_registry_drop() {
        with_test_dir(|| {
            let session = format!("persist-session-{}", uuid::Uuid::new_v4());
            let contract = set_goal(&session, "survive restarts");
            // Simulate a fresh process: registry starts empty, load comes from disk.
            registry().lock().unwrap().goals.remove(&session);
            let restored = get_goal(&session).expect("restored from disk");
            assert_eq!(restored.goal_id, contract.goal_id);
            assert_eq!(restored.objective, "survive restarts");
            clear_goal(&session);
        });
    }

    #[test]
    fn prompt_block_contains_objective_and_rules() {
        with_test_dir(|| {
            let session = format!("prompt-session-{}", uuid::Uuid::new_v4());
            let contract = set_goal(&session, "write the docs");
            let block = prompt_block(&contract);
            assert!(block.contains("Goal Contract"));
            assert!(block.contains(&contract.goal_id));
            assert!(block.contains("write the docs"));
            assert!(block.contains("goal_complete"));
            assert!(block.contains("Status: ACTIVE"));
            clear_goal(&session);
        });
    }

    #[test]
    fn session_file_stem_sanitizes_unsafe_names() {
        assert_eq!(session_file_stem("abc/../../etc"), "abc_______etc");
        assert_eq!(session_file_stem(""), "session");
    }
}
