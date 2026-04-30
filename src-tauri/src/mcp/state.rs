//! Runtime state for the MCP server: running handle + auth token.
//!
//! Token is persisted to `<app_data>/mcp_token.txt` so it survives app
//! restarts. Regenerating rotates it (any active MCP clients will need to
//! re-copy the new token).

use rand::Rng;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use tokio::sync::oneshot;

pub struct RunningServer {
    pub port: u16,
    pub shutdown_tx: Option<oneshot::Sender<()>>,
}

pub struct McpServerState {
    inner: Mutex<Inner>,
}

struct Inner {
    running: Option<RunningServer>,
    token: String,
}

impl McpServerState {
    pub fn new(app: &AppHandle) -> Self {
        let token = load_or_generate_token(app);
        Self {
            inner: Mutex::new(Inner {
                running: None,
                token,
            }),
        }
    }

    pub fn token(&self) -> String {
        self.inner.lock().expect("mcp state poisoned").token.clone()
    }

    pub fn regenerate_token(&self, app: &AppHandle) -> String {
        let new_token = generate_token();
        let mut guard = self.inner.lock().expect("mcp state poisoned");
        guard.token = new_token.clone();
        drop(guard);
        if let Err(e) = persist_token(app, &new_token) {
            log::warn!("[mcp] failed to persist regenerated token: {e}");
        }
        new_token
    }

    pub fn is_running(&self) -> bool {
        self.inner
            .lock()
            .expect("mcp state poisoned")
            .running
            .is_some()
    }

    pub fn port(&self) -> Option<u16> {
        self.inner
            .lock()
            .expect("mcp state poisoned")
            .running
            .as_ref()
            .map(|r| r.port)
    }

    /// Install a running-server handle. Returns an error if one is already
    /// active — callers should stop first.
    pub fn set_running(&self, running: RunningServer) -> Result<(), String> {
        let mut guard = self.inner.lock().expect("mcp state poisoned");
        if guard.running.is_some() {
            return Err("MCP server is already running".into());
        }
        guard.running = Some(running);
        Ok(())
    }

    pub fn take_running(&self) -> Option<RunningServer> {
        self.inner
            .lock()
            .expect("mcp state poisoned")
            .running
            .take()
    }
}

fn token_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create_dir_all: {e}"))?;
    Ok(dir.join("mcp_token.txt"))
}

fn load_or_generate_token(app: &AppHandle) -> String {
    match token_path(app) {
        Ok(path) if path.exists() => match std::fs::read_to_string(&path) {
            Ok(s) => {
                let trimmed = s.trim().to_string();
                if trimmed.is_empty() {
                    let t = generate_token();
                    let _ = persist_token(app, &t);
                    t
                } else {
                    trimmed
                }
            }
            Err(e) => {
                log::warn!("[mcp] failed to read token, regenerating: {e}");
                let t = generate_token();
                let _ = persist_token(app, &t);
                t
            }
        },
        _ => {
            let t = generate_token();
            let _ = persist_token(app, &t);
            t
        }
    }
}

fn persist_token(app: &AppHandle, token: &str) -> Result<(), String> {
    let path = token_path(app)?;
    std::fs::write(&path, token).map_err(|e| format!("write token: {e}"))?;
    Ok(())
}

fn generate_token() -> String {
    // 32 hex chars = 128 bits of entropy. Plenty for a localhost-only token.
    const CHARSET: &[u8] = b"0123456789abcdef";
    let mut rng = rand::thread_rng();
    (0..32)
        .map(|_| {
            let i = rng.gen_range(0..CHARSET.len());
            CHARSET[i] as char
        })
        .collect()
}
