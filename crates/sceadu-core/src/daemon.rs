//! Foreground Sceadu daemon and its local Unix-socket IPC.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use libp2p::multiaddr::Protocol;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::agent::{new_context_id, new_message_id, AgentMessage, Part, Role, TextPart};
use crate::node::{spawn_with_config, Multiaddr, NodeConfig, NodeHandle, PeerId};
use crate::{AgentId, Error, Identity, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum IpcRequest {
    Send {
        to: String,
        message: AgentMessage,
    },
    SendText {
        to: String,
        peer_agent_id: AgentId,
        text: String,
        #[serde(default)]
        context_id: Option<String>,
    },
    Inbox {
        #[serde(default = "default_limit")]
        limit: u32,
    },
    Read {
        message_id: String,
    },
    Reply {
        message_id: String,
        parts: Vec<Part>,
        #[serde(default)]
        to: Option<String>,
    },
    Status,
    Lookup {
        agent_id: String,
    },
    Discover {
        introducers: Vec<String>,
        #[serde(default)]
        skill: Option<String>,
        #[serde(default = "default_limit")]
        max: u32,
    },
    Publish,
    Peers,
    FileSend {
        to: String,
        path: std::path::PathBuf,
    },
}

fn default_limit() -> u32 {
    20
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    pub ok: bool,
    #[serde(default)]
    pub data: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl IpcResponse {
    pub fn ok(data: Value) -> Self {
        Self {
            ok: true,
            data,
            error: None,
        }
    }

    pub fn error(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: Value::Null,
            error: Some(error.into()),
        }
    }
}

/// Run a foreground daemon until Ctrl-C or SIGTERM.
pub async fn run(
    identity: Identity,
    listen: Option<Multiaddr>,
    config: NodeConfig,
    socket_path: PathBuf,
) -> Result<()> {
    prepare_socket(&socket_path)?;
    let (node, address) = spawn_with_config(&identity, listen, config).await?;
    let node = Arc::new(node);
    let listener = UnixListener::bind(&socket_path)
        .map_err(|error| Error::Unsupported(format!("bind daemon socket: {error}")))?;
    set_socket_permissions(&socket_path)?;

    println!(
        "{}",
        json!({
            "event": "ready",
            "agent_id": node.agent_id().to_string(),
            "peer_id": node.peer_id().to_string(),
            "listen": address.as_ref().map(ToString::to_string),
            "socket": socket_path.display().to_string(),
        })
    );

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let node = node.clone();
                        tokio::spawn(async move {
                            if let Err(error) = handle_connection(stream, node).await {
                                eprintln!("sceadu daemon: ipc error: {error}");
                            }
                        });
                    }
                    Err(error) => eprintln!("sceadu daemon: accept error: {error}"),
                }
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}

async fn handle_connection(stream: UnixStream, node: Arc<NodeHandle>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| Error::Unsupported(format!("ipc read: {error}")))?
    else {
        return Ok(());
    };
    let response = match serde_json::from_str::<IpcRequest>(&line) {
        Ok(request) => dispatch(request, node).await,
        Err(error) => IpcResponse::error(format!("invalid request: {error}")),
    };
    let mut encoded = serde_json::to_vec(&response)
        .map_err(|error| Error::Unsupported(format!("ipc encode: {error}")))?;
    encoded.push(b'\n');
    writer
        .write_all(&encoded)
        .await
        .map_err(|error| Error::Unsupported(format!("ipc write: {error}")))?;
    Ok(())
}

