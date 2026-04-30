//! Tauri commands for controlling the embedded MCP server.

use std::sync::Arc;
use tauri::{AppHandle, State};

use crate::mcp::{server, McpServerState, ServerInfo};

const DEFAULT_PORT: u16 = 37824;

fn build_info(state: &McpServerState) -> ServerInfo {
    let port = state.port();
    let url = port.map(|p| format!("http://127.0.0.1:{p}/mcp"));
    ServerInfo {
        running: state.is_running(),
        port,
        url,
        token: state.token(),
    }
}

#[tauri::command]
#[specta::specta]
pub async fn start_mcp_server(
    app: AppHandle,
    state: State<'_, Arc<McpServerState>>,
) -> Result<ServerInfo, String> {
    let _ = app;
    if state.is_running() {
        return Ok(build_info(&state));
    }
    let handle = server::start(state.inner().clone(), DEFAULT_PORT).await?;
    state.set_running(handle)?;
    log::info!("[mcp] started (port={:?})", state.port());
    Ok(build_info(&state))
}

#[tauri::command]
#[specta::specta]
pub async fn stop_mcp_server(
    state: State<'_, Arc<McpServerState>>,
) -> Result<ServerInfo, String> {
    if let Some(mut running) = state.take_running() {
        if let Some(tx) = running.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
    Ok(build_info(&state))
}

#[tauri::command]
#[specta::specta]
pub async fn get_mcp_server_status(
    state: State<'_, Arc<McpServerState>>,
) -> Result<ServerInfo, String> {
    Ok(build_info(&state))
}

#[tauri::command]
#[specta::specta]
pub async fn regenerate_mcp_token(
    app: AppHandle,
    state: State<'_, Arc<McpServerState>>,
) -> Result<ServerInfo, String> {
    state.regenerate_token(&app);
    Ok(build_info(&state))
}
