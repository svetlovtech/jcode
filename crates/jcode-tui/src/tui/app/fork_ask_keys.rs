//! Fork: keyboard handling for the ask_user modal (`AskModal::key` and
//! the `handle_modal_key` dispatcher that stages answers into
//! `ForkAskState`). Split out of the former monolithic fork_ask_modal.rs.

use super::super::App;
use super::fork_ask_state::{AskModal, AskModalAction, MULTIPLE_SEPARATOR, NO_ANSWER};

impl AskModal {
    pub fn key(
        &mut self,
        code: ratatui::crossterm::event::KeyCode,
        _modifiers: ratatui::crossterm::event::KeyModifiers,
    ) -> AskModalAction {
        use ratatui::crossterm::event::KeyCode;

        if self.custom_mode {
            return self.custom_mode_key(code);
        }

        match code {
            KeyCode::Up => {
                self.cursor = (self.cursor + self.row_count() - 1) % self.row_count();
                AskModalAction::None
            }
            KeyCode::Down => {
                self.cursor = (self.cursor + 1) % self.row_count();
                AskModalAction::None
            }
            KeyCode::Esc => AskModalAction::Cancel,
            KeyCode::Enter => self.enter_action(),
            KeyCode::Char(' ') => self.space_action(),
            KeyCode::Char(d @ '1'..='9') => self.digit_action(d),
            _ => AskModalAction::None,
        }
    }

    /// Keys while composing a free-form answer.
    fn custom_mode_key(&mut self, code: ratatui::crossterm::event::KeyCode) -> AskModalAction {
        use ratatui::crossterm::event::KeyCode;

        match code {
            KeyCode::Esc => {
                // Back to the option list, modal stays open.
                self.custom_mode = false;
                AskModalAction::None
            }
            KeyCode::Enter => {
                let draft = self.custom_draft.trim().to_string();
                if draft.is_empty() {
                    AskModalAction::None
                } else {
                    AskModalAction::Submit(draft)
                }
            }
            KeyCode::Backspace => {
                self.custom_draft.pop();
                AskModalAction::None
            }
            KeyCode::Char(ch) => {
                self.custom_draft.push(ch);
                AskModalAction::None
            }
            _ => AskModalAction::None,
        }
    }

    /// Enter on the list: open custom mode, submit checked labels (multiple),
    /// or submit the highlighted option.
    fn enter_action(&mut self) -> AskModalAction {
        if self.cursor == self.custom_row() {
            self.custom_mode = true;
            return AskModalAction::None;
        }
        if self.spec.multiple {
            let labels = self.checked_labels();
            if labels.is_empty() {
                // Nothing checked: swallow Enter so the modal cannot be closed
                // by an accidental keypress; the user still has Esc.
                return AskModalAction::None;
            }
            return AskModalAction::Submit(labels.join(MULTIPLE_SEPARATOR));
        }
        AskModalAction::Submit(self.spec.options[self.cursor].label.clone())
    }

    /// Space toggles the highlighted option in multi-select mode.
    fn space_action(&mut self) -> AskModalAction {
        if !self.spec.multiple || self.cursor >= self.spec.options.len() {
            return AskModalAction::None;
        }
        self.checked[self.cursor] = !self.checked[self.cursor];
        AskModalAction::None
    }

    /// Digits `1..9`: instant submit in single-select, toggle in multi-select.
    fn digit_action(&mut self, digit: char) -> AskModalAction {
        let index = match digit.to_digit(10) {
            Some(d) if d >= 1 => (d as usize) - 1,
            _ => return AskModalAction::None,
        };
        if index >= self.spec.options.len() {
            return AskModalAction::None;
        }
        if self.spec.multiple {
            self.checked[index] = !self.checked[index];
            return AskModalAction::None;
        }
        AskModalAction::Submit(self.spec.options[index].label.clone())
    }
}


pub(crate) fn handle_modal_key(
    app: &mut App,
    code: ratatui::crossterm::event::KeyCode,
    modifiers: ratatui::crossterm::event::KeyModifiers,
) -> bool {
    // The daemon should time the question out and send an update, but if the
    // modal somehow outlives the timeout by a grace period, close it here.
    let expired = app
        .fork_ask
        .modal
        .as_ref()
        .is_some_and(AskModal::is_expired);
    if expired {
        if let Some(modal) = app.fork_ask.modal.take() {
            let answer = modal.answer_preview();
            app.set_status_notice(format!("Вопрос закрыт по таймауту: {answer}"));
        }
        return true;
    }

    let Some(modal) = app.fork_ask.modal.as_mut() else {
        return false;
    };

    let action = modal.key(code, modifiers);
    let request_id = modal.request_id.clone();
    match action {
        AskModalAction::None => {}
        AskModalAction::Submit(answer) => {
            app.fork_ask.modal = None;
            let mut arm = false;
            app.fork_ask.stage(request_id, answer, &mut arm);
            // Fork: the blocking ask_user tool generates no server traffic, so
            // without an explicit dispatch the staged answer would wait for the
            // next unrelated event. Arm the followup dispatcher immediately.
            app.pending_queued_dispatch = true;
        }
        AskModalAction::Cancel => {
            app.fork_ask.modal = None;
            let mut arm = false;
            app.fork_ask
                .stage(request_id, NO_ANSWER.to_string(), &mut arm);
            app.pending_queued_dispatch = true;
        }
    }
    true
}
