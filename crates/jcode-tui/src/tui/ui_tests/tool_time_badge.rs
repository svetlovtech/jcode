//! Fork: acceptance tests for the tool row time badge (timestamp + duration
//! + severity coloring) and the configured UTC offset rendering. Split from
//! ui_tests/tools.rs so the shared tool-summary tests stay focused.

use super::*;

/// Fork acceptance proof: a completed tool row with a stored timestamp and
/// duration renders the time badge through the real render_tool_message
/// pipeline: time-of-day HH:MM:SS plus the compact duration, after the
/// token count. This is the exact path the transcript draws every frame.
#[test]
fn test_tool_row_renders_time_and_duration_badge() {
    let _lock = viewport_snapshot_test_lock();
    let _config_guard = isolate_config_home();
    let stamp = chrono::DateTime::parse_from_rfc3339("2026-09-23T20:15:42Z")
        .expect("parse stamp")
        .with_timezone(&chrono::Local);
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "ok".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call-1".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({ "command": "echo ok" }),
            intent: Some("Acceptance: time badge".to_string()),
            thought_signature: None,
        }),
        timestamp: Some(stamp.with_timezone(&chrono::Utc)),
        tool_duration_ms: Some(48_300),
    };

    let lines = messages::render_tool_message(&msg, 200, crate::config::DiffDisplayMode::Off);
    let row: String = lines
        .first()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .unwrap_or_default();

    let expected_stamp = stamp.format("%H:%M:%S").to_string();
    assert!(
        row.contains(&expected_stamp),
        "time-of-day stamp missing from rendered row: {row}"
    );
    assert!(
        row.contains("48.3s"),
        "duration badge missing from rendered row: {row}"
    );
    assert!(
        row.contains("tok"),
        "token badge must stay: {row}"
    );
}

/// A live row without stored time data (older server, pending reload) keeps
/// the classic token-only badge: no empty separator pair.
#[test]
fn test_tool_row_without_time_data_has_no_badge() {
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "ok".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call-2".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({ "command": "echo ok" }),
            intent: None,
            thought_signature: None,
        }),
        timestamp: None,
        tool_duration_ms: None,
    };

    let lines = messages::render_tool_message(&msg, 200, crate::config::DiffDisplayMode::Off);
    let row: String = lines
        .first()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .unwrap_or_default();
    assert!(!row.contains("::"), "no time stamp expected: {row}");
}

/// Fork: at a narrow width the time badge must survive as part of the
/// preserved suffix (token badge + time badge stay, summary truncates) —
/// the same guarantee the token badge already has.
#[test]
fn test_tool_row_time_badge_survives_narrow_width() {
    let _lock = viewport_snapshot_test_lock();
    let _config_guard = isolate_config_home();
    let stamp = chrono::DateTime::parse_from_rfc3339("2026-09-23T20:15:42Z")
        .expect("parse stamp")
        .with_timezone(&chrono::Utc);
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "ok".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call-n".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({ "command": "echo a-very-long-command-output" }),
            intent: Some("a very long intent summary that must truncate first".to_string()),
            thought_signature: None,
        }),
        timestamp: Some(stamp),
        tool_duration_ms: Some(48_300),
    };

    let expected_stamp = stamp.with_timezone(&chrono::Local).format("%H:%M:%S").to_string();
    for width in [40, 56, 72, 120] {
        let lines = messages::render_tool_message(&msg, width, crate::config::DiffDisplayMode::Off);
        let row: String = lines
            .first()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .unwrap_or_default();
        assert!(
            row.contains(&expected_stamp),
            "stamp lost at width {width}: {row}"
        );
        assert!(row.contains("48.3s"), "duration lost at width {width}: {row}");
        assert!(row.contains("tok"), "tokens lost at width {width}: {row}");
        let stamp_pos = row.find(&expected_stamp).expect("stamp present");
        let tok_pos = row.find("tok").expect("tokens present");
        assert!(
            tok_pos < stamp_pos,
            "time badge must trail the token badge at width {width}: {row}"
        );
    }
}