async fn dispatch(request: IpcRequest, node: Arc<NodeHandle>) -> IpcResponse {
    let result = match request {
        IpcRequest::Send { to, mut message } => {
            message.from = node.agent_id().to_string();
            if let Err(error) = message.validate() {
                return IpcResponse::error(error.to_string());
            }
            send_to(&node, &to, message)
                .await
                .map(|_| json!({"status": "delivered"}))
        }
        IpcRequest::SendText {
            to,
            peer_agent_id,
            text,
            context_id,
        } => {
            let message = AgentMessage {
                protocol: crate::agent::MESSAGE_PROTOCOL.to_owned(),
                message_id: new_message_id(),
                context_id: context_id.unwrap_or_else(new_context_id),
                in_reply_to: None,
                from: node.agent_id().to_string(),
                to: peer_agent_id.to_string(),
                role: Role::User,
                parts: vec![Part::Text(TextPart { text })],
                created_at: now_unix(),
            };
            if let Err(error) = message.validate() {
                return IpcResponse::error(error.to_string());
            }
            send_to(&node, &to, message)
                .await
                .map(|_| json!({"status": "delivered"}))
        }
        IpcRequest::Inbox { limit } => node
            .inbox()
            .list(limit)
            .map(|messages| json!(messages))
            .map_err(Error::Agent),
        IpcRequest::Read { message_id } => {
            let inbox = node.inbox();
            match inbox.read(&message_id) {
                Ok(Some(stored)) => {
                    let _ = inbox.mark_read(&message_id);
                    Ok(json!(stored))
                }
                Ok(None) => Err(Error::Unsupported("message not found".to_owned())),
                Err(error) => Err(Error::Agent(error)),
            }
        }
        IpcRequest::Reply {
            message_id,
            parts,
            to,
        } => reply_to(&node, &message_id, parts, to.as_deref())
            .await
            .map(|_| json!({"status": "delivered"})),
        IpcRequest::Status => node.network_status().await.map(|status| json!(status)),
        IpcRequest::Lookup { agent_id } => {
            let Ok(agent_id) = agent_id.parse::<AgentId>() else {
                return IpcResponse::error("invalid agent id");
            };
            node.lookup(agent_id).await.map(|manifest| json!(manifest))
        }
        IpcRequest::Discover {
            introducers,
            skill,
            max,
        } => discover(&node, &introducers, skill, max)
            .await
            .map(|records| json!(records)),
        IpcRequest::Publish => node.publish().await.map(|_| json!({"status": "published"})),
        IpcRequest::FileSend { to, path } => {
            file_to(&node, &to, &path).await.map(|file| json!(file))
        }
        IpcRequest::Peers => node.peers().await.map(
            |peers| json!({"peers": peers.iter().map(ToString::to_string).collect::<Vec<_>>()}),
        ),
    };
    match result {
        Ok(data) => IpcResponse::ok(data),
        Err(error) => IpcResponse::error(error.to_string()),
    }
}

async fn send_to(node: &NodeHandle, to: &str, message: AgentMessage) -> Result<()> {
    let address = to
        .parse::<Multiaddr>()
        .map_err(|error| Error::Dial(format!("invalid address: {error}")))?;
    let peer = peer_from_multiaddr(&address)
        .ok_or_else(|| Error::Dial("multiaddr must include /p2p/<peer-id>".to_owned()))?;
    node.dial(address).await?;
    node.send_message(peer, message).await
}

async fn reply_to(
    node: &NodeHandle,
    message_id: &str,
    parts: Vec<Part>,
    to: Option<&str>,
) -> Result<()> {
    let original = node
        .inbox()
        .read(message_id)
        .map_err(Error::Agent)?
        .ok_or_else(|| Error::Unsupported("message not found".to_owned()))?;
    let reply = AgentMessage {
        protocol: crate::agent::MESSAGE_PROTOCOL.to_owned(),
        message_id: new_message_id(),
        context_id: original.context_id.clone(),
        in_reply_to: Some(message_id.to_owned()),
        from: node.agent_id().to_string(),
        to: original.from.clone(),
        role: Role::Agent,
        parts,
        created_at: now_unix(),
    };
    let peer = if let Some(address) = to {
        let address = address
            .parse::<Multiaddr>()
            .map_err(|error| Error::Dial(format!("invalid address: {error}")))?;
        let peer = peer_from_multiaddr(&address)
            .ok_or_else(|| Error::Dial("multiaddr must include /p2p/<peer-id>".to_owned()))?;
        node.dial(address).await?;
        peer
    } else {
        let address = original
            .from
            .parse::<AgentId>()
            .map_err(|_| Error::Unsupported("invalid sender".to_owned()))?;
        peer_id_for_agent_id(&address)?
    };
    node.send_message(peer, reply).await
}

