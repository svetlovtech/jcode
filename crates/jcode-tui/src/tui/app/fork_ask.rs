//! Fork: agent ask_user prompts surfaced in the TUI.
//!
//! ALL fork-specific ask_user state and behavior lives in this module, owned
//! by [`ForkAskState`] (a single field on `App`). Other modules never touch
//! the ask fields directly - they call the accessor/methods below - so an
//! upstream merge only has to reconcile:
//!   * one `pub fork_ask: ForkAskState` field on `App` (app.rs),
//!   * one-line hook call sites (marked `// Fork:` in the shared files),
//!   * this module and `fork_ask_modal.rs` (fork-only files, no conflicts).
//!
//! The daemon delivers an ask_user question as `ServerEvent::StdinRequest`
//! with `source == "ask_user"` (command stdin requests carry `"stdin"` and
//! never enter this module).
//!
//! Flow:
//! 1. `on_ask_prompt` / `on_ask_prompt_with_spec` record the pending request
//!    (and open the interactive modal when a structured spec arrived).
//! 2. The user's answer - typed, picked in the modal, or queued - is staged
//!    via `stage_answer` and flushed by `flush_staged` (remote followups) as
//!    `Request::StdinResponse`.
//! 3. `clear_if_pending` / `on_question_resolved_elsewhere` drop the
//!    interception when the turn continues, the question times out, or the
//!    answer arrived on another surface (Telegram).

use super::fork_ask_modal;
use super::fork_ask_modal::{AskModal, AskSpecUi};
use super::{App, DisplayMessage};
use crate::tui::backend::RemoteConnection;

/// Upper bound for the status-notice preview of the prompt.
const STATUS_PROMPT_PREVIEW_CHARS: usize = 110;

/// Fallback answer recorded when the modal is cancelled or expires.
pub(super) const NO_ANSWER: &str = "(без ответа)";

/// All pending ask_user state for this client. Kept in one struct so `App`
/// carries a single fork-owned field and upstream merges stay trivial.
#[derive(Default)]
pub struct ForkAskState {
    /// `(request_id, prompt)` while waiting for the user's typed answer.
    pub(super) pending_stdin: Option<(String, String)>,
    /// Interactive modal state (drawn as a full-screen overlay).
    pub(super) modal: Option<AskModal>,
    /// Answer staged by the modal (sync key handler), flushed async.
    pub(super) staged_answer: Option<(String, String)>,
}

impl ForkAskState {
    /// Whether an interactive modal is currently open.
    pub fn modal(&self) -> Option<&AskModal> {
        self.modal.as_ref()
    }

    /// Whether a question is waiting for the user's typed answer.
    pub fn has_pending_prompt(&self) -> bool {
        self.pending_stdin.is_some()
    }

    /// Whether a modal answer is staged but not yet sent.
    pub fn has_staged_answer(&self) -> bool {
        self.staged_answer.is_some()
    }

    /// Take the staged answer, if any (called by the async flusher).
    pub(super) fn take_staged_answer(&mut self) -> Option<(String, String)> {
        self.staged_answer.take()
    }
}

/// Accessor namespace on `App`: `app.fork_ask_ops().xxx()`. Keeps every hook
/// call site a single line while the state itself stays private to this
/// module's enum of operations.
pub(super) struct ForkAskOps<'a> {
    app: &'a mut App,
}

impl App {
    /// Fork: entry point for all ask_user state operations. See
    /// [`ForkAskOps`] and [`ForkAskState`].
    pub(super) fn fork_ask_ops(&mut self) -> ForkAskOps<'_> {
        ForkAskOps { app: self }
    }
}

