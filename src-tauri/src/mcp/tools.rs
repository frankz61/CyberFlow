//! MCP tool definitions. Only the two "one-click" end-to-end flows are
//! exposed — the caller (LLM / MCP client) doesn't need to know about the
//! individual launch / inject / click steps.

use serde_json::{json, Value};

use crate::commands::{mstsc, sangfor};

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

pub fn list_tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "sangfor_login",
            description: "End-to-end Sangfor VDI login on the local Windows host. \
                Launches the Sangfor desktop cloud client if not running, waits for \
                its login dialog, types the username and password via SendInput, then \
                clicks the Login button. Returns after the login request has been \
                submitted (does not wait for the remote desktop session to appear).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "username": {
                        "type": "string",
                        "description": "Sangfor account username."
                    },
                    "password": {
                        "type": "string",
                        "description": "Sangfor account password."
                    }
                },
                "required": ["username", "password"],
                "additionalProperties": false
            }),
        },
        ToolDef {
            name: "mstsc_connect",
            description: "End-to-end Windows Remote Desktop (mstsc) connect on the \
                local host. Launches mstsc if not running, waits for its dialog, \
                types the target IP/hostname into the Computer field, then clicks \
                Connect. Returns after the connect request has been submitted.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "ip": {
                        "type": "string",
                        "description": "IP address or hostname of the target machine."
                    }
                },
                "required": ["ip"],
                "additionalProperties": false
            }),
        },
    ]
}

pub async fn call_tool(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "sangfor_login" => {
            let username = args
                .get("username")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing 'username' argument".to_string())?;
            let password = args
                .get("password")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing 'password' argument".to_string())?;
            sangfor::run_sangfor_full_flow(username, password).await?;
            Ok(text_result("Sangfor login flow completed"))
        }
        "mstsc_connect" => {
            let ip = args
                .get("ip")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing 'ip' argument".to_string())?;
            mstsc::run_mstsc_full_flow(ip).await?;
            Ok(text_result(&format!(
                "mstsc connect flow completed (ip={ip})"
            )))
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

fn text_result(msg: &str) -> Value {
    // Shape per MCP CallToolResult: { content: [{type: "text", text: "..."}] }
    json!({
        "content": [
            { "type": "text", "text": msg }
        ],
        "isError": false
    })
}
