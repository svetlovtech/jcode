//! Advanced footer spans (`display.footer_style = "advanced"`), isolated from
//! `ui_input.rs` so upstream merges touch one small call site.

use super::*;

/// provider token, reasoning effort, context bar, and (when available) session
/// cost and total tokens. Unavailable pieces are omitted instead of rendered
/// as placeholders.
pub(super) fn overscroll_advanced_spans(
    app: &dyn TuiState,
    data: &crate::tui::info_widget::InfoWidgetData,
) -> Vec<Span<'static>> {
    let sep = || Span::styled(" · ", Style::default().fg(rgb(100, 100, 110)));
    let mut spans: Vec<Span<'static>> = Vec::new();
    let push = |spans: &mut Vec<Span<'static>>, span: Span<'static>| {
        if !spans.is_empty() {
            spans.push(sep());
        }
        spans.push(span);
    };

    // Working directory basename.
    if let Some(dir) = app.working_dir().and_then(|d| overscroll_dir_label(&d)) {
        push(
            &mut spans,
            Span::styled(dir, Style::default().fg(rgb(140, 140, 150))),
        );
    }

    // Git branch.
    if let Some(branch) = overscroll_git_branch(data) {
        push(
            &mut spans,
            Span::styled(branch, Style::default().fg(rgb(150, 170, 140))),
        );
    }

    // Model plus a compact provider token in parentheses, e.g. "glm-5 (zai)".
    let model = data
        .model
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| app.provider_model());
    if !model.is_empty() && !overscroll_is_placeholder(&model) {
        push(
            &mut spans,
            Span::styled(
                session_facts::pretty_model(&model),
                Style::default().fg(rgb(255, 150, 200)).bold(),
            ),
        );
        let provider = data
            .provider_name
            .clone()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| app.provider_name());
        if !provider.is_empty() && !overscroll_is_runtime_placeholder(&provider) {
            // Compact provider token: first word lowercased ("Zai Coding Plan" -> "zai").
            let token = provider
                .split_whitespace()
                .next()
                .unwrap_or(provider.as_str())
                .to_ascii_lowercase();
            spans.push(Span::styled(
                format!(" ({token})"),
                Style::default().fg(rgb(140, 180, 255)),
            ));
        }
        // Reasoning level, e.g. " high".
        if let Some(effort) = data
            .reasoning_effort
            .as_deref()
            .and_then(overscroll_short_reasoning)
        {
            spans.push(Span::styled(
                format!(" {}", effort),
                Style::default().fg(rgb(140, 140, 150)),
            ));
        }
    }

    // Context usage bar (includes the percentage label).
    if let Some((used, limit)) = overscroll_context_usage(data) {
        push(
            &mut spans,
            Span::styled(
                format!(
                    "{}/{} ",
                    overscroll_format_tokens(used),
                    overscroll_format_tokens(limit)
                ),
                Style::default().fg(rgb(140, 140, 150)),
            ),
        );
        spans.extend(overscroll_context_bar(used, limit, 10));
    }

    if let Some(usage) = data.usage_info.as_ref() {
        // Total session tokens (input + output).
        let total_tokens = usage.input_tokens.saturating_add(usage.output_tokens);
        if total_tokens > 0 {
            push(
                &mut spans,
                Span::styled(
                    format!("Σ {}", overscroll_format_tokens(total_tokens as usize)),
                    Style::default().fg(rgb(160, 160, 160)),
                ),
            );
        }
    }

    // Fork: chat-integration availability (pi's tg-bridge indicator).
    if let Some(chat_span) = crate::tui::chat_status::footer_span() {
        push(&mut spans, chat_span);
    }

    spans
}

/// The upstream (classic) overscroll status span list. Verbatim upstream body,
/// moved here so `ui_input.rs` keeps a single fork delegation point.
pub(super) fn overscroll_classic_spans(
    app: &dyn TuiState,
    data: &crate::tui::info_widget::InfoWidgetData,
    sep: &dyn Fn() -> Span<'static>,
) -> Vec<Span<'static>> {
    let mut spans: Vec<Span> = Vec::new();

    // Model
    let model = data
        .model
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| app.provider_model());
    if !model.is_empty() && !overscroll_is_placeholder(&model) {
        spans.push(Span::styled(
            session_facts::pretty_model(&model),
            Style::default().fg(rgb(255, 150, 200)).bold(),
        ));
        // Reasoning level shown inline next to the model, e.g. " high".
        if let Some(effort) = data
            .reasoning_effort
            .as_deref()
            .and_then(overscroll_short_reasoning)
        {
            spans.push(Span::styled(
                format!(" {}", effort),
                Style::default().fg(rgb(140, 140, 150)),
            ));
        }
    }

    // Provider
    let provider = data
        .provider_name
        .clone()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| app.provider_name());
    if !provider.is_empty() && !overscroll_is_runtime_placeholder(&provider) {
        if !spans.is_empty() {
            spans.push(sep());
        }
        spans.push(Span::styled(
            overscroll_provider_display(&provider),
            Style::default().fg(rgb(140, 180, 255)),
        ));
    }

    // Access method (auth)
    if let Some((label, color)) = overscroll_auth_label(data.auth_method) {
        if !spans.is_empty() {
            spans.push(sep());
        }
        spans.push(Span::styled(label.to_string(), Style::default().fg(color)));
    }

    // Context usage as a rounded bar
    if let Some((used, limit)) = overscroll_context_usage(data) {
        if !spans.is_empty() {
            spans.push(sep());
        }
        spans.push(Span::styled(
            format!(
                "{}/{} ",
                overscroll_format_tokens(used),
                overscroll_format_tokens(limit)
            ),
            Style::default().fg(rgb(140, 140, 150)),
        ));
        spans.extend(overscroll_context_bar(used, limit, 10));
    }

    // Working directory last, shown as a home-relative path, with the git
    // branch alongside when available.
    if let Some(dir) = app.working_dir().and_then(|d| overscroll_dir_label(&d)) {
        if !spans.is_empty() {
            spans.push(sep());
        }
        spans.push(Span::styled(" ", Style::default().fg(rgb(140, 180, 255))));
        spans.push(Span::styled(dir, Style::default().fg(rgb(140, 140, 150))));
        if let Some(branch) = overscroll_git_branch(data) {
            spans.push(Span::styled(
                format!("  {branch}"),
                Style::default().fg(rgb(150, 170, 140)),
            ));
        }
    }

    spans
}
