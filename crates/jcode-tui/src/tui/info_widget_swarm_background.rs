use super::{BackgroundInfo, InfoWidgetData, SwarmInfo, truncate_smart};
use crate::protocol::SwarmMemberStatus;
use crate::tui::color_support::rgb;
use ratatui::prelude::*;
use unicode_width::UnicodeWidthStr;

pub(super) fn render_swarm_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.swarm_info else {
        return Vec::new();
    };

    // Dock mode: this session manages agents, render the compact two-line
    // summary (agents tally + task-graph node bar).
    if !info.managed_members.is_empty() {
        return crate::tui::info_widget::swarm_gallery::render_swarm_compact_lines(
            &info.managed_members,
            info.plan_progress,
            inner.width as usize,
            inner.height as usize,
        );
    }

    let mut lines: Vec<Line> = vec![render_swarm_stats_line(info)];

    if info.members.is_empty()
        && let Some(status) = &info.subagent_status
    {
        lines.push(Line::from(vec![
            Span::styled("▶ ", Style::default().fg(rgb(255, 200, 100))),
            Span::styled(
                truncate_smart(status, inner.width.saturating_sub(4) as usize),
                Style::default().fg(rgb(200, 200, 210)),
            ),
        ]));
    }

    let max_names = inner.height.saturating_sub(lines.len() as u16) as usize;
    let max_name_len = inner.width.saturating_sub(6) as usize;
    if !info.members.is_empty() {
        for member in info.members.iter().take(max_names.min(3)) {
            lines.push(swarm_member_line(member, max_name_len));
        }
    } else {
        for name in info.session_names.iter().take(max_names.min(3)) {
            lines.push(render_swarm_name_line(name, max_name_len));
        }
    }

    lines
}

pub(super) fn render_background_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.background_info else {
        return Vec::new();
    };

    render_background_lines(info, inner.width as usize)
}

pub(super) fn render_background_compact(info: &BackgroundInfo, width: usize) -> Vec<Line<'static>> {
    render_background_lines(info, width)
}

fn swarm_member_label(member: &SwarmMemberStatus) -> String {
    member
        .friendly_name
        .clone()
        .unwrap_or_else(|| member.session_id.chars().take(8).collect())
}

fn swarm_status_style(status: &str) -> (Color, &'static str) {
    match status {
        "spawned" => (rgb(140, 140, 150), "○"),
        "ready" => (rgb(120, 180, 120), "●"),
        "running" => (rgb(255, 200, 100), "▶"),
        "blocked" => (rgb(255, 170, 80), "⏸"),
        "failed" => (rgb(255, 100, 100), "✗"),
        "completed" => (rgb(100, 200, 100), "✓"),
        "stopped" => (rgb(140, 140, 150), "■"),
        "crashed" => (rgb(255, 80, 80), "!"),
        _ => (rgb(140, 140, 150), "·"),
    }
}

fn swarm_role_prefix(member: &SwarmMemberStatus) -> &'static str {
    match member.role.as_deref() {
        Some("coordinator") => "★ ",
        _ => "  ",
    }
}

fn swarm_member_line(member: &SwarmMemberStatus, max_width: usize) -> Line<'static> {
    let name = swarm_member_label(member);
    let mut detail = member.detail.clone().unwrap_or_default();
    if !detail.is_empty() {
        detail = format!(" - {}", detail);
    }
    let role_prefix = swarm_role_prefix(member);
    let line_text = truncate_smart(&format!("{} {}{}", name, member.status, detail), max_width);
    let (color, icon) = swarm_status_style(&member.status);
    Line::from(vec![
        Span::styled(
            role_prefix.to_string(),
            Style::default().fg(rgb(255, 200, 100)),
        ),
        Span::styled(format!("{} ", icon), Style::default().fg(color)),
        Span::styled(line_text, Style::default().fg(rgb(140, 140, 150))),
    ])
}

fn render_swarm_stats_line(info: &SwarmInfo) -> Line<'static> {
    let mut stats_parts: Vec<Span> =
        vec![Span::styled("🐝 ", Style::default().fg(rgb(255, 200, 100)))];

    if info.session_count > 0 {
        stats_parts.push(Span::styled(
            format!("{}s", info.session_count),
            Style::default().fg(rgb(160, 160, 170)),
        ));
    }
    if let Some(clients) = info.client_count {
        if info.session_count > 0 {
            stats_parts.push(Span::styled(" · ", Style::default().fg(rgb(100, 100, 110))));
        }
        stats_parts.push(Span::styled(
            format!("{}c", clients),
            Style::default().fg(rgb(160, 160, 170)),
        ));
    }

    Line::from(stats_parts)
}

fn render_swarm_name_line(name: &str, max_name_len: usize) -> Line<'static> {
    Line::from(vec![
        Span::styled("  · ", Style::default().fg(rgb(100, 100, 110))),
        Span::styled(
            truncate_smart(name, max_name_len),
            Style::default().fg(rgb(140, 140, 150)),
        ),
    ])
}

fn render_background_lines(info: &BackgroundInfo, width: usize) -> Vec<Line<'static>> {
    let Some(summary) = background_summary(info) else {
        return Vec::new();
    };
    let mut lines = vec![Line::from(vec![
        Span::styled("⏳ ", Style::default().fg(rgb(180, 140, 255))),
        Span::styled(summary, Style::default().fg(rgb(160, 160, 170))),
    ])];
    let (rows, hidden) = background_task_rows(info, width, "  ", COMPACT_MAX_TASK_ROWS);
    lines.extend(rows);
    if hidden > 0 {
        lines.push(Line::from(vec![
            Span::styled("   ", Style::default().fg(rgb(100, 100, 110))),
            Span::styled(
                format!("+{} more", hidden),
                Style::default().fg(rgb(140, 140, 150)),
            ),
        ]));
    }
    lines
}

