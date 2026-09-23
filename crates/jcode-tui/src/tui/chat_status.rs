//! Fork: chat-integration footer status (mirrors pi's `tg-bridge` indicator).
//!
//! Shows a compact 🟢/🔴/⚪ `chat` span in the advanced footer so the user
//! can see whether the AABEE chat service is reachable without making a tool
//! call. State machine mirrors pi-telegram-bridge's `availability`: unknown
//! at startup, `up` after any successful chat-service response, `down` after
//! a failure. A non-blocking probe runs at session start and every 5 minutes
//! so the indicator reflects reality before the first tool call.

use crate::tui::color_support::rgb;
use ratatui::style::Style;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatAvailability {
    Unknown,
    Up,
    Down,
}

struct ChatStatusState {
    availability: ChatAvailability,
    last_probe: Option<Instant>,
    probe_inflight: bool,
}

static CHAT_STATUS: OnceLock<Mutex<ChatStatusState>> = OnceLock::new();

const PROBE_INTERVAL: Duration = Duration::from_secs(5 * 60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(6);

fn state() -> &'static Mutex<ChatStatusState> {
    CHAT_STATUS.get_or_init(|| {
        Mutex::new(ChatStatusState {
            availability: ChatAvailability::Unknown,
            last_probe: None,
            probe_inflight: false,
        })
    })
}

pub fn availability() -> ChatAvailability {
    state()
        .lock()
        .map(|s| s.availability)
        .unwrap_or(ChatAvailability::Unknown)
}

fn on_success() {
    if let Ok(mut s) = state().lock() {
        s.availability = ChatAvailability::Up;
        s.last_probe = Some(Instant::now());
        s.probe_inflight = false;
    }
}

fn on_failure() {
    if let Ok(mut s) = state().lock() {
        s.availability = ChatAvailability::Down;
        s.last_probe = Some(Instant::now());
        s.probe_inflight = false;
    }
}

/// Kick a background probe when the last one is stale. Never blocks: the
/// HTTP call runs on a detached thread and updates the state on completion.
pub fn probe_if_stale() {
    let inflight = {
        let Ok(mut s) = state().lock() else { return };
        if s.probe_inflight {
            return;
        }
        if let Some(last) = s.last_probe {
            if last.elapsed() < PROBE_INTERVAL {
                return;
            }
        }
        s.probe_inflight = true;
        true
    };
    if !inflight {
        return;
    }
    std::thread::spawn(|| {
        let ok = std::panic::catch_unwind(probe_blocking).unwrap_or(false);
        if ok {
            on_success();
        } else {
            on_failure();
        }
    });
}

fn probe_blocking() -> bool {
    let chat = &crate::config::config().chat;
    if !chat.is_configured() {
        return false;
    }
    let Some(token) = chat.resolved_token() else {
        return false;
    };
    // A status probe must resolve quickly; the configured timeout serves
    // blocking user questions and can be minutes.
    let Ok(client) =
        crate::chat::ChatServiceClient::new(&chat.url, &token, PROBE_TIMEOUT.as_secs())
    else {
        return false;
    };
    // inbox_list is the cheapest authenticated endpoint; pi's bridge probes
    // the same one.
    let probe = std::thread::spawn(move || -> anyhow::Result<()> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| anyhow::anyhow!("tokio runtime: {e}"))?;
        rt.block_on(async { tokio::time::timeout(PROBE_TIMEOUT, client.inbox_list()).await })
            .map_err(|e| anyhow::anyhow!("probe timed out: {e}"))?
            .map_err(|e| anyhow::anyhow!("inbox probe: {e}"))?;
        Ok(())
    });
    matches!(probe.join(), Ok(Ok(()))) // thread ok, request ok
}

/// Render the footer span. `None` = hide (chat integration not configured).
pub fn footer_span() -> Option<ratatui::text::Span<'static>> {
    let chat = &crate::config::config().chat;
    if !chat.is_configured() {
        return None;
    }
    let (icon, style) = match availability() {
        ChatAvailability::Up => ("🟢 chat", Style::default().fg(rgb(150, 200, 150))),
        ChatAvailability::Down => ("🔴 chat", Style::default().fg(rgb(220, 120, 120))),
        ChatAvailability::Unknown => ("⚪ chat", Style::default().fg(rgb(140, 140, 150))),
    };
    Some(ratatui::text::Span::styled(icon, style))
}
