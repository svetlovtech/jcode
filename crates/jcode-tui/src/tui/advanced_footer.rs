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

    // Fork: session time. While the agent works, show the wall-clock session
    // age plus the summed runtime of this session's subagents; when idle,
    // show how long ago the last activity was instead of a ticking clock so
    // the span stays stable and does not nag.
    if let Some(time_span) = session_time_spans(app) {
        push(&mut spans, time_span);
    }

    // Fork: chat-integration availability (pi's tg-bridge indicator).
    if let Some(chat_span) = crate::tui::chat_status::footer_span() {
        push(&mut spans, chat_span);
    }

    spans
}

/// Fork: compact duration, tuned for a footer span: "42s", "12m 5s",
/// "1h 23m", "3d 4h". Skips zero leading units so the label never grows.
fn format_footer_duration(secs: u64) -> String {
    if secs < 60 {
        return format!("{secs}s");
    }
    let mins = secs / 60;
    if mins < 60 {
        let s = secs % 60;
        if s > 0 {
            return format!("{mins}m {s}s");
        }
        return format!("{mins}m");
    }
    let hours = mins / 60;
    if hours < 24 {
        let m = mins % 60;
        if m > 0 {
            return format!("{hours}h {m}m");
        }
        return format!("{hours}h");
    }
    let days = hours / 24;
    let h = hours % 24;
    if h > 0 {
        format!("{days}d {h}h")
    } else {
        format!("{days}d")
    }
}

/// Fork: footer span with session time and subagent runtime.
///
/// Active turn: `⏱ <session age> · Σ agents <summed subagent runtime>`.
/// Idle:        `⏱ <session age> · idle <since last activity>`.
/// Subagent runtime sums the live `elapsed_secs` of running members plus the
/// last-known runtime of finished ones; hidden entirely when there are none.
fn session_time_spans(app: &dyn TuiState) -> Option<ratatui::text::Span<'static>> {
    let up_secs = app.session_age_secs()?;
    let mut text = format!("⏱ {}", format_footer_duration(up_secs));

    let members = app.swarm_members_for_transcript();
    let agent_secs: u64 = members
        .iter()
        .filter_map(|m| m.runtime.elapsed_secs)
        .sum();
    if !members.is_empty() {
        text.push_str(&format!(" · Σ agents {}", format_footer_duration(agent_secs)));
    }

    if !app.is_processing() {
        if let Some(idle) = app.time_since_activity() {
            text.push_str(&format!(" · idle {}", format_footer_duration(idle.as_secs())));
        }
    }

    Some(ratatui::text::Span::styled(
        text,
        Style::default().fg(rgb(140, 140, 150)),
    ))
}

#[cfg(test)]
mod duration_format_tests {
    use super::format_footer_duration;

    #[test]
    fn footer_duration_buckets() {
        assert_eq!(format_footer_duration(0), "0s");
        assert_eq!(format_footer_duration(42), "42s");
        assert_eq!(format_footer_duration(60), "1m");
        assert_eq!(format_footer_duration(125), "2m 5s");
        assert_eq!(format_footer_duration(3600), "1h");
        assert_eq!(format_footer_duration(4980), "1h 23m");
        assert_eq!(format_footer_duration(90000), "1d 1h");
    }
}
