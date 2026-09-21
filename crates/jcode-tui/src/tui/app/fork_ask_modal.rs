//! Fork: interactive ask_user modal for the TUI.
//!
//! All fork-specific ask_user *modal* behavior lives here so upstream merges
//! touch only small, well-marked call sites. The daemon delivers an ask_user
//! question as a `ServerEvent::StdinRequest` with `source == "ask_user"`;
//! `fork_ask` records the pending stdin request and this module draws/edits
//! the structured question UI on top of it.
//!
//! Flow:
//! 1. `on_ask_prompt_with_spec` (in `fork_ask`) records the pending request
//!    and opens the modal.
//! 2. `handle_modal_key` (called first from `input::handle_modal_key`)
//!    dispatches keys into `AskModal::key` and stages the answer in
//!    `App::pending_ask_answer`.
//! 3. `process_remote_followups` (in `remote`) flushes the staged answer via
//!    `fork_ask::send_answer` (`Request::StdinResponse`).
//! 4. `clear_if_pending` closes the modal when the question is answered or
//!    timed out elsewhere (e.g. Telegram).
//!
//! `AskSpecUi`/`AskOptionUi` are local copies of the wire shape; once the
//! protocol crate publishes the structured AskSpec, the coordinator replaces
//! them with `jcode_protocol` types and adapts the single construction site.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};

use super::{App};
use crate::tui::color_support::rgb;
use jcode_tui_style::theme::{accent_color, dim_color};

/// Background fill of the modal box (matches the inline interactive pickers).
const MODAL_BG: (u8, u8, u8) = (18, 18, 26);
/// Border color of the modal box.
const MODAL_BORDER: (u8, u8, u8) = (85, 85, 110);
/// Grace period after the declared timeout before the TUI self-closes the
/// modal (the daemon should time the question out first and send an update).
const TIMEOUT_CLOSE_GRACE_SECS: u64 = 30;

/// One selectable option of an ask_user question (local wire copy).
#[derive(Debug, Clone, PartialEq)]
pub struct AskOptionUi {
    pub label: String,
    pub description: String,
}

/// Structured ask_user question (local wire copy of the future protocol type).
#[derive(Debug, Clone, PartialEq)]
pub struct AskSpecUi {
    pub header: String,
    pub question: String,
    pub options: Vec<AskOptionUi>,
    pub multiple: bool,
    pub timeout_secs: u64,
    pub question_index: usize,
    pub question_total: usize,
}

/// Separator used when joining multiple checked answers.
const MULTIPLE_SEPARATOR: &str = "; ";
/// Fallback answer recorded when the modal is cancelled or expires.
const NO_ANSWER: &str = "(без ответа)";

/// Mutable UI state of one open ask_user question.
#[derive(Debug, Clone)]
pub struct AskModal {
    pub request_id: String,
    pub spec: AskSpecUi,
    /// Highlighted row: `0..options.len()` are options, `options.len()` is the
    /// "own answer" row.
    pub cursor: usize,
    /// Checked flags per option (used when `multiple`).
    pub checked: Vec<bool>,
    /// Whether the user is composing a free-form answer.
    pub custom_mode: bool,
    /// Draft of the free-form answer while `custom_mode`.
    pub custom_draft: String,
    /// When the modal opened; drives the countdown display and self-close.
    pub opened_at: std::time::Instant,
}

/// Result of one key press on the modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskModalAction {
    None,
    /// Submit this answer text for the pending request.
    Submit(String),
    /// Close without answering ("(без ответа)").
    Cancel,
}

impl AskModal {
    pub fn new(request_id: String, spec: AskSpecUi) -> Self {
        let len = spec.options.len() + 1; // last row = own answer
        Self {
            request_id,
            checked: vec![false; len],
            cursor: 0,
            custom_mode: false,
            custom_draft: String::new(),
            opened_at: std::time::Instant::now(),
            spec,
        }
    }

    /// Row index of the "own answer" entry.
    fn custom_row(&self) -> usize {
        self.spec.options.len()
    }

    fn row_count(&self) -> usize {
        self.spec.options.len() + 1
    }

    /// Remaining seconds until the declared timeout elapses (0 when past it).
    fn remaining_secs(&self) -> u64 {
        self.spec
            .timeout_secs
            .saturating_sub(self.opened_at.elapsed().as_secs())
    }

