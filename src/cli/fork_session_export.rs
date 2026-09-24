//! Fork: `jcode session export` CLI handler.
//!
//! Fork-only module: all logic lives here so the shared `commands.rs` only
//! carries a two-line hook (`pub use` + doc pointer). See
//! `docs/UPSTREAM_MERGE_GUIDE.md`.

use anyhow::Result;

/// `jcode session export <id|path> [--format html|json] [-o path] [--open]`.
///
/// Resolves the session like `jcode replay` does (ID, short name, or file
/// path), then writes either the full JSON dump or the self-contained HTML
/// viewer from `jcode-export-core`.
pub fn run_session_export_command(
    session_ref: &str,
    format: Option<&str>,
    output: Option<&str>,
    open_after: bool,
    include_swarm: bool,
) -> Result<()> {
    let format = format
        .and_then(jcode_export_core::ExportFormat::parse)
        .unwrap_or(jcode_export_core::ExportFormat::Html);

    // Resolve as a file path first (same contract as replay::load_session).
    let source_path = std::path::Path::new(session_ref);
    let (session, source_file) = if source_path.exists() {
        (
            crate::session::Session::load_from_path(source_path)?,
            Some(source_path.to_path_buf()),
        )
    } else {
        let resolved_id = crate::session::find_session_by_name_or_id(session_ref)?;
        let path = crate::session::session_path(&resolved_id)?;
        (crate::session::Session::load(&resolved_id)?, Some(path))
    };

    // Fork: optionally pull in related subagent sessions (same matcher as
    // `jcode replay --swarm`: parent/child links, same working dir, ±6h).
    let related_sessions: Vec<crate::session::Session> = if include_swarm {
        crate::replay::load_swarm_sessions(&session.id, false)
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
            message_count: session.messages.len(),
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
            message_count: s.messages.len(),
        }
    }))
    .collect();

    // Raw stored session JSON: re-read the file so the export preserves the
    // exact storage shape (skip-cached fields like persist state never hit
    // disk anyway).
    let raw_session_json: Option<serde_json::Value> = source_file
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

    let out_path = match output.map(std::path::PathBuf::from) {
        Some(path) => path,
        None => jcode_export_core::default_output_path(
            source_file.as_deref().and_then(|p| p.parent()),
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

    println!("Exported to: {}", out_path.display());

    if open_after {
        open::that_detached(&out_path)?;
    }

    Ok(())
}
