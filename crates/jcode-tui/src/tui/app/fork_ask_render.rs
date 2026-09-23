//! Fork: rendering for the ask_user modal (`draw_ask_modal`), split out
//! of the former monolithic fork_ask_modal.rs.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};

use super::fork_ask_state::{AskModal, MODAL_BG, MODAL_BORDER};
use crate::tui::color_support::rgb;
use jcode_tui_style::theme::{accent_color, dim_color};

/// Draw the centered ask modal over the (already cleared) full frame.
pub fn draw_ask_modal(frame: &mut ratatui::Frame, modal: &AskModal) {
    let area = frame.area();
    if area.width < 8 || area.height < 5 {
        return;
    }

    let width = (area.width * 3 / 5)
        .min(80)
        .max(1)
        .min(area.width.saturating_sub(4))
        .max(1);
    let inner_width = width.saturating_sub(2) as usize;

    // Fork: the box height must be IDENTICAL in every modal state (option
    // list, custom-answer input). The modal renders as a late overlay on top
    // of the chat; if the box changed size between states, cells of the
    // previous, larger box were left on screen (stale "1. …" rows) until some
    // other keypress forced a full repaint. The height is therefore computed
    // once from the full list-mode layout, and the short custom-mode body is
    // padded with blank background lines instead.
    //
    // Fork: long questions/labels/descriptions are WORD-WRAPPED (display-width
    // aware) instead of truncated into "…", so the full text stays readable.
    // The wrapped line counts drive the height computation above.
    let spec = &modal.spec;
    let question_lines = wrap_display(&spec.question, inner_width);
    // Row budget mirrors the render below: the marker+state prefix before an
    // option label is 6 columns wide ("› " + "[x] " / "N. "), descriptions sit
    // behind a 4-column indent.
    let label_width = inner_width.saturating_sub(6);
    let desc_width = inner_width.saturating_sub(4);
    let option_row_counts: Vec<usize> = spec
        .options
        .iter()
        .map(|option| {
            let label_rows = wrap_display(&option.label, label_width).len().max(1);
            let desc_rows = if option.description.is_empty() {
                0
            } else {
                wrap_display(&option.description, desc_width).len()
            };
            label_rows + desc_rows
        })
        .collect();
    let list_body_rows = question_lines.len()
        + option_row_counts.iter().sum::<usize>()
        + 1 // "own answer" row
        + 1; // bottom hint line
    // +1 spare row: keeps a small bottom margin like the previous layout and
    // guarantees the custom-mode body (question + input + Esc hint + hint)
    // still fits even for a degenerate spec with zero options.
    let height = (2 + list_body_rows + 1).min(area.height as usize) as u16;

    let vertical = (area.height.saturating_sub(height)) / 2;
    let horizontal = (area.width.saturating_sub(width)) / 2;
    let box_area = Rect::new(horizontal, vertical, width, height.max(3));

    let bg = rgb(MODAL_BG.0, MODAL_BG.1, MODAL_BG.2);
    let border = rgb(MODAL_BORDER.0, MODAL_BORDER.1, MODAL_BORDER.2);
    let accent = accent_color();
    let dim = dim_color();

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

    let mut lines: Vec<Line> = Vec::new();

    for question_line in &question_lines {
        lines.push(Line::from(vec![Span::styled(
            question_line.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )]));
    }

    if modal.custom_mode {
        // One-line free-form input with cursor block.
        lines.push(Line::from(vec![
            Span::styled("> ", Style::default().fg(accent)),
            Span::raw(truncate_display(
                &modal.custom_draft,
                inner_width.saturating_sub(4),
            )),
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
            let marker_style = if cursor_here {
                Style::default().fg(accent)
            } else {
                Style::default()
            };
            // Wrapped label rows: the first row carries the marker/state
            // prefix, continuation rows are indented under the label start.
            let state_len = state.chars().count();
            let label_lines = wrap_display(&option.label, label_width);
            let label_lines = if label_lines.is_empty() {
                vec![String::new()]
            } else {
                label_lines
            };
            for (label_index, label_line) in label_lines.iter().enumerate() {
                if label_index == 0 {
                    lines.push(Line::from(vec![
                        Span::styled(format!("{marker} "), marker_style),
                        Span::styled(state.clone(), state_style),
                        Span::styled(label_line.clone(), label_style),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(2 + state_len)),
                        Span::styled(label_line.clone(), label_style),
                    ]));
                }
            }
            if !option.description.is_empty() {
                let description_style = Style::default().fg(dim).add_modifier(Modifier::ITALIC);
                for description_line in wrap_display(&option.description, desc_width) {
                    lines.push(Line::from(Span::styled(
                        format!("    {description_line}"),
                        description_style,
                    )));
                }
            }
        }

        let cursor_on_custom = modal.cursor == modal.custom_row();
        lines.push(Line::from(vec![
            Span::styled(
                if cursor_on_custom { "› " } else { "  " },
                if cursor_on_custom {
                    Style::default().fg(accent)
                } else {
                    Style::default()
                },
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

    let paragraph = Paragraph::new(lines)
        .block(block)
        .style(Style::default().bg(bg));
    // Fork: wipe the box area first. The modal overlays the live transcript;
    // without an explicit clear the transcript text showed through every
    // empty cell of the box (only cells the Paragraph actually wrote got the
    // background color).
    frame.render_widget(ratatui::widgets::Clear, box_area);
    frame.render_widget(paragraph, box_area);
}

/// Display-width-aware truncation with an ellipsis (mirrors the inline picker).
pub(crate) fn truncate_display(text: &str, max_width: usize) -> String {
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

/// Word-wrap `text` into lines of at most `max_width` display columns.
///
/// Breaks preferentially between words; a single word wider than the line is
/// hard-broken at a character boundary. Explicit `\n` in the text start a new
/// line. Continuation lines are returned WITHOUT any indent; callers add the
/// prefix that matches the row they continue.
pub(crate) fn wrap_display(text: &str, max_width: usize) -> Vec<String> {
    let char_width =
        |ch: char| unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut used = 0usize;
        for word in paragraph.split_whitespace() {
            let mut rest = word;
            while !rest.is_empty() {
                let rest_width = unicode_width::UnicodeWidthStr::width(rest);
                let separator = usize::from(!line.is_empty());
                if used + separator + rest_width <= max_width {
                    if separator == 1 {
                        line.push(' ');
                    }
                    line.push_str(rest);
                    used += separator + rest_width;
                    break;
                }
                if !line.is_empty() {
                    // Flush the current line and retry the whole word on a
                    // fresh one.
                    lines.push(std::mem::take(&mut line));
                    used = 0;
                    continue;
                }
                // The word alone exceeds the line width: hard-break it.
                let mut take_bytes = 0usize;
                let mut acc = 0usize;
                for ch in rest.chars() {
                    let ch_width = char_width(ch);
                    if acc + ch_width > max_width {
                        break;
                    }
                    acc += ch_width;
                    take_bytes += ch.len_utf8();
                }
                if take_bytes == 0 {
                    // Degenerate width (narrower than one character): emit the
                    // remainder as-is instead of stalling.
                    line.push_str(rest);
                    break;
                }
                let (head, tail) = rest.split_at(take_bytes);
                lines.push(head.to_string());
                rest = tail;
            }
        }
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

// Fork: adapt the wire `AskSpec` into the local UI shape. Kept as a `From`
// impl at the module boundary so the rest of the TUI never sees protocol
// types directly.