    /// Whether the modal outlived its timeout by the close grace period.
    pub fn is_expired(&self) -> bool {
        self.spec.timeout_secs > 0
            && self.opened_at.elapsed().as_secs() >= self.spec.timeout_secs + TIMEOUT_CLOSE_GRACE_SECS
    }

    /// Countdown label shown in the header: `⏳ mm:ss`.
    pub fn action_timeout_text(&self) -> String {
        let secs = self.remaining_secs();
        format!("⏳ {:02}:{:02}", secs / 60, secs % 60)
    }

    /// Compact answer preview for chat/status messages.
    pub fn answer_preview(&self) -> String {
        if self.custom_mode {
            return if self.custom_draft.trim().is_empty() {
                NO_ANSWER.to_string()
            } else {
                self.custom_draft.trim().to_string()
            };
        }
        let picked: Vec<String> = self
            .spec
            .options
            .iter()
            .zip(self.checked.iter())
            .filter(|(_, checked)| **checked)
            .map(|(option, _)| option.label.clone())
            .collect();
        if picked.is_empty() {
            NO_ANSWER.to_string()
        } else {
            picked.join(MULTIPLE_SEPARATOR)
        }
    }

    /// Checked option labels joined for a multi-select submit.
    fn checked_labels(&self) -> Vec<String> {
        self.spec
            .options
            .iter()
            .zip(self.checked.iter())
            .filter(|(_, checked)| **checked)
            .map(|(option, _)| option.label.clone())
            .collect()
    }

