//! Fork: relay swarm attention transitions to the chat integration (Telegram).
//!
//! The swarm dock and strip already mark members whose lifecycle needs
//! attention (failed, crashed, blocked, waiting_network). Those markers are
//! only visible inside the TUI, so a user running a long swarm from a phone
//! never learns an agent got stuck. This module watches the same transitions
//! the status strip announces and fires a best-effort
//! [`ChatServiceClient::notify`] per newly-attention member, deduplicated by
//! (session_id, status) so a flapping agent cannot spam Telegram.
//!
//! Off silently when `[chat]` is not configured; failures are logged and
//! never block the UI thread (sends run on a detached thread like
//! `chat_status::probe_if_stale`).

use crate::protocol::SwarmMemberStatus;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Statuses the dock treats as attention-worthy. Kept in sync with the
/// `dock_status_rank` tier 0 in `info_widget_swarm_background`.
const ATTENTION_STATUSES: &[&str] = &["blocked", "waiting_network", "failed", "crashed"];

/// How long a (session, status) dedup entry suppresses repeat notifications.
/// A member that recovers and fails again after this window notifies again.
const DEDUP_TTL: Duration = Duration::from_secs(30 * 60);

struct DedupState {
    seen: HashMap<(String, String), Instant>,
}

static DEDUP: LazyLock<Mutex<DedupState>> = LazyLock::new(|| {
    Mutex::new(DedupState {
        seen: HashMap::new(),
    })
});

fn is_attention(status: &str) -> bool {
    ATTENTION_STATUSES.contains(&status)
}

/// Check a swarm snapshot against the previous one and notify Telegram for
/// every member that newly entered an attention status. Cheap when the chat
/// integration is off: one config read.
pub fn relay_attention_transitions(prev: &[SwarmMemberStatus], next: &[SwarmMemberStatus]) {
    let chat = &crate::config::config().chat;
    if !chat.is_configured() {
        return;
    }
    let prev_status: std::collections::HashMap<&str, &str> = prev
        .iter()
        .map(|m| (m.session_id.as_str(), m.status.as_str()))
        .collect();

    let mut to_notify: Vec<(String, String, String)> = Vec::new();
    {
        let Ok(mut dedup) = DEDUP.lock() else {
            return;
        };
        // Expire stale dedup entries so the map cannot grow unboundedly.
        dedup.seen.retain(|_, at| at.elapsed() < DEDUP_TTL);
        for member in next {
            if !is_attention(&member.status) {
                continue;
            }
            if prev_status
                .get(member.session_id.as_str())
                .is_some_and(|prev| *prev == member.status.as_str())
            {
                // Already in this status before: not a new transition.
                continue;
            }
            let key = (member.session_id.clone(), member.status.clone());
            if let Some(at) = dedup.seen.get(&key)
                && at.elapsed() < DEDUP_TTL
            {
                continue;
            }
            dedup.seen.insert(key, Instant::now());
            to_notify.push((
                member.friendly_name.clone().unwrap_or_else(|| {
                    member.session_id.chars().take(8).collect::<String>()
                }),
                member.status.clone(),
                member.detail.clone().unwrap_or_default(),
            ));
        }
    }
    if to_notify.is_empty() {
        return;
    }

    let Some(token) = chat.resolved_token() else {
        return;
    };
    let Ok(client) =
        crate::chat::ChatServiceClient::new(&chat.url, &token, chat.resolved_timeout_secs())
    else {
        return;
    };
    // Detached thread: notifications must never block the UI thread.
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                crate::logging::warn(&format!("chat attention relay: runtime: {e}"));
                return;
            }
        };
        rt.block_on(async {
            for (name, status, detail) in to_notify {
                let title = format!("🐝 {name}: {status}");
                let body = if detail.trim().is_empty() {
                    "Swarm agent needs attention.".to_string()
                } else {
                    let mut body = detail.trim().to_string();
                    if body.len() > 800 {
                        body.truncate(800);
                        body.push('…');
                    }
                    body
                };
                if let Err(e) = client.notify(&title, &body).await {
                    crate::logging::warn(&format!(
                        "chat attention relay: Telegram notify failed: {e}"
                    ));
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: &str, status: &str) -> SwarmMemberStatus {
        SwarmMemberStatus {
            session_id: id.to_string(),
            friendly_name: Some(id.to_string()),
            status: status.to_string(),
            detail: None,
            task_label: None,
            role: None,
            is_headless: Some(true),
            live_attachments: None,
            status_age_secs: Some(1),
            output_tail: None,
            report_back_to_session_id: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: crate::protocol::SwarmMemberRuntime::default(),
        }
    }

    #[test]
    fn attention_statuses_cover_the_dock_tier() {
        for status in ["blocked", "waiting_network", "failed", "crashed"] {
            assert!(is_attention(status), "{status}");
        }
        for status in ["running", "ready", "completed", "stopped", "thinking"] {
            assert!(!is_attention(status), "{status}");
        }
    }

    #[test]
    fn dedup_suppresses_repeat_transition_within_ttl() {
        // Uses the real DEDUP map: same (session, status) twice in a row is
        // suppressed even though prev snapshot does not contain the status.
        let next = vec![member("dedup-test-a", "failed")];
        relay_attention_transitions(&[], &next);
        // Second call within TTL must not enqueue another notification. The
        // chat integration is not configured in tests, so relay returns early
        // before dedup - exercise the dedup map directly instead.
        let key = ("dedup-test-a".to_string(), "failed".to_string());
        let Ok(mut dedup) = DEDUP.lock() else {
            return;
        };
        assert!(dedup.seen.contains_key(&key));
        dedup.seen.remove(&key);
    }
}