/// Most task rows the standalone widget lists before `+N more`.
const WIDGET_MAX_TASK_ROWS: usize = 6;

/// Most task rows the compact (Overview) form lists before `+N more`.
const COMPACT_MAX_TASK_ROWS: usize = 3;

/// Task rows plus how many were left out. Prefers the session-scoped
/// `rows` (which carry status and percent). Falls back to plain running task
/// names when only the process-local manager knows about tasks.
fn background_task_rows(
    info: &BackgroundInfo,
    width: usize,
    indent: &'static str,
    max_rows: usize,
) -> (Vec<Line<'static>>, usize) {
    let row_width = width.saturating_sub(UnicodeWidthStr::width(indent)).max(12);
    if !info.rows.is_empty() {
        // Most recently active task first.
        let total = info.rows.len();
        let shown = total.min(max_rows);
        let lines = info
            .rows
            .iter()
            .rev()
            .take(shown)
            .map(|task| {
                let mut line = background_task_row_line(task, row_width);
                if !indent.is_empty() {
                    line.spans.insert(0, Span::raw(indent));
                }
                line
            })
            .collect();
        return (lines, total - shown);
    }

    let mut lines = Vec::new();
    let shown = info.running_tasks.len().min(max_rows);
    for (index, task) in info.running_tasks.iter().take(shown).enumerate() {
        let detail = if index == 0 {
            info.progress_detail.as_deref()
        } else {
            None
        };
        let row_text = if let Some(detail) = detail {
            truncate_smart(
                &format!("{} · {}", task, detail),
                row_width.saturating_sub(2),
            )
        } else {
            truncate_smart(task, row_width.saturating_sub(2))
        };
        lines.push(Line::from(vec![
            Span::raw(indent),
            Span::styled("◌ ", Style::default().fg(rgb(180, 140, 255))),
            Span::styled(row_text, Style::default().fg(rgb(180, 180, 190))),
        ]));
    }

    (lines, info.running_tasks.len().saturating_sub(shown))
}

/// One task row: `◌ cargo test  ━━╺─── 42%`. Completed tasks show `✓` with a
/// full bar, failed tasks `×` with `failed`.
pub(crate) fn background_task_row_line(
    task: &crate::tui::BackgroundTaskRow,
    width: usize,
) -> Line<'static> {
    use crate::tui::BackgroundTaskRowStatus as Status;
    const BAR_WIDTH: usize = 6;
    let dim = rgb(110, 110, 120);
    let (icon, task_color, percent) = match task.status {
        Status::Running => (
            "◌",
            rgb(180, 140, 255),
            task.percent.unwrap_or(0.0).clamp(0.0, 100.0),
        ),
        Status::Completed => ("✓", Color::Green, 100.0),
        Status::Failed => (
            "×",
            Color::Red,
            task.percent.unwrap_or(0.0).clamp(0.0, 100.0),
        ),
    };
    let status_label = if task.status == Status::Failed {
        "failed".to_string()
    } else {
        format!("{}%", percent.round() as u8)
    };
    let filled = ((percent / 100.0) * BAR_WIDTH as f32).round() as usize;
    let (active_bar, remaining_bar) = if task.status == Status::Failed {
        (
            "━".repeat(filled.min(BAR_WIDTH)),
            "─".repeat(BAR_WIDTH.saturating_sub(filled)),
        )
    } else if filled >= BAR_WIDTH {
        ("━".repeat(BAR_WIDTH), String::new())
    } else {
        (
            format!("{}╺", "━".repeat(filled)),
            "─".repeat(BAR_WIDTH.saturating_sub(filled + 1)),
        )
    };

    // icon + space + label + two spaces + bar + space + status
    let fixed_width = UnicodeWidthStr::width(
        format!("◌   {}{} {}", active_bar, remaining_bar, status_label).as_str(),
    );
    let max_label_width = width.saturating_sub(fixed_width).max(1);
    let label = truncate_task_label(&task.label, max_label_width);

    Line::from(vec![
        Span::styled(icon, Style::default().fg(task_color)),
        Span::raw(" "),
        Span::styled(label, Style::default().fg(rgb(190, 190, 200))),
        Span::raw("  "),
        Span::styled(active_bar, Style::default().fg(task_color)),
        Span::styled(remaining_bar, Style::default().fg(dim)),
        Span::styled(format!(" {}", status_label), Style::default().fg(dim)),
    ])
}

fn truncate_task_label(label: &str, max_width: usize) -> String {
    let label = label.replace(['\r', '\n'], " ");
    if UnicodeWidthStr::width(label.as_str()) <= max_width {
        return label;
    }
    if max_width <= 1 {
        return "…".to_string();
    }
    let mut truncated = String::new();
    let mut used = 0usize;
    for ch in label.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w + 1 > max_width {
            break;
        }
        used += w;
        truncated.push(ch);
    }
    truncated.push('…');
    truncated
}

fn background_summary(info: &BackgroundInfo) -> Option<String> {
    if !info.has_content() {
        return None;
    }
    let running = if info.rows.is_empty() {
        info.running_count
    } else {
        info.rows
            .iter()
            .filter(|row| row.status == crate::tui::BackgroundTaskRowStatus::Running)
            .count()
    };
    if running == 0 {
        return Some("Background".to_string());
    }
    Some(format!("Background · {} running", running))
}