    /// Handle one key press. Returns the resulting action.
    pub fn key(&mut self, code: ratatui::crossterm::event::KeyCode, _modifiers: ratatui::crossterm::event::KeyModifiers) -> AskModalAction {
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

/// Sync key handler called first from `input::handle_modal_key`.
///
/// Submissions cannot be sent from a sync context, so the answer is staged in
/// `App::pending_ask_answer` and flushed by `process_remote_followups`.
/// Returns `true` when the key was consumed (the modal is open).
pub(super) fn handle_modal_key(
    app: &mut App,
    code: ratatui::crossterm::event::KeyCode,
    modifiers: ratatui::crossterm::event::KeyModifiers,
) -> bool {
    // The daemon should time the question out and send an update, but if the
    // modal somehow outlives the timeout by a grace period, close it here.
    let expired = app
        .pending_ask_modal
        .as_ref()
        .is_some_and(AskModal::is_expired);
    if expired {
        if let Some(modal) = app.pending_ask_modal.take() {
            let answer = modal.answer_preview();
            app.set_status_notice(format!("Вопрос закрыт по таймауту: {answer}"));
        }
        return true;
    }

    let Some(modal) = app.pending_ask_modal.as_mut() else {
        return false;
    };

    match modal.key(code, modifiers) {
        AskModalAction::None => {}
        AskModalAction::Submit(answer) => {
            app.pending_ask_answer = Some((modal.request_id.clone(), answer));
            app.pending_ask_modal = None;
            // Fork: the blocking ask_user tool generates no server traffic, so
            // without an explicit dispatch the staged answer would wait for the
            // next unrelated event. Arm the followup dispatcher immediately.
            app.pending_queued_dispatch = true;
        }
        AskModalAction::Cancel => {
            let request_id = modal.request_id.clone();
            app.pending_ask_answer = Some((request_id, NO_ANSWER.to_string()));
            app.pending_ask_modal = None;
            app.pending_queued_dispatch = true;
        }
    }
    true
}

/// Draw the centered ask modal over the (already cleared) full frame.
pub fn draw_ask_modal(frame: &mut ratatui::Frame, modal: &AskModal) {
    let area = frame.area();
    if area.width < 8 || area.height < 5 {
        return;
    }

    // Fork: the box height must be IDENTICAL in every modal state (option
    // list, custom-answer input). The modal renders as a late overlay on top
    // of the chat; if the box shrank when custom_mode opened, cells of the
    // previous, larger box were left on screen (stale "1. …" rows) until some
    // other keypress forced a full repaint. Always size the box for the full
    // option list and pad the short custom-mode body with blank background
    // lines instead.
    let list_rows = modal.spec.options.len() + 1;
    let description_rows = modal
        .spec
        .options
        .iter()
        .filter(|option| !option.description.is_empty())
        .count();
    // +2 for the custom-mode input line and its Esc hint (list rows already
    // cover the custom input's single line).
    let height = (2 + 2 + list_rows + description_rows + 2).min(area.height as usize) as u16;

    let width = (area.width * 3 / 5).min(80).max(1).min(area.width.saturating_sub(4)).max(1);
    let vertical = (area.height.saturating_sub(height)) / 2;
    let horizontal = (area.width.saturating_sub(width)) / 2;
    let box_area = Rect::new(horizontal, vertical, width, height.max(3));

    let bg = rgb(MODAL_BG.0, MODAL_BG.1, MODAL_BG.2);
    let border = rgb(MODAL_BORDER.0, MODAL_BORDER.1, MODAL_BORDER.2);
    let accent = accent_color();
    let dim = dim_color();

    let spec = &modal.spec;
    let mut title_spans = vec![Span::styled(
        format!("❓ {}", spec.header),
        Style::default().fg(accent).add_modifier(Modifier::BOLD),
    )];
    if spec.question_total > 1 {
        title_spans.push(Span::styled(
            format!(" ({}/{})", spec.question_index + 1, spec.question_total),
            Style::default().fg(dim),
        ));
    }
    if spec.timeout_secs > 0 {
        title_spans.push(Span::styled(
            format!("  {}", modal.action_timeout_text()),
            Style::default().fg(dim),
        ));
    }

    let inner_width = box_area.width.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(vec![Span::styled(
        truncate_display(&spec.question, inner_width),
        Style::default().add_modifier(Modifier::BOLD),
    )]));

    if modal.custom_mode {
        // One-line free-form input with cursor block.
        lines.push(Line::from(vec![
            Span::styled("> ", Style::default().fg(accent)),
            Span::raw(truncate_display(&modal.custom_draft, inner_width.saturating_sub(4))),
            Span::styled("▏", Style::default().fg(accent)),
        ]));
        lines.push(Line::from(Span::styled(
            "Esc — вернуться к списку",
            Style::default().fg(dim).add_modifier(Modifier::ITALIC),
        )));
    } else {
        for (index, option) in spec.options.iter().enumerate() {
            let cursor_here = modal.cursor == index;
            let marker = if cursor_here { "›" } else { " " };
            let (state, state_style) = if spec.multiple {
                let checked_style = Style::default().fg(accent).add_modifier(Modifier::BOLD);
                if modal.checked[index] {
                    ("[x] ".to_string(), checked_style)
                } else {
                    ("[ ] ".to_string(), Style::default().fg(dim))
                }
            } else {
                (format!("{}. ", index + 1), Style::default().fg(dim))
            };
            let label_style = if cursor_here {
                Style::default().fg(accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{marker} "), if cursor_here { Style::default().fg(accent) } else { Style::default() }),
                Span::styled(state, state_style),
                Span::styled(truncate_display(&option.label, inner_width.saturating_sub(6)), label_style),
            ]));
            if !option.description.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!(
                        "    {}",
                        truncate_display(&option.description, inner_width.saturating_sub(4))
                    ),
                    Style::default().fg(dim).add_modifier(Modifier::ITALIC),
                )));
            }
        }

        let cursor_on_custom = modal.cursor == modal.custom_row();
        lines.push(Line::from(vec![
            Span::styled(
                if cursor_on_custom { "› " } else { "  " },
                if cursor_on_custom { Style::default().fg(accent) } else { Style::default() },
            ),
            Span::styled(
                "✎ Свой вариант…",
                if cursor_on_custom {
                    Style::default().fg(accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(dim)
                },
            ),
        ]));
    }

    let hint = if modal.custom_mode {
        "Enter отправить · Esc к списку"
    } else if spec.multiple {
        "↑/↓ · Space/цифра отметить · Enter отправить · Esc без ответа"
    } else {
        "↑/↓ · цифра — быстрый выбор · Enter · Esc без ответа"
    };
    lines.push(Line::from(Span::styled(
        truncate_display(hint, inner_width),
        Style::default().fg(dim),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(bg))
        .title(Line::from(title_spans));

    let paragraph = Paragraph::new(lines).block(block).style(Style::default().bg(bg));
    // Fork: wipe the box area first. The modal overlays the live transcript;
    // without an explicit clear the transcript text showed through every
    // empty cell of the box (only cells the Paragraph actually wrote got the
    // background color).
    frame.render_widget(ratatui::widgets::Clear, box_area);
    frame.render_widget(paragraph, box_area);
}

/// Display-width-aware truncation with an ellipsis (mirrors the inline picker).
fn truncate_display(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let display_width = |text: &str| unicode_width::UnicodeWidthStr::width(text);
    if display_width(text) <= max_width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width + 1 > max_width {
            break;
        }
        out.push(ch);
        used += ch_width;
    }
    out.push('…');
    out
}

