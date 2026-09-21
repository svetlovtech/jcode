//! Fork: agent ask_user prompts surfaced in the TUI.
//!
//! All fork-specific ask_user UI behavior lives here so upstream merges touch
//! only small, well-marked call sites. The daemon delivers an ask_user
//! question as a `ServerEvent::StdinRequest` with `source == "ask_user"`
//! (command stdin requests carry `"stdin"` and never enter this module).
//!
//! Flow:
//! 1. `on_ask_prompt` records the pending request and shows the prompt.
//! 2. The user's next typed/queued message is delivered as
//!    `Request::StdinResponse` (see interceptors in `remote`).
//! 3. `clear_if_pending` drops the interception when the turn continues
//!    (answered in Telegram or timed out).

use crate::tui::backend::RemoteConnection;
use super::{App, DisplayMessage};

/// Upper bound for the status-notice preview of the prompt.
const STATUS_PROMPT_PREVIEW_CHARS: usize = 110;

/// Handle an ask_user question arriving from the daemon.
pub(super) fn on_ask_prompt(app: &mut App, request_id: String, prompt: String) {
    app.pending_stdin = Some((request_id, prompt.clone()));
    app.push_display_message(DisplayMessage::system(format!(
        "❓ Вопрос от агента (ваше следующее сообщение станет ответом):\n{prompt}"
    )));
    app.set_status_notice(format!("⌨ {}", flat_prompt(&prompt)));
}

/// Fork: open the structured ask modal on top of the pending stdin request.
/// Same interception semantics as `on_ask_prompt`, plus a modal UI built from
/// the structured spec (local `AskSpecUi` until the protocol crate publishes
/// the wire type and the coordinator swaps the construction site).
pub(super) fn on_ask_prompt_with_spec(
    app: &mut App,
    request_id: String,
    prompt: String,
    spec: super::fork_ask_modal::AskSpecUi,
) {
    on_ask_prompt(app, request_id.clone(), prompt);
    app.pending_ask_modal = Some(super::fork_ask_modal::AskModal::new(request_id, spec));
}

/// Deliver `answer` as the response to the pending ask_user request.
///
/// Returns `true` when the answer went out (or the question was consumed);
/// `false` only when nothing is pending (the caller should fall through to
/// normal input handling). On a send failure the pending request is restored
/// so the user can retry.
pub(super) async fn send_answer(
    app: &mut App,
    remote: &mut RemoteConnection,
    answer: &str,
) -> bool {
    let Some((request_id, prompt)) = app.pending_stdin.clone() else {
        return false;
    };
    let answer = answer.trim().to_string();
    match remote.send_stdin_response(&request_id, &answer).await {
        Ok(()) => {
            app.pending_stdin = None;
            app.push_display_message(DisplayMessage::system(format!(
                "Ответ отправлен агенту: {answer}"
            )));
        }
        Err(err) => {
            app.pending_stdin = Some((request_id, prompt));
            app.set_status_notice(format!("Не удалось отправить ответ: {err}"));
        }
    }
    true
}

/// The turn continued, so the pending question was answered elsewhere (e.g.
/// Telegram) or timed out; stop intercepting typed input.
pub(super) fn clear_if_pending(app: &mut App) {
    if app.pending_stdin.take().is_some() {
        app.set_status_notice("Вопрос закрыт (ответ принят в Telegram)");
    }
    // Fork: a question closed elsewhere (Telegram/timeout) must close the
    // modal and drop any staged-but-unflushed modal answer too.
    app.pending_ask_modal = None;
    app.pending_ask_answer = None;
}

/// Fork: the ask_user tool reported that this question was resolved on a
/// losing surface (Telegram won the race while this client showed the modal).
/// Close the modal for this request id (any request id, if a stale event
/// arrives after the client already re-asked) and surface the winning answer.
pub(super) fn on_question_resolved_elsewhere(app: &mut App, answer: &str) {
    let was_open = app.pending_ask_modal.is_some();
    app.pending_ask_modal = None;
    app.pending_ask_answer = None;
    if app.pending_stdin.take().is_some() || was_open {
        app.push_display_message(DisplayMessage::system(format!(
            "Вопрос закрыт: ответ пришёл из Telegram: {answer}"
        )));
        app.set_status_notice("Вопрос закрыт (ответ из Telegram)");
    }
}

/// One-line preview of a multi-line prompt for the status bar.
fn flat_prompt(prompt: &str) -> String {
    let flat: String = prompt
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    if flat.chars().count() > STATUS_PROMPT_PREVIEW_CHARS {
        format!(
            "{}…",
            flat.chars().take(STATUS_PROMPT_PREVIEW_CHARS).collect::<String>()
        )
    } else {
        flat
    }
}
