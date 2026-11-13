//! Minimal local MCP stdio adapter for the Sceadu daemon.
//!
//! This is an adapter, not an agent-to-agent protocol: MCP clients call these
//! tools to send and read Sceadu messages.

use std::path::Path;

use anyhow::{anyhow, Result};
use sceadu_core::agent::{AgentMessage, Part, TextPart};
use sceadu_core::daemon::{self, IpcRequest};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const PROTOCOL_VERSION: &str = "2024-11-05";

pub async fn run(socket: &Path) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                write_json(
                    &mut stdout,
                    &error_response(Value::Null, -32700, &error.to_string()),
                )
                .await?;
                continue;
            }
        };
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            continue;
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let params = request.get("params").cloned().unwrap_or(Value::Null);

        let response = match method {
            "initialize" => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "sceadu-mcp", "version": env!("CARGO_PKG_VERSION")}
                }
            })),
            "notifications/initialized" | "notifications/cancelled" => None,
            "ping" => Some(json!({"jsonrpc": "2.0", "id": id, "result": {}})),
            "tools/list" => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"tools": tool_defs()}
            })),
            "tools/call" => Some(match call_tool(socket, &params).await {
                Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                Err(error) => tool_error(id, &error.to_string()),
            }),
            _ => Some(error_response(id, -32601, "method not found")),
        };
        if let Some(response) = response {
            write_json(&mut stdout, &response).await?;
        }
    }
    Ok(())
}

async fn call_tool(socket: &Path, params: &Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("tools/call requires name"))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let request = match name {
        "sceadu_send" => {
            if let Some(text) = args.get("text").and_then(Value::as_str) {
                IpcRequest::SendText {
                    to: args
                        .get("to")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    peer_agent_id: required_str(&args, "peer_agent_id")?.parse()?,
                    text: text.to_owned(),
                    context_id: args
                        .get("context_id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                }
            } else {
                IpcRequest::Send {
                    to: required_str(&args, "to")?,
                    message: serde_json::from_value(
                        args.get("message")
                            .cloned()
                            .ok_or_else(|| anyhow!("message or text is required"))?,
                    )?,
                }
            }
        }
        "sceadu_send_text" => IpcRequest::SendText {
            to: args
                .get("to")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            peer_agent_id: required_str(&args, "peer_agent_id")?.parse()?,
            text: required_str(&args, "text")?,
            context_id: args
                .get("context_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        },
        "sceadu_inbox" => IpcRequest::Inbox {
            limit: args.get("limit").and_then(Value::as_u64).unwrap_or(20) as u32,
        },
        "sceadu_read" => IpcRequest::Read {
            message_id: required_str(&args, "message_id")?,
        },
        "sceadu_reply" => IpcRequest::Reply {
            message_id: required_str(&args, "message_id")?,
            parts: vec![Part::Text(TextPart {
                text: required_str(&args, "text")?,
            })],
            to: args
                .get("to")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        },
        "sceadu_file_send" => IpcRequest::FileSend {
            to: required_str(&args, "to")?,
            path: required_str(&args, "path")?.into(),
        },
        "sceadu_status" => IpcRequest::Status,
        "sceadu_peers" => IpcRequest::Peers,
        "sceadu_biographies" => IpcRequest::Biographies,
        "sceadu_biography" => IpcRequest::Biography {
            agent_id: required_str(&args, "agent_id")?,
        },
        "sceadu_refresh_biographies" => IpcRequest::RefreshBiographies,
        _ => return Err(anyhow!("unknown tool: {name}")),
    };
    let response = daemon::call(socket, &request).await?;
    if !response.ok {
        return Err(anyhow!(response
            .error
            .unwrap_or_else(|| "daemon request failed".to_owned())));
    }
    Ok(json!({
        "content": [{"type": "text", "text": serde_json::to_string_pretty(&response.data)?}],
        "isError": false
    }))
}

fn tool_error(id: Value, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": message}],
            "isError": true
        }
    })
}

fn required_str(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow!("{key} is required"))
}

fn tool_defs() -> Value {
    json!([
        {
            "name": "sceadu_send",
            "description": "Send a text message to a Sceadu peer. Provide the peer multiaddr, the peer AgentID, and the text.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to": {"type": "string", "description": "Peer multiaddr including /p2p/<peer-id>"},
                    "peer_agent_id": {"type": "string", "description": "Target aan: AgentID"},
                    "text": {"type": "string", "description": "Message text"},
                    "context_id": {"type": "string"}
                },
                "required": ["peer_agent_id", "text"]
            }
        },
        {
            "name": "sceadu_send_text",
            "description": "Send a text AgentMessage to a peer AgentID. The daemon resolves the peer endpoint from the local Biography store. Pass peer_agent_id and text; optionally override the multiaddr with to.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to": {"type": "string", "description": "Peer multiaddr including /p2p/<peer-id>"},
                    "peer_agent_id": {"type": "string", "description": "Target aan: AgentID"},
                    "text": {"type": "string"},
                    "context_id": {"type": "string"}
                },
                "required": ["peer_agent_id", "text"]
            }
        },
        {
            "name": "sceadu_inbox",
            "description": "List received Sceadu messages, newest first.",
            "inputSchema": {
                "type": "object",
                "properties": {"limit": {"type": "integer", "minimum": 1, "maximum": 1000}}
            }
        },
        {
            "name": "sceadu_read",
            "description": "Read one message by message_id and mark it read.",
            "inputSchema": {
                "type": "object",
                "properties": {"message_id": {"type": "string"}},
                "required": ["message_id"]
            }
        },
        {
            "name": "sceadu_reply",
            "description": "Reply to a received message with text. Optionally provide the peer address if the sender is no longer connected.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "message_id": {"type": "string"},
                    "text": {"type": "string"},
                    "to": {"type": "string"}
                },
                "required": ["message_id", "text"]
            }
        },
        {
            "name": "sceadu_file_send",
            "description": "Send a file (max 50 MB) to a peer and return its FileRef.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to": {"type": "string"},
                    "path": {"type": "string"}
                },
                "required": ["to", "path"]
            }
        },
        {"name": "sceadu_status", "description": "Return daemon network status.", "inputSchema": {"type": "object"}},
        {"name": "sceadu_peers", "description": "List connected peers.", "inputSchema": {"type": "object"}},
        {
            "name": "sceadu_biographies",
            "description": "Return all verified agent biographies known by this node.",
            "inputSchema": {"type": "object"}
        },
        {
            "name": "sceadu_biography",
            "description": "Return one verified biography by AgentID from the local store.",
            "inputSchema": {
                "type": "object",
                "properties": {"agent_id": {"type": "string"}},
                "required": ["agent_id"]
            }
        },
        {
            "name": "sceadu_refresh_biographies",
            "description": "Ask the configured bootstrap node for biographies and merge them locally. Use when local biographies are insufficient.",
            "inputSchema": {"type": "object"}
        }
    ])
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message}
    })
}

async fn write_json(stdout: &mut tokio::io::Stdout, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    stdout.write_all(&bytes).await?;
    stdout.flush().await?;
    Ok(())
}

// Keep the import used when MCP gains reply/file tools that accept an existing
// AgentMessage directly.
#[allow(dead_code)]
fn _message_type(_: AgentMessage) {}