// Fork: adapt the wire `AskSpec` into the local UI shape. Kept as a `From`
// impl at the module boundary so the rest of the TUI never sees protocol
// types directly.
impl From<jcode_protocol::AskSpec> for AskSpecUi {
    fn from(spec: jcode_protocol::AskSpec) -> Self {
        Self {
            header: spec.header,
            question: spec.question,
            options: spec
                .options
                .into_iter()
                .map(|option| AskOptionUi {
                    label: option.label,
                    description: option.description,
                })
                .collect(),
            multiple: spec.multiple,
            timeout_secs: spec.timeout_secs,
            question_index: spec.question_index,
            question_total: spec.question_total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    fn single_spec() -> AskSpecUi {
        AskSpecUi {
            header: "Подтверждение".into(),
            question: "Продолжить?".into(),
            options: vec![
                AskOptionUi { label: "Да".into(), description: "продолжить".into() },
                AskOptionUi { label: "Нет".into(), description: String::new() },
                AskOptionUi { label: "Отмена".into(), description: String::new() },
            ],
            multiple: false,
            timeout_secs: 60,
            question_index: 0,
            question_total: 1,
        }
    }

    fn multi_spec() -> AskSpecUi {
        let mut spec = single_spec();
        spec.multiple = true;
        spec
    }

    fn no_timeout(mut spec: AskSpecUi) -> AskSpecUi {
        spec.timeout_secs = 0;
        spec
    }

    #[test]
    fn navigation_wraps_around() {
        let mut modal = AskModal::new("r1".into(), no_timeout(single_spec()));
        assert_eq!(modal.cursor, 0);
        assert_eq!(modal.key(KeyCode::Up, KeyModifiers::NONE), AskModalAction::None);
        assert_eq!(modal.cursor, 3); // custom row
        assert_eq!(modal.key(KeyCode::Down, KeyModifiers::NONE), AskModalAction::None);
        assert_eq!(modal.cursor, 0);
        assert_eq!(modal.key(KeyCode::Down, KeyModifiers::NONE), AskModalAction::None);
        assert_eq!(modal.cursor, 1);
    }

    #[test]
    fn digit_submits_instantly_in_single_select() {
        let mut modal = AskModal::new("r1".into(), no_timeout(single_spec()));
        assert_eq!(
            modal.key(KeyCode::Char('2'), KeyModifiers::NONE),
            AskModalAction::Submit("Нет".into())
        );
    }

    #[test]
    fn digit_toggles_in_multi_select() {
        let mut modal = AskModal::new("r1".into(), no_timeout(multi_spec()));
        assert_eq!(modal.key(KeyCode::Char('1'), KeyModifiers::NONE), AskModalAction::None);
        assert!(modal.checked[0]);
        assert_eq!(modal.key(KeyCode::Char('3'), KeyModifiers::NONE), AskModalAction::None);
        assert!(modal.checked[2]);
        assert_eq!(modal.key(KeyCode::Char('3'), KeyModifiers::NONE), AskModalAction::None);
        assert!(!modal.checked[2]);
    }

    #[test]
    fn space_toggles_in_multi_select() {
        let mut modal = AskModal::new("r1".into(), no_timeout(multi_spec()));
        assert_eq!(modal.key(KeyCode::Down, KeyModifiers::NONE), AskModalAction::None);
        assert_eq!(modal.key(KeyCode::Char(' '), KeyModifiers::NONE), AskModalAction::None);
        assert!(modal.checked[1]);
    }

    #[test]
    fn digit_out_of_range_is_ignored() {
        let mut modal = AskModal::new("r1".into(), no_timeout(single_spec()));
        assert_eq!(
            modal.key(KeyCode::Char('9'), KeyModifiers::NONE),
            AskModalAction::None
        );
    }

    #[test]
    fn enter_on_custom_row_opens_custom_mode() {
        let mut modal = AskModal::new("r1".into(), no_timeout(single_spec()));
        modal.cursor = modal.custom_row();
        assert_eq!(modal.key(KeyCode::Enter, KeyModifiers::NONE), AskModalAction::None);
        assert!(modal.custom_mode);
    }

    #[test]
    fn custom_mode_typing_goes_to_draft_and_enter_submits() {
        let mut modal = AskModal::new("r1".into(), no_timeout(single_spec()));
        modal.custom_mode = true;
        for ch in "почти".chars() {
            assert_eq!(modal.key(KeyCode::Char(ch), KeyModifiers::NONE), AskModalAction::None);
        }
        assert_eq!(modal.custom_draft, "почти");
        assert_eq!(
            modal.key(KeyCode::Enter, KeyModifiers::NONE),
            AskModalAction::Submit("почти".into())
        );
    }

    #[test]
    fn custom_mode_esc_returns_to_list_without_closing() {
        let mut modal = AskModal::new("r1".into(), no_timeout(single_spec()));
        modal.custom_mode = true;
        modal.custom_draft = "черновик".into();
        assert_eq!(modal.key(KeyCode::Esc, KeyModifiers::NONE), AskModalAction::None);
        assert!(!modal.custom_mode);
        // Draft survives the trip back to the list.
        assert_eq!(modal.custom_draft, "черновик");
    }

    #[test]
    fn custom_mode_enter_with_empty_draft_is_ignored() {
        let mut modal = AskModal::new("r1".into(), no_timeout(single_spec()));
        modal.custom_mode = true;
        assert_eq!(modal.key(KeyCode::Enter, KeyModifiers::NONE), AskModalAction::None);
    }

    #[test]
    fn custom_mode_backspace_edits_draft() {
        let mut modal = AskModal::new("r1".into(), no_timeout(single_spec()));
        modal.custom_mode = true;
        modal.custom_draft = "ab".into();
        assert_eq!(modal.key(KeyCode::Backspace, KeyModifiers::NONE), AskModalAction::None);
        assert_eq!(modal.custom_draft, "a");
    }

    #[test]
    fn esc_on_list_cancels() {
        let mut modal = AskModal::new("r1".into(), no_timeout(single_spec()));
        assert_eq!(modal.key(KeyCode::Esc, KeyModifiers::NONE), AskModalAction::Cancel);
    }

    #[test]
    fn enter_in_multi_select_without_checks_is_ignored() {
        let mut modal = AskModal::new("r1".into(), no_timeout(multi_spec()));
        assert_eq!(modal.key(KeyCode::Enter, KeyModifiers::NONE), AskModalAction::None);
    }

    #[test]
    fn enter_in_multi_select_joins_checked_labels() {
        let mut modal = AskModal::new("r1".into(), no_timeout(multi_spec()));
        modal.checked[0] = true;
        modal.checked[2] = true;
        assert_eq!(
            modal.key(KeyCode::Enter, KeyModifiers::NONE),
            AskModalAction::Submit("Да; Отмена".into())
        );
    }

    #[test]
    fn enter_in_single_select_submits_cursor_option() {
        let mut modal = AskModal::new("r1".into(), no_timeout(single_spec()));
        modal.cursor = 1;
        assert_eq!(
            modal.key(KeyCode::Enter, KeyModifiers::NONE),
            AskModalAction::Submit("Нет".into())
        );
    }

    #[test]
    fn timeout_text_shows_remaining_time() {
        let spec = AskSpecUi { timeout_secs: 95, ..single_spec() };
        let modal = AskModal::new("r1".into(), spec);
        let text = modal.action_timeout_text();
        assert!(text.starts_with("⏳ 01:"), "unexpected timer text: {text}");
    }

    #[test]
    fn answer_preview_reports_no_answer_when_nothing_checked() {
        let modal = AskModal::new("r1".into(), no_timeout(multi_spec()));
        assert_eq!(modal.answer_preview(), NO_ANSWER);
    }

    // ---- render tests ----

    fn render_lines(modal: &AskModal, width: u16, height: u16) -> Vec<String> {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))
            .expect("failed to create test terminal");
        terminal
            .draw(|frame| draw_ask_modal(frame, modal))
            .expect("failed to draw ask modal");
        let buf = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn render_draws_box_with_header_and_question() {
        let modal = AskModal::new("r1".into(), single_spec());
        let lines = render_lines(&modal, 100, 20);
        assert!(
            lines.iter().any(|l| l.contains('╭')),
            "rounded top border should be rendered:\n{}",
            lines.join("\n")
        );
        assert!(
            lines.iter().any(|l| l.contains("Подтверждение")),
            "header should be rendered:\n{}",
            lines.join("\n")
        );
        assert!(
            lines.iter().any(|l| l.contains("Продолжить?")),
            "question should be rendered:\n{}",
            lines.join("\n")
        );
    }

    #[test]
    fn render_multi_select_shows_checkboxes_and_hint() {
        let modal = AskModal::new("r1".into(), multi_spec());
        let lines = render_lines(&modal, 100, 20);
        assert!(
            lines.iter().any(|l| l.contains("[ ]")),
            "multi-select should show checkboxes:\n{}",
            lines.join("\n")
        );
        assert!(
            lines.iter().any(|l| l.contains("Space")),
            "multi-select hint should mention Space:\n{}",
            lines.join("\n")
        );
    }

    #[test]
    fn render_cursor_row_has_marker() {
        let modal = AskModal::new("r1".into(), single_spec());
        let lines = render_lines(&modal, 100, 20);
        assert!(
            lines.iter().any(|l| l.contains("› [ ]") || l.contains("› 1.")),
            "cursor row should have the › marker:\n{}",
            lines.join("\n")
        );
    }

    #[test]
    fn render_custom_mode_shows_draft_and_hint() {
        let mut modal = AskModal::new("r1".into(), no_timeout(single_spec()));
        modal.cursor = modal.custom_row();
        modal.custom_mode = true;
        modal.custom_draft = "мой ответ".into();
        let lines = render_lines(&modal, 100, 20);
        assert!(
            lines.iter().any(|l| l.contains("> мой ответ")),
            "custom draft should be rendered:\n{}",
            lines.join("\n")
        );
        assert!(
            lines.iter().any(|l| l.contains("Esc к списку")),
            "custom mode hint should be rendered:\n{}",
            lines.join("\n")
        );
    }

    #[test]
    fn render_single_select_hides_checkboxes() {
        let modal = AskModal::new("r1".into(), no_timeout(single_spec()));
        let lines = render_lines(&modal, 100, 20);
        assert!(
            !lines.iter().any(|l| l.contains("[ ]")),
            "single-select should not show checkboxes:\n{}",
            lines.join("\n")
        );
        assert!(
            lines.iter().any(|l| l.contains("1.")),
            "single-select should number options:\n{}",
            lines.join("\n")
        );
    }

    #[test]
    fn render_custom_variant_row_present() {
        let modal = AskModal::new("r1".into(), no_timeout(single_spec()));
        let lines = render_lines(&modal, 100, 20);
        assert!(
            lines.iter().any(|l| l.contains("Свой вариант")),
            "own-answer row should be rendered:\n{}",
            lines.join("\n")
        );
    }

    // Fork: end-to-end seam tests — a `ServerEvent::StdinRequest` carrying the
    // structured ask spec opens the modal, modal keys stage the answer, and the
    // textual fallback still works when the spec is absent.
    fn wire_spec(multiple: bool) -> jcode_protocol::AskSpec {
        jcode_protocol::AskSpec {
            header: "Deploy".into(),
            question: "Which environment?".into(),
            options: vec![
                jcode_protocol::AskOptionSpec {
                    label: "Staging".into(),
                    description: "test cluster".into(),
                },
                jcode_protocol::AskOptionSpec {
                    label: "Production".into(),
                    description: String::new(),
                },
            ],
            multiple,
            timeout_secs: 600,
            question_index: 1,
            question_total: 2,
        }
    }

    fn stdin_request(ask: Option<jcode_protocol::AskSpec>) -> crate::protocol::ServerEvent {
        crate::protocol::ServerEvent::StdinRequest {
            request_id: "ask-42".into(),
            prompt: "❓ Deploy: Which environment?\n  [1] Staging\n  [2] Production\n".into(),
            is_password: false,
            tool_call_id: String::new(),
            source: "ask_user".into(),
            ask,
        }
    }

    #[test]
    fn stdin_request_with_spec_opens_modal_and_digit_submits() {
        let mut app = crate::tui::app::tests::create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        let event = stdin_request(Some(wire_spec(false)));
        app.handle_server_event(event, &mut remote);

        assert!(app.pending_ask_modal.is_some(), "modal should open");
        assert_eq!(app.pending_stdin.as_ref().unwrap().0, "ask-42");

        // Digit "2" instantly submits the highlighted option label.
        assert!(super::handle_modal_key(
            &mut app,
            KeyCode::Char('2'),
            KeyModifiers::NONE
        ));
        assert!(app.pending_ask_modal.is_none(), "modal should close");
        let (request_id, answer) = app.pending_ask_answer.clone().unwrap();
        assert_eq!(request_id, "ask-42");
        assert_eq!(answer, "Production");
        // pending_stdin is intentionally kept: send_answer reads request_id
        // from it when flushing the staged answer.
        assert!(app.pending_stdin.is_some());
        // Fork: the dispatcher must be armed so the staged answer flushes at
        // once instead of waiting for the next server event (a blocked
        // ask_user tool produces none).
        assert!(app.pending_queued_dispatch);
    }

    #[test]
    fn stdin_request_multi_select_toggles_then_joins_labels() {
        let mut app = crate::tui::app::tests::create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_server_event(stdin_request(Some(wire_spec(true))), &mut remote);

        assert!(super::handle_modal_key(&mut app, KeyCode::Char('1'), KeyModifiers::NONE));
        assert!(super::handle_modal_key(&mut app, KeyCode::Char('2'), KeyModifiers::NONE));
        assert!(super::handle_modal_key(&mut app, KeyCode::Enter, KeyModifiers::NONE));
        let (_, answer) = app.pending_ask_answer.clone().unwrap();
        assert_eq!(answer, "Staging; Production");
    }

    #[test]
    fn stdin_request_esc_answers_no_answer() {
        let mut app = crate::tui::app::tests::create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_server_event(stdin_request(Some(wire_spec(false))), &mut remote);
        assert!(super::handle_modal_key(&mut app, KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.pending_ask_modal.is_none());
        let (_, answer) = app.pending_ask_answer.clone().unwrap();
        assert_eq!(answer, NO_ANSWER);
    }

    #[test]
    fn stdin_request_without_spec_keeps_textual_fallback() {
        let mut app = crate::tui::app::tests::create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_server_event(stdin_request(None), &mut remote);
        assert!(app.pending_ask_modal.is_none(), "no modal without a spec");
        assert!(app.pending_stdin.is_some(), "typed-answer interception stays");
    }

    #[test]
    fn clear_if_pending_closes_modal() {
        let mut app = crate::tui::app::tests::create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_server_event(stdin_request(Some(wire_spec(false))), &mut remote);
        assert!(app.pending_ask_modal.is_some());
        crate::tui::app::fork_ask::clear_if_pending(&mut app);
        assert!(app.pending_ask_modal.is_none());
        assert!(app.pending_stdin.is_none());
        assert!(app.pending_ask_answer.is_none());
    }

    #[test]
    fn resolved_elsewhere_closes_modal_and_reports_answer() {
        let mut app = crate::tui::app::tests::create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_server_event(stdin_request(Some(wire_spec(true))), &mut remote);
        assert!(app.pending_ask_modal.is_some());

        crate::tui::app::fork_ask::on_question_resolved_elsewhere(&mut app, "из Telegram");
        assert!(app.pending_ask_modal.is_none());
        assert!(app.pending_stdin.is_none());
        assert!(app.pending_ask_answer.is_none());
        // The transcript must say which answer won the race.
        let last = app.display_messages.last().unwrap();
        assert!(last.content.contains("из Telegram"));
    }

    #[test]
    fn resolved_elsewhere_is_noop_without_open_modal() {
        let mut app = crate::tui::app::tests::create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        // No modal open: a stale event must not push a stray transcript line.
        crate::tui::app::fork_ask::on_question_resolved_elsewhere(&mut app, "поздно");
        assert!(app.display_messages.is_empty());
    }
}