async fn file_to(
    node: &NodeHandle,
    to: &str,
    path: &std::path::Path,
) -> Result<crate::agent::FileRef> {
    if path.is_dir() {
        return Err(Error::Unsupported("file path is a directory".to_owned()));
    }
    let address = to
        .parse::<Multiaddr>()
        .map_err(|error| Error::Dial(format!("invalid address: {error}")))?;
    let peer = peer_from_multiaddr(&address)
        .ok_or_else(|| Error::Dial("multiaddr must include /p2p/<peer-id>".to_owned()))?;
    node.dial(address).await?;
    node.send_file(peer, path).await
}

async fn discover(
    node: &NodeHandle,
    introducers: &[String],
    skill: Option<String>,
    max: u32,
) -> Result<Vec<crate::identity::ContactRecord>> {
    let mut peers = Vec::new();
    for introducer in introducers {
        let address = introducer
            .parse::<Multiaddr>()
            .map_err(|error| Error::Dial(format!("invalid introducer: {error}")))?;
        let peer = peer_from_multiaddr(&address)
            .ok_or_else(|| Error::Dial("introducer needs /p2p/<peer-id>".to_owned()))?;
        node.dial(address).await?;
        peers.push(peer);
    }
    node.discover(&peers, skill, max, max).await
}

/// Send one IPC request and read one response.
pub async fn call(socket_path: &Path, request: &IpcRequest) -> Result<IpcResponse> {
    let stream = UnixStream::connect(socket_path)
        .await
        .map_err(|error| Error::Unsupported(format!("daemon is not running: {error}")))?;
    let (reader, mut writer) = stream.into_split();
    let mut encoded = serde_json::to_vec(request)
        .map_err(|error| Error::Unsupported(format!("ipc encode: {error}")))?;
    encoded.push(b'\n');
    writer
        .write_all(&encoded)
        .await
        .map_err(|error| Error::Unsupported(format!("ipc write: {error}")))?;
    let mut lines = BufReader::new(reader).lines();
    let line = lines
        .next_line()
        .await
        .map_err(|error| Error::Unsupported(format!("ipc read: {error}")))?
        .ok_or_else(|| Error::Unsupported("daemon closed the connection".to_owned()))?;
    serde_json::from_str(&line)
        .map_err(|error| Error::Unsupported(format!("ipc response: {error}")))
}

fn prepare_socket(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| Error::Unsupported(format!("socket dir: {error}")))?;
    }
    if path.exists() {
        std::fs::remove_file(path)
            .map_err(|error| Error::Unsupported(format!("remove stale socket: {error}")))?;
    }
    Ok(())
}

fn set_socket_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| Error::Unsupported(format!("socket permissions: {error}")))?;
    }
    Ok(())
}

fn peer_from_multiaddr(address: &Multiaddr) -> Option<PeerId> {
    address
        .iter()
        .filter_map(|protocol| match protocol {
            Protocol::P2p(peer) => Some(peer),
            _ => None,
        })
        .last()
}

fn peer_id_for_agent_id(agent_id: &AgentId) -> Result<PeerId> {
    let public = libp2p::identity::ed25519::PublicKey::try_from_bytes(agent_id.as_bytes())
        .map_err(|error| Error::IdentityMessage(error.to_string()))?;
    Ok(PeerId::from_public_key(&libp2p::identity::PublicKey::from(
        public,
    )))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
