//! Fork: tests for the advanced footer (`display.footer_style = "advanced"`),
//! especially the session token total (Σ) and cost spans.

use super::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn advanced_footer_state(total_cost: f32, input: u64, output: u64) -> TestState {
    let info_widget_data = info_widget::InfoWidgetData {
        model: Some("glm-5.3-flash".to_string()),
        reasoning_effort: Some("max".to_string()),
        context_limit: Some(1_000_000),
        provider_name: Some("zai-gateway".to_string()),
        auth_method: info_widget::AuthMethod::ApiKey,
        observed_context_tokens: Some(24_000),
        usage_info: Some(info_widget::UsageInfo {
            provider: info_widget::UsageProvider::CostBased,
            total_cost,
            input_tokens: input,
            output_tokens: output,
            available: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    TestState {
        provider_name: Some("zai-gateway".to_string()),
        provider_model: Some("glm-5.3-flash".to_string()),
        working_dir: Some("/tmp/proj".to_string()),
        info_widget_data,
        suppress_info_widgets: true,
        display_messages: vec![DisplayMessage::assistant("tail line")],
        messages_version: 1,
        ..Default::default()
    }
}

fn draw_footer(state: &TestState, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("failed to create test terminal");
    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, width, height);
            crate::tui::ui::input_ui::draw_overscroll_status(frame, state, area);
        })
        .expect("failed to draw footer");
    let buf = terminal.backend().buffer();
    let mut lines = Vec::with_capacity(height as usize);
    for y in 0..height {
        let mut line = String::with_capacity(width as usize);
        for x in 0..width {
            line.push_str(buf[(x, y)].symbol());
        }
        lines.push(line.trim_end().to_string());
    }
    lines
}

#[test]
fn advanced_footer_shows_total_session_tokens() {
    let state = advanced_footer_state(0.0, 150_000, 5_000);
    let rows = draw_footer(&state, 160, 1);
    let line = &rows[0];
    assert!(line.contains('Σ'), "Σ marker missing: {line}");
    // 155_000 tokens -> "155k".
    assert!(line.contains("155k"), "token total missing: {line}");
}

// Fork: cost was removed from the advanced footer; the chat-integration
// availability indicator (⚪/🟢/🔴 chat) took its slot. Whether the chat
// span appears depends on the machine's ~/.jcode/config.toml (it is hidden
// when [chat] is not configured), so only the cost removal is asserted.
#[test]
fn advanced_footer_hides_cost() {
    let state = advanced_footer_state(0.1234, 10_000, 1_000);
    let rows = draw_footer(&state, 160, 1);
    let line = &rows[0];
    assert!(!line.contains('$'), "cost must be gone: {line}");
}

#[test]
fn advanced_footer_omits_cost_when_zero() {
    let state = advanced_footer_state(0.0, 2_000, 500);
    let rows = draw_footer(&state, 160, 1);
    let line = &rows[0];
    assert!(line.contains('Σ'), "Σ marker missing: {line}");
    assert!(!line.contains('$'), "zero cost must be omitted: {line}");
}
