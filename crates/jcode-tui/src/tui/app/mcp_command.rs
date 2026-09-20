//! Fork: `/mcp` slash command - manage MCP servers from the TUI.
//!
//! Fork policy: all logic lives in this dedicated module; the only upstream
//! files touched are the dispatch table (`commands_dispatch.rs`, one call
//! site), the local/remote run loops (one `poll_mcp_command` call each), and
//! the `App` struct (one pending-result field).
//!
//! Usage:
//!   /mcp                     - list configured + connected servers
//!   /mcp reload              - re-read mcp.json and reconnect everything
//!   /mcp connect <name>      - connect a configured (possibly disabled) server
//!   /mcp disconnect <name>   - disconnect a connected server
//!
//! Operations run against the local process's `McpManager`, which the wire
//! client does not own, so SSH/remote sessions block the command like the
//! other laptop-local actions. Results are delivered to the transcript via a
//! pending receiver polled from the run loop (same shape as
//! `PendingLocalTransfer`).

use super::{App, PendingMcpCommand};
use std::sync::mpsc;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Entry point for the `/mcp` slash command. Returns `true` when the input
/// was claimed.
pub(in crate::tui::app) fn handle_mcp_command(app: &mut App, input: &str) -> bool {
    let trimmed = input.trim();
    if trimmed != "/mcp" && !trimmed.starts_with("/mcp ") {
        return false;
    }

    if crate::tui::is_ssh_remote() {
        super::commands_dispatch::ssh_local_action_blocked(app, "MCP management");
        return true;
    }

    let argument = trimmed
        .strip_prefix("/mcp")
        .map(str::trim)
        .unwrap_or_default();
    let parts = argument.split_whitespace().collect::<Vec<_>>();

    // Bare /mcp opens the interactive picker (arrows + Enter); typed
    // subcommands stay available for power users and scripting.
    if parts.is_empty() {
        open_mcp_picker(app);
        return true;
    }

    let action = match parts.as_slice() {
        [] | ["list"] => McpAction::List,
        ["reload"] => McpAction::Reload,
        ["connect", name] => McpAction::Connect((*name).to_string()),
        ["disconnect", name] => McpAction::Disconnect((*name).to_string()),
        [unknown, ..] => {
            app.push_display_message(super::DisplayMessage::system(format!(
                "Unknown /mcp action '{unknown}'. Usage: /mcp [list | reload | connect <name> | disconnect <name>]"
            )));
            return true;
        }
    };

    let manager = Arc::clone(&app.mcp_manager);
    let (tx, rx) = mpsc::channel::<String>();
    app.pending_mcp_command = Some(PendingMcpCommand { receiver: rx });
    app.set_status_notice("MCP command running...".to_string());

    tokio::spawn(async move {
        let report = run_mcp_action(manager, action).await;
        let _ = tx.send(report);
    });
    true
}

enum McpAction {
    List,
    Reload,
    Connect(String),
    Disconnect(String),
}

async fn run_mcp_action(manager: Arc<RwLock<crate::mcp::McpManager>>, action: McpAction) -> String {
    match action {
        McpAction::List => list_servers(&manager).await,
        McpAction::Reload => reload_servers(manager).await,
        McpAction::Connect(name) => connect_server_blocking(&manager, &name).await,
        McpAction::Disconnect(name) => disconnect_server_blocking(&manager, &name).await,
    }
}

async fn list_servers(manager: &Arc<RwLock<crate::mcp::McpManager>>) -> String {
    let manager = manager.read().await;
    let configured = manager.config().servers.clone();
    let connected = manager.connected_servers().await;
    let all_tools = manager.all_tools().await;

    if configured.is_empty() {
        return "No MCP servers configured. Add servers to ~/.jcode/mcp.json \
                (or .jcode/mcp.json in the project), then run /mcp reload."
            .to_string();
    }

    let mut lines = vec!["MCP servers:".to_string()];
    for (name, server) in &configured {
        let state = if connected.contains(&name.to_string()) { "connected" } else { "not connected" };
        let tool_count = all_tools.iter().filter(|(server, _)| server == name).count();
        let kind = if server.url.is_some() { "remote" } else { "stdio" };
        lines.push(format!("  {name} ({kind}, {state}, {tool_count} tools)"));
    }
    lines.join("\n")
}

async fn reload_servers(manager: Arc<RwLock<crate::mcp::McpManager>>) -> String {
    let mut manager = manager.write().await;
    match manager.reload().await {
        Ok((connected_count, failures)) => {
            let total = manager.config().servers.len();
            if failures.is_empty() {
                format!("MCP reload complete: {connected_count}/{total} servers connected.")
            } else {
                let failed: Vec<String> =
                    failures.iter().map(|(name, error)| format!("  {name}: {error}")).collect();
                format!(
                    "MCP reload complete: {connected_count}/{total} servers connected. Failures:\n{}",
                    failed.join("\n")
                )
            }
        }
        Err(error) => format!("MCP reload failed: {error}"),
    }
}