/// Fork: observed-behavior proof for the user-reported "0.0s" complaint.
/// Renders a near-instant tool row (45 ms) through the real pipeline and
/// prints the resulting line so the observation lands in the test log; the
/// assertions pin the contract: ms duration shown, no "0.0s", no bare "0s".
#[test]
fn test_tool_row_ms_duration_observed_output() {
    let _lock = viewport_snapshot_test_lock();
    let _config_guard = isolate_config_home();
    let stamp = chrono::DateTime::parse_from_rfc3339("2026-09-23T20:23:35Z")
        .expect("parse stamp")
        .with_timezone(&chrono::Utc);
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "ok".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call-ms".to_string(),
            name: "agentgrep".to_string(),
            input: serde_json::json!({ "query": "find it" }),
            intent: Some("Find implementation".to_string()),
            thought_signature: None,
        }),
        timestamp: Some(stamp),
        tool_duration_ms: Some(45),
    };

    let lines = messages::render_tool_message(&msg, 200, crate::config::DiffDisplayMode::Off);
    let row: String = lines
        .first()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .unwrap_or_default();
    println!("observed tool row: {row}");

    let expected_stamp = stamp.with_timezone(&chrono::Local).format("%H:%M:%S").to_string();
    assert!(row.contains("45ms"), "ms duration missing: {row}");
    assert!(!row.contains("0.0s"), "0.0s must be gone: {row}");
    assert!(!row.contains("0ms"), "0ms must be gone: {row}");
    assert!(row.contains(&expected_stamp), "stamp missing: {row}");
}

/// Fork: end-to-end acceptance of the user's UTC+3 request — the exact
/// scenario from their screenshot (near-instant agentgrep row) rendered
/// with display.timestamp_tz = "UTC+3" set through the isolated config
/// home. The observed row must show the UTC+3 stamp (20:23:35Z stored ->
/// 23:23:35 displayed), ms duration, and no 0.0s.
#[test]
fn test_tool_row_stamp_renders_in_configured_utc3() {
    let _lock = viewport_snapshot_test_lock();
    let _guard = isolate_config_home_with(
        "[display]\nfooter_style = \"advanced\"\ntimestamp_tz = \"UTC+3\"\n",
    );
    let stamp = chrono::DateTime::parse_from_rfc3339("2026-09-23T20:23:35Z")
        .expect("parse stamp")
        .with_timezone(&chrono::Utc);
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "ok".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call-tz".to_string(),
            name: "agentgrep".to_string(),
            input: serde_json::json!({ "query": "x" }),
            intent: Some("UTC+3 acceptance".to_string()),
            thought_signature: None,
        }),
        timestamp: Some(stamp),
        tool_duration_ms: Some(45),
    };

    let lines = messages::render_tool_message(&msg, 200, crate::config::DiffDisplayMode::Off);
    let row: String = lines
        .first()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .unwrap_or_default();
    println!("observed UTC+3 row: {row}");

    assert!(
        row.contains("23:23:35"),
        "UTC+3 stamp (20:23Z + 3h = 23:23) missing: {row}"
    );
    assert!(row.contains("45ms"), "ms duration missing: {row}");
    assert!(!row.contains("0.0s"), "0.0s banned: {row}");
}


/// Fork: pin the exact in-binary UTC+3 arithmetic on the live-session data:
/// stored 2026-09-23T21:51:28Z with timestamp_tz=UTC+3 must render
/// 00:51:28 (next day). Complements the acceptance test with a deterministic
/// no-local-timezone case (the machine's TZ cannot influence it).
#[test]
fn test_tool_row_utc3_exact_offset_arithmetic() {
    let _lock = viewport_snapshot_test_lock();
    let _guard = isolate_config_home_with("[display]\ntimestamp_tz = \"UTC+3\"\n");
    let stamp = chrono::DateTime::parse_from_rfc3339("2026-09-23T21:51:28Z")
        .expect("parse stamp")
        .with_timezone(&chrono::Utc);
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "ok".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call-utc3".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({ "command": "echo tz-probe-ok" }),
            intent: None,
            thought_signature: None,
        }),
        timestamp: Some(stamp),
        tool_duration_ms: Some(2_325),
    };

    let lines = messages::render_tool_message(&msg, 200, crate::config::DiffDisplayMode::Off);
    let row: String = lines
        .first()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .unwrap_or_default();
    println!("observed: {row}");
    assert!(
        row.contains("00:51:28"),
        "21:51:28Z + 3h must be 00:51:28 UTC+3: {row}"
    );
    assert!(row.contains("2.3s"), "2_325ms must render 2.3s: {row}");
}

