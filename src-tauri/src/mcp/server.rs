//! MCP server over HTTP + JSON-RPC 2.0 (Streamable HTTP transport).
//!
//! Single endpoint: `POST /mcp`. Accepts a JSON-RPC request (initialize /
//! ping / tools/list / tools/call / notifications/initialized), returns a
//! JSON-RPC response. Notifications (id absent) return 204 No Content.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use specta::Type;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use super::state::{McpServerState, RunningServer};
use super::tools;

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "cyberflow";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub running: bool,
    pub port: Option<u16>,
    pub url: Option<String>,
    pub token: String,
}

#[derive(Clone)]
struct AppCtx {
    token: Arc<String>,
}

pub async fn start(
    state: Arc<McpServerState>,
    requested_port: u16,
) -> Result<RunningServer, String> {
    let token = state.token();
    let ctx = AppCtx {
        token: Arc::new(token.clone()),
    };

    let addr = format!("127.0.0.1:{requested_port}");
    let listener = TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("bind {addr}: {e}"))?;
    let bound_port = listener
        .local_addr()
        .map_err(|e| format!("local_addr: {e}"))?
        .port();

    let app = Router::new()
        .route("/mcp", post(handle_jsonrpc))
        .with_state(ctx);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        log::info!("[mcp] server listening on 127.0.0.1:{bound_port}");
        let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
            log::info!("[mcp] shutdown signal received");
        });
        if let Err(e) = serve.await {
            log::error!("[mcp] server error: {e}");
        }
        log::info!("[mcp] server stopped");
    });

    Ok(RunningServer {
        port: bound_port,
        shutdown_tx: Some(shutdown_tx),
    })
}

// ---- JSON-RPC types -----------------------------------------------------

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

// Standard JSON-RPC error codes.
const PARSE_ERROR: i32 = -32700;
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INTERNAL_ERROR: i32 = -32603;

// ---- handler ------------------------------------------------------------

async fn handle_jsonrpc(
    State(ctx): State<AppCtx>,
    headers: HeaderMap,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Response {
    // Auth first.
    if let Err(resp) = check_auth(&headers, &ctx.token) {
        return resp;
    }

    let body = match body {
        Ok(Json(v)) => v,
        Err(_) => {
            return error_response(
                Value::Null,
                PARSE_ERROR,
                "invalid JSON body".into(),
            );
        }
    };

    let req: JsonRpcRequest = match serde_json::from_value(body) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                Value::Null,
                INVALID_REQUEST,
                format!("invalid JSON-RPC request: {e}"),
            );
        }
    };

    let id = req.id.clone().unwrap_or(Value::Null);
    let is_notification = req.id.is_none();

    let outcome = dispatch(&req.method, req.params).await;

    if is_notification {
        // JSON-RPC 2.0: notifications get no response body.
        return StatusCode::NO_CONTENT.into_response();
    }

    match outcome {
        Ok(result) => success_response(id, result),
        Err((code, message)) => error_response(id, code, message),
    }
}

fn check_auth(headers: &HeaderMap, expected: &str) -> Result<(), Response> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let provided = auth.strip_prefix("Bearer ").unwrap_or("");
    if provided.is_empty() || provided != expected {
        let body = json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": {
                "code": -32001,
                "message": "unauthorized: missing or invalid bearer token"
            }
        });
        return Err((StatusCode::UNAUTHORIZED, Json(body)).into_response());
    }
    Ok(())
}

async fn dispatch(method: &str, params: Value) -> Result<Value, (i32, String)> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION
            }
        })),

        // Notification — client says "I finished initializing". No response needed
        // but we still accept it.
        "notifications/initialized" => Ok(Value::Null),

        "ping" => Ok(json!({})),

        "tools/list" => {
            let tools: Vec<Value> = tools::list_tools()
                .into_iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": t.input_schema
                    })
                })
                .collect();
            Ok(json!({ "tools": tools }))
        }

        "tools/call" => {
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| (INVALID_REQUEST, "missing 'name'".into()))?
                .to_string();
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            log::info!("[mcp] tools/call name={name}");
            match tools::call_tool(&name, &args).await {
                Ok(result) => Ok(result),
                Err(e) => {
                    // Per MCP, tool execution failures are reported as a
                    // successful call with isError=true content, NOT a
                    // JSON-RPC error. That lets clients surface the
                    // failure to the LLM as a tool result.
                    Ok(json!({
                        "content": [
                            { "type": "text", "text": e }
                        ],
                        "isError": true
                    }))
                }
            }
        }

        other => Err((METHOD_NOT_FOUND, format!("method not found: {other}"))),
    }
}

fn success_response(id: Value, result: Value) -> Response {
    let body = JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    };
    (StatusCode::OK, Json(body)).into_response()
}

fn error_response(id: Value, code: i32, message: String) -> Response {
    let body = JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message,
            data: None,
        }),
    };
    let status = match code {
        PARSE_ERROR | INVALID_REQUEST => StatusCode::BAD_REQUEST,
        METHOD_NOT_FOUND => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let _ = INTERNAL_ERROR; // keep the const live even if unused upstream
    (status, Json(body)).into_response()
}