impl ForkAskOps<'_> {
    /// Handle an ask_user question arriving from the daemon (textual fallback:
    /// no structured spec, the next typed message becomes the answer).
    pub(super) fn on_ask_prompt(&mut self, request_id: String, prompt: String) {
        let app = &mut *self.app;
        app.fork_ask.pending_stdin = Some((request_id, prompt.clone()));
        app.push_display_message(DisplayMessage::system(format!(
            "❓ Вопрос от агента (ваше следующее сообщение станет ответом):\n{prompt}"
        )));
        app.set_status_notice(format!("⌨ {}", flat_prompt(&prompt)));
    }

    /// Open the interactive modal on top of the pending stdin request.
    pub(super) fn on_ask_prompt_with_spec(
        &mut self,
        request_id: String,
        prompt: String,
        spec: AskSpecUi,
    ) {
        self.on_ask_prompt(request_id.clone(), prompt);
        self.app.fork_ask.modal = Some(AskModal::new(request_id, spec));
    }

    /// Feed one key press to the open modal. Returns `true` when the key was
    /// consumed (a modal is open). Submissions stage an answer and arm the
    /// followup dispatcher; the async flusher sends it.
    pub(super) fn modal_key(
        &mut self,
        code: ratatui::crossterm::event::KeyCode,
        modifiers: ratatui::crossterm::event::KeyModifiers,
    ) -> bool {
        super::fork_ask_modal::handle_modal_key(self.app, code, modifiers)
    }

    /// Stage `answer` for the pending request and arm the followup
    /// dispatcher so `flush_staged` sends it on this event-loop pass. Safe to
    /// call when nothing is pending (no-op).
    pub(super) fn stage_answer(&mut self, answer: &str) {
        let app = &mut *self.app;
        let Some((request_id, _)) = app.fork_ask.pending_stdin.clone() else {
            return;
        };
        app.fork_ask.staged_answer = Some((request_id, answer.to_string()));
        app.pending_queued_dispatch = true;
    }

    /// Send the staged answer, if any. Returns `true` when an answer went out
    /// (or was consumed); `false` when nothing was staged.
    pub(super) async fn flush_staged(&mut self, remote: &mut RemoteConnection) -> bool {
        let Some((request_id, answer)) = self.app.fork_ask.take_staged_answer() else {
            return false;
        };
        send_answer_for(self.app, remote, &request_id, &answer).await;
        true
    }

    /// Deliver a typed/queued message as the answer to the pending request.
    /// Returns `true` when the message was consumed as an answer; `false`
    /// when nothing is pending (caller falls through to normal handling).
    pub(super) async fn send_typed_answer(
        &mut self,
        remote: &mut RemoteConnection,
        answer: &str,
    ) -> bool {
        let Some((request_id, _)) = self.app.fork_ask.pending_stdin.clone() else {
            return false;
        };
        send_answer_for(self.app, remote, &request_id, answer).await;
        true
    }

    /// The turn continued, so the pending question was answered elsewhere
    /// (e.g. Telegram) or timed out; stop intercepting typed input.
    pub(super) fn clear_if_pending(&mut self) {
        let app = &mut *self.app;
        if app.fork_ask.pending_stdin.take().is_some() {
            app.set_status_notice("Вопрос закрыт (ответ принят в Telegram)");
        }
        app.fork_ask.modal = None;
        app.fork_ask.staged_answer = None;
    }

    /// The ask_user tool reported the question was resolved on a losing
    /// surface (Telegram won the race while this client showed the modal).
    /// Close the modal and surface the winning answer.
    pub(super) fn on_question_resolved_elsewhere(&mut self, answer: &str) {
        let app = &mut *self.app;
        let was_open = app.fork_ask.modal.is_some();
        app.fork_ask.modal = None;
        app.fork_ask.staged_answer = None;
        if app.fork_ask.pending_stdin.take().is_some() || was_open {
            app.push_display_message(DisplayMessage::system(format!(
                "Вопрос закрыт: ответ пришёл из Telegram: {answer}"
            )));
            app.set_status_notice("Вопрос закрыт (ответ из Telegram)");
        }
    }

    /// A queued message arrives while a question is pending: it IS the answer.
    /// Returns the joined answer text, or `None` when the queue is empty.
    pub(super) fn take_queued_answer(&mut self) -> Option<String> {
        let app = &mut *self.app;
        if app.fork_ask.pending_stdin.is_none() {
            return None;
        }
        let messages = std::mem::take(&mut app.queued_messages);
        let answer = messages.join(" ").trim().to_string();
        if answer.is_empty() {
            app.set_status_notice("Пустой ответ — вопрос остаётся открытым");
            return None;
        }
        Some(answer)
    }

    /// Whether the user's next queued message should be delivered as the
    /// answer (a question is pending and messages are queued).
    pub(super) fn queued_message_is_answer(&self) -> bool {
        self.app.fork_ask.pending_stdin.is_some() && !self.app.queued_messages.is_empty()
    }

    /// Arm the followup dispatcher when a typed message is queued while a
    /// question is pending (the message becomes the answer).
    pub(super) fn arm_dispatch_if_pending(&mut self) {
        if self.app.fork_ask.pending_stdin.is_some() {
            self.app.pending_queued_dispatch = true;
        }
    }
}

/// Shared send path for staged and typed answers. On a send failure the
/// pending request is restored so the user can retry.
async fn send_answer_for(
    app: &mut App,
    remote: &mut RemoteConnection,
    request_id: &str,
    answer: &str,
) {
    let answer = answer.trim().to_string();
    match remote.send_stdin_response(request_id, &answer).await {
        Ok(()) => {
            app.fork_ask.pending_stdin = None;
            app.push_display_message(DisplayMessage::system(format!(
                "Ответ отправлен агенту: {answer}"
            )));
        }
        Err(err) => {
            app.set_status_notice(format!("Не удалось отправить ответ: {err}"));
        }
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
            flat.chars()
                .take(STATUS_PROMPT_PREVIEW_CHARS)
                .collect::<String>()
        )
    } else {
        flat
    }
}

// ── fork_ask_modal plumbing ────────────────────────────────────────────────
// The modal state itself lives in ForkAskState; fork_ask_modal's sync key
// handler needs raw access, granted through these narrow pub(super) helpers.

impl ForkAskState {
    pub(super) fn modal_mut(&mut self) -> Option<&mut AskModal> {
        self.modal.as_mut()
    }

    pub(super) fn stage(&mut self, request_id: String, answer: String, arm_dispatch: &mut bool) {
        self.staged_answer = Some((request_id, answer));
        *arm_dispatch = true;
    }
}
