//! pi-style footer spans (`display.footer_style = "pi"`), isolated from
//! `ui_input.rs` so upstream merges touch one small call site.

use super::*;

/// provider token, reasoning effort, context bar, and (when available) session
/// cost and total tokens. Unavailable pieces are omitted instead of rendered
/// as placeholders.
pub(super) fn overscroll_pi_spans(
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
        // Session cost, when the provider reports a nonzero total.
        if usage.total_cost > 0.0 {
            push(
                &mut spans,
                Span::styled(
                    overscroll_format_cost(usage.total_cost),
                    Style::default().fg(rgb(150, 200, 150)),
                ),
            );
        }
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

    spans
}

/// Format a cost as a dollar amount with 2-4 decimals: 0.0123 -> "$0.0123",
/// 1.2 -> "$1.20". Trailing zeros are trimmed but at least two decimals stay.
fn overscroll_format_cost(cost: f32) -> String {
    let formatted = format!("{:.4}", cost);
    let (int_part, frac) = formatted
        .split_once('.')
        .unwrap_or((formatted.as_str(), ""));
    let mut frac = frac.trim_end_matches('0').to_string();
    while frac.len() < 2 {
        frac.push('0');
    }
    format!("${int_part}.{frac}")
}
