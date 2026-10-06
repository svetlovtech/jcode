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
        let result =
            export_session_to_file(&session, format, explicit_path.as_deref(), include_swarm)
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
///
/// Fork fix: in remote mode the TUI's in-memory `app.session.messages` stays
/// empty — `ServerEvent::History` only fills the display transcript
/// (DisplayMessage), never the stored messages. That made `/export` write a
/// viewer with 0 entries ("This session has no exportable messages").
/// Reload the session from disk when the in-memory copy has no messages; the
/// stored file carries the full transcript with timestamps and durations.
fn export_session_to_file(
    session: &crate::session::Session,
    format: jcode_export_core::ExportFormat,
    explicit_path: Option<&str>,
    include_swarm: bool,
) -> anyhow::Result<std::path::PathBuf> {
    let session = if session.messages.is_empty() {
        match crate::session::Session::load(&session.id) {
            Ok(stored) if !stored.messages.is_empty() => {
                crate::logging::info(&format!(
                    "export: in-memory session had 0 messages; loaded {} from disk",
                    stored.messages.len()
                ));
                stored
            }
            Ok(_) => {
                crate::logging::info(
                    "export: session file on disk also has 0 messages; exporting as-is",
                );
                session.clone()
            }
            Err(error) => {
                crate::logging::warn(&format!(
                    "export: could not load session {} from disk ({}); exporting in-memory copy",
                    session.id, error
                ));
                session.clone()
            }
        }
    } else {
        session.clone()
    };
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
    let related_inputs: Vec<jcode_export_core::RelatedSessionInput> =
        std::iter::once(jcode_export_core::RelatedSessionInput {
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
            message_count: session.messages.len(),
        })
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
                message_count: s.messages.len(),
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
                self.push_display_message(DisplayMessage::error(format!("Export failed: {error}")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hermetic JCODE_HOME so Session::save/load hit a temp dir. Mirrors the
    /// app-tests helper, scoped locally because fork_export is its own module.
    fn with_temp_jcode_home<T>(f: impl FnOnce() -> T) -> T {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        struct RestoreEnv(Option<std::ffi::OsString>);
        impl Drop for RestoreEnv {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(value) => crate::env::set_var("JCODE_HOME", value),
                    None => crate::env::remove_var("JCODE_HOME"),
                }
            }
        }
        let _env_guard = RestoreEnv(std::env::var_os("JCODE_HOME"));
        crate::env::set_var("JCODE_HOME", temp.path());
        crate::config::invalidate_config_cache();
        let result = f();
        crate::config::invalidate_config_cache();
        result
    }

    /// Fork regression: in remote mode `app.session.messages` stays empty
    /// (History fills only DisplayMessages), so `/export` used to write a
    /// viewer with 0 entries. The disk fallback in `export_session_to_file`
    /// must recover the stored transcript when the in-memory copy is empty.
    #[test]
    fn export_falls_back_to_disk_when_in_memory_messages_are_empty() {
        with_temp_jcode_home(|| {
            let mut stored = crate::session::Session::create(None, None);
            stored.add_message(
                crate::message::Role::User,
                vec![crate::message::ContentBlock::Text {
                    text: "regression prompt".into(),
                    cache_control: None,
                }],
            );
            stored.add_message(
                crate::message::Role::Assistant,
                vec![crate::message::ContentBlock::Text {
                    text: "regression answer".into(),
                    cache_control: None,
                }],
            );
            stored.save().expect("save stored session");

            // Simulate the remote-mode App view: same id/metadata, 0 messages.
            let mut in_memory =
                crate::session::Session::load(&stored.id).expect("reload stored session");
            in_memory.messages.clear();
            assert!(in_memory.messages.is_empty(), "fixture must be empty");

            let out = super::export_session_to_file(
                &in_memory,
                jcode_export_core::ExportFormat::Json,
                None,
                false,
            )
            .expect("export with disk fallback");
            let text = std::fs::read_to_string(&out).expect("read export");
            assert!(
                text.contains("regression prompt"),
                "export must contain the stored transcript, got {} bytes",
                text.len()
            );
            let value: serde_json::Value = serde_json::from_str(&text).expect("valid json export");
            let entries = value["viewer"]["entries"].as_array();
            assert_eq!(
                entries.map(Vec::len),
                Some(2),
                "user + assistant entry must survive the empty in-memory copy"
            );
        });
    }
}
