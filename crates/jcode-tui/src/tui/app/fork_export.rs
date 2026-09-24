//! Fork: `/export` writes the current session to JSON or a self-contained
//! HTML viewer (pi-harness-style, rendered by `jcode-export-core`).
//!
//! Fork-only module: no upstream conflicts. Registered in
//! `state_ui_input_helpers.rs` (`RegisteredCommand::public("/export", ...)`)
//! and dispatched from `commands_dispatch.rs`. The worker thread publishes
//! `BusEvent::SessionExportReady`; the local bus pump routes it to
//! [`App::handle_session_export_ready`].

use super::{App, DisplayMessage};
use crate::bus::{Bus, BusEvent, SessionExportReady};

/// `/export [html|json] [path]` - spawn the export worker.
pub(super) fn handle_export_command(app: &mut App, trimmed: &str) -> bool {
    let Some(rest) = super::commands::slash_command_rest(trimmed, "/export") else {
        return false;
    };

    let mut format = jcode_export_core::ExportFormat::Html;
    let mut explicit_path: Option<String> = None;
    let mut include_swarm = false;
    for token in rest.split_whitespace() {
        if token == "swarm" {
            include_swarm = true;
            continue;
        }
        match jcode_export_core::ExportFormat::parse(token) {
            Some(parsed) => format = parsed,
            None => explicit_path = Some(token.to_string()),
        }
    }

    let session = app.session.clone();
    let session_id = session.id.clone();
    let label = match format {
        jcode_export_core::ExportFormat::Html => "self-contained HTML",
        jcode_export_core::ExportFormat::Json => "JSON",
    };
    app.push_display_message(DisplayMessage::system(format!(
        "Exporting session as {label} ..."
    )));
    app.set_status_notice(format!("Export -> {label}"));

    std::thread::spawn(move || {
        let result = export_session_to_file(&session, format, explicit_path.as_deref(), include_swarm)
            .map_err(|error| error.to_string());
        Bus::global().publish(BusEvent::SessionExportReady(SessionExportReady {
            session_id,
            result,
        }));
    });

    true
}

/// Build the export payload from a session and write it to `explicit_path`
/// (or the default `<sessions-dir>/<id>.export.<ext>`).
fn export_session_to_file(
    session: &crate::session::Session,
    format: jcode_export_core::ExportFormat,
    explicit_path: Option<&str>,
    include_swarm: bool,
) -> anyhow::Result<std::path::PathBuf> {
    // Related sessions (subagents) for the per-session Gantt. Same matcher
    // as `jcode replay --swarm`; the primary session itself is filtered out.
    let related_sessions: Vec<crate::session::Session> = if include_swarm {
        jcode_app_core::replay::load_swarm_sessions(&session.id, false)
            .unwrap_or_default()
            .into_iter()
            .map(|swarm| swarm.session)
            .filter(|s| s.id != session.id)
            .collect()
    } else {
        Vec::new()
    };
    let related_inputs: Vec<jcode_export_core::RelatedSessionInput> = std::iter::once(
        jcode_export_core::RelatedSessionInput {
            id: session.id.clone(),
            short_name: session.short_name.clone(),
            custom_title: session.custom_title.clone(),
            model: session.model.clone(),
            created_at: Some(
                session
                    .created_at
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            ),
            updated_at: Some(
                session
                    .updated_at
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            ),
            is_primary: true,
        },
    )
    .chain(related_sessions.iter().map(|s| {
        jcode_export_core::RelatedSessionInput {
            id: s.id.clone(),
            short_name: s.short_name.clone(),
            custom_title: s.custom_title.clone(),
            model: s.model.clone(),
            created_at: Some(
                s.created_at
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            ),
            updated_at: Some(
                s.updated_at
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            ),
            is_primary: false,
        }
    }))
    .collect();
    let source_path = crate::session::session_path(&session.id).ok();
    let raw_session_json: Option<serde_json::Value> = source_path
        .as_deref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok());

    let compaction = session.compaction.clone();
    let input = jcode_export_core::SessionExportInput {
        id: &session.id,
        parent_id: session.parent_id.as_deref(),
        title: session.title.as_deref(),
        custom_title: session.custom_title.as_deref(),
        short_name: session.short_name.as_deref(),
        created_at: Some(
            session
                .created_at
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        ),
        updated_at: Some(
            session
                .updated_at
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        ),
        model: session.model.as_deref(),
        provider_key: session.provider_key.as_deref(),
        working_dir: session.working_dir.as_deref(),
        status: &session.status,
        messages: &session.messages,
        compaction: compaction.as_ref(),
        related: Some(&related_inputs),
        raw_session_json: raw_session_json.as_ref(),
    };

    let out_path = match explicit_path {
        Some(path) => std::path::PathBuf::from(path),
        None => jcode_export_core::default_output_path(
            source_path.as_deref().and_then(|p| p.parent()),
            &session.id,
            format,
        ),
    };

    let content = match format {
        jcode_export_core::ExportFormat::Json => jcode_export_core::export_json(&input)?,
        jcode_export_core::ExportFormat::Html => jcode_export_core::export_html(&input)?,
    };
    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out_path, content)?;
    Ok(out_path)
}

impl App {
    /// Bus pump callback: surface the finished export in the transcript.
    pub(super) fn handle_session_export_ready(&mut self, event: SessionExportReady) {
        if event.session_id != self.session.id {
            return;
        }
        match event.result {
            Ok(path) => {
                self.push_display_message(DisplayMessage::system(format!(
                    "✓ Exported session to {}",
                    path.display()
                )));
                self.set_status_notice(format!("Exported: {}", path.display()));
            }
            Err(error) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Export failed: {error}"
                )));
            }
        }
    }
}