pub(in crate::tui::app) async fn connect_server_blocking(manager: &Arc<RwLock<crate::mcp::McpManager>>, name: &str) -> String {
    let configured = {
        let manager = manager.read().await;
        manager.config().servers.get(name).cloned()
    };
    let Some(config) = configured else {
        return format!(
            "Server '{name}' is not in the MCP config. Add it to ~/.jcode/mcp.json, then /mcp reload."
        );
    };

    let manager = manager.read().await;
    let connected = manager.connected_servers().await;
    if connected.contains(&name.to_string()) {
        return format!("Server '{name}' is already connected. Use '/mcp disconnect {name}' first.");
    }
    match manager.connect(name, &config).await {
        Ok(()) => {
            let tool_count = manager.all_tools().await.iter().filter(|(s, _)| s == name).count();
            format!("Connected to '{name}' ({tool_count} tools).")
        }
        Err(error) => format!("Failed to connect to '{name}': {error}"),
    }
}

/// Toggle by live state at execution time: connect when not connected,
/// disconnect when connected. The picker opens before knowing the state,
/// so the decision happens here. `registry` shares the session's tool map,
/// so newly connected server tools register (and disconnected ones get
/// dropped on the next reload) exactly like the agent-side `mcp` tool.
pub(in crate::tui::app) async fn toggle_server_blocking(
    manager: &Arc<RwLock<crate::mcp::McpManager>>,
    name: &str,
    registry: &crate::tool::Registry,
) -> String {
    let connected = manager.read().await.connected_servers().await;
    let report = if connected.contains(&name.to_string()) {
        disconnect_server_blocking(manager, name).await
    } else {
        connect_server_blocking(manager, name).await
    };

    // Keep the session tool registry in sync with the manager state: register
    // tools for every currently connected server (idempotent per name).
    if report.starts_with("Connected to") {
        let mcp_tools = crate::mcp::create_mcp_tools(Arc::clone(manager)).await;
        let server_prefix = crate::mcp::dispatch_name(name, "");
        for (tool_name, tool) in mcp_tools {
            if tool_name.starts_with(&server_prefix) {
                registry.register(tool_name, tool).await;
            }
        }
    }
    report
}

pub(in crate::tui::app) async fn disconnect_server_blocking(manager: &Arc<RwLock<crate::mcp::McpManager>>, name: &str) -> String {
    let manager = manager.read().await;
    let connected = manager.connected_servers().await;
    if !connected.contains(&name.to_string()) {
        return format!("Server '{name}' is not connected.");
    }
    match manager.disconnect(name).await {
        Ok(()) => format!("Disconnected '{name}'."),
        Err(error) => format!("Failed to disconnect '{name}': {error}"),
    }
}

/// Poll a finished `/mcp` operation and surface its report in the transcript.
/// Called from the local/remote run loops; returns `true` when a report
/// arrived (so the caller redraws).
pub(in crate::tui::app) fn poll_mcp_command(app: &mut App) -> bool {
    let Some(pending) = app.pending_mcp_command.as_ref() else {
        return false;
    };
    match pending.receiver.try_recv() {
        Ok(report) => {
            app.pending_mcp_command = None;
            app.push_display_message(super::DisplayMessage::system(report));
            true
        }
        Err(mpsc::TryRecvError::Empty) => false,
        Err(mpsc::TryRecvError::Disconnected) => {
            app.pending_mcp_command = None;
            app.push_display_message(super::DisplayMessage::system(
                "MCP command failed: background task ended without a report.".to_string(),
            ));
            true
        }
    }
}

/// Open the interactive `/mcp` picker: one row per configured server,
/// Enter toggles connect/disconnect. Built from a fresh config read so
/// new servers appear without a reload.
pub(in crate::tui::app) fn open_mcp_picker(app: &mut App) {
    use crate::tui::{InlineInteractiveState, PickerEntry, PickerKind, PickerOption};

    let config = mcp_config();
    let connected: std::collections::HashSet<String> = {
        match app.mcp_manager.try_read() {
            Ok(manager) => {
                // connected_servers() is async; approximate with the config
                // snapshot's enabled state plus a non-blocking check below.
                let _ = manager;
                std::collections::HashSet::new()
            }
            Err(_) => std::collections::HashSet::new(),
        }
    };
    let _ = connected;

    let mut entries: Vec<PickerEntry> = config
        .servers
        .iter()
        .map(|(name, server)| {
            let kind_label = if server.url.is_some() { "remote" } else { "stdio" };
            PickerEntry {
                name: name.clone(),
                options: vec![PickerOption {
                    provider: format!("{} - Enter to toggle", kind_label),
                    api_method: String::new(),
                    available: true,
                    detail: server
                        .url
                        .clone()
                        .unwrap_or_else(|| server.command.clone()),
                    estimated_reference_cost_micros: None,
                }],
                action: crate::tui::PickerAction::McpServer {
                    name: name.clone(),
                },
                selected_option: 0,
                is_current: false,
                is_default: false,
                is_favorite: false,
                recommended: false,
                recommendation_rank: usize::MAX,
                usage_score: 0,
                old: false,
                created_date: None,
                effort: None,
            }
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));

    app.inline_view_state = None;
    app.inline_interactive_state = Some(InlineInteractiveState {
        kind: PickerKind::Model,
        filtered: (0..entries.len()).collect(),
        entries,
        selected: 0,
        column: 0,
        filter: String::new(),
        preview: false,
    });
    app.input.clear();
    app.cursor_pos = 0;
}

/// Read the merged MCP config synchronously (same resolution as the manager).
fn mcp_config() -> crate::mcp::McpConfig {
    crate::mcp::McpConfig::load_for_dir(
        std::env::current_dir().ok().as_deref(),
    )
}
