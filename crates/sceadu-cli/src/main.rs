use std::fs;
use std::net::SocketAddrV6;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use rpassword::prompt_password;
use sceadu_core::agent::{AgentMessage, Part, TextPart};
use sceadu_core::daemon::{self, IpcRequest, IpcResponse};
use sceadu_core::keystore;
use sceadu_core::{
    ensure_ipv6_listen_addr, ensure_ipv6_peer_addr, AgentId, Biography, ConnectivityPolicy,
    Identity, InviteTicket, Multiaddr, NodeConfig, SessionConfig, TransportConfig,
    DEFAULT_BOOTSTRAP,
};
use serde::Serialize;
use serde_json::json;
mod mcp;

use zeroize::Zeroizing;

#[derive(Debug, Parser)]
#[command(name = "sceadu", version, about = "Sceadu agent message network CLI")]
struct Cli {
    /// Path to the identity keystore.
    #[arg(long, global = true)]
    path: Option<PathBuf>,

    /// Read the passphrase from a file (tests and automation only).
    #[arg(long, global = true)]
    passphrase_file: Option<PathBuf>,

    /// Unix socket of the running daemon.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create an encrypted identity keystore.
    Init,
    /// Print the AgentID from an existing keystore.
    Id,
    /// Build and print a signed biography.
    Biography {
        /// Human-readable description.
        #[arg(long, default_value = "Sceadu agent")]
        description: String,
        /// Capability/region tag. Repeat.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Endpoint multiaddr. Repeat.
        #[arg(long = "endpoint")]
        endpoint: Vec<String>,
    },
    /// Run the foreground daemon that owns the node and inbox.
    Daemon {
        /// Listen multiaddr.
        #[arg(long, default_value = "/ip6/::1/tcp/0")]
        listen: Multiaddr,
        /// Connectivity policy.
        #[arg(long, default_value = "auto")]
        connectivity: String,
        /// Relay peer to reserve a circuit with.
        #[arg(long)]
        relay: Option<Multiaddr>,
        /// Act as a circuit relay v2 server.
        #[arg(long)]
        relay_server: bool,
        /// SOCKS5 proxy for Tor onion dialing.
        #[arg(long)]
        tor_proxy: Option<SocketAddrV6>,
        /// External onion endpoint to advertise.
        #[arg(long)]
        onion: Option<Multiaddr>,
        /// AgentID allowed to deliver messages. Repeat.
        #[arg(long = "allow-agent")]
        allow_agents: Vec<AgentId>,
        /// Bootstrap peer multiaddr. Repeat; overrides SCEADU_BOOTSTRAP/default.
        #[arg(long = "bootstrap")]
        bootstrap: Vec<Multiaddr>,
        /// Biography description advertised to peers.
        #[arg(long, default_value = "Sceadu agent")]
        description: String,
        /// Biography tag. Repeat.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// SQLite inbox path.
        #[arg(long)]
        inbox: Option<PathBuf>,
        /// SQLite biography store path.
        #[arg(long)]
        biography_store: Option<PathBuf>,
        /// Idle connection timeout in seconds (default 3600).
        #[arg(long, default_value_t = 3600)]
        idle_timeout: u64,
    },
    /// Send one AgentMessage JSON file to a peer address.
    Send {
        /// Peer multiaddr, including /p2p/<peer-id>.
        #[arg(long)]
        to: Multiaddr,
        /// AgentMessage JSON file.
        #[arg(long)]
        message: PathBuf,
    },
    /// List received messages.
    Inbox {
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Read and mark one message as read.
    Read { message_id: String },
    /// Reply to a message with text.
    Reply {
        message_id: String,
        #[arg(long)]
        text: String,
        /// Peer address to dial if the sender is no longer connected.
        #[arg(long)]
        to: Option<Multiaddr>,
    },
    /// Send a file to a peer and print its FileRef.
    File {
        #[command(subcommand)]
        command: FileCommand,
    },
    /// List connected peers.
    Peers,
    /// Print daemon network status.
    Status,
    /// List verified biographies known by the local daemon.
    Biographies,
    /// Print one biography from the local daemon.
    BiographyGet { agent_id: AgentId },
    /// Ask the bootstrap peer for biographies and merge them locally.
    RefreshBiographies,
    /// Run the local MCP stdio adapter.
    Mcp,
    /// Print the installed skill path.
    SkillPath {
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Write SKILL.md under the skill directory.
    SkillInstall {
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
    /// Create an invite ticket.
    Invite {
        #[arg(long)]
        peer: Multiaddr,
        #[arg(long, default_value_t = 3600)]
        ttl: u64,
        #[arg(long)]
        out: PathBuf,
    },
    /// Verify an invite ticket and print its seed.
    Join { ticket_file: PathBuf },
    /// Verify a signed biography.
    Verify { file: PathBuf },
    /// Manage local agent skills.
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
}

#[derive(Debug, Subcommand)]
enum FileCommand {
    /// Send a local file over the Sceadu file stream.
    Send {
        #[arg(long)]
        to: Multiaddr,
        path: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum SkillCommand {
    Install {
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
    Path {
        #[arg(long)]
        dir: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init => init(&cli.path, &cli.passphrase_file)?,
        Command::Id => {
            let identity = load_identity(&cli.path, &cli.passphrase_file)?;
            print_json(&json!({"agent_id": identity.agent_id().to_string()}))?;
        }
        Command::Biography {
            description,
            tags,
            endpoint,
        } => {
            let identity = load_identity(&cli.path, &cli.passphrase_file)?;
            let biography = build_biography(&identity, description, tags, endpoint)?;
            print_json(&biography)?;
        }
        Command::Daemon {
            listen,
            connectivity,
            relay,
            relay_server,
            tor_proxy,
            onion,
            allow_agents,
            bootstrap,
            description,
            tags,
            inbox,
            biography_store,
            idle_timeout,
        } => {
            let identity = load_identity(&cli.path, &cli.passphrase_file)?;
            ensure_ipv6_listen_addr(&listen)?;
            if allow_agents.is_empty() {
                bail!("at least one --allow-agent is required for the closed demo admission mode");
            }
            let inbox_path = inbox.unwrap_or(default_inbox_path()?);
            let file_store_path = inbox_path
                .parent()
                .map(|parent| parent.join("files"))
                .unwrap_or_else(|| PathBuf::from(".sceadu/files"));
            let config = NodeConfig {
                transport: TransportConfig {
                    connectivity_policy: parse_connectivity_policy(&connectivity)?,
                    relay,
                    relay_server,
                    tor_proxy,
                    onion,
                },
                session: SessionConfig {
                    idle_connection_timeout: Duration::from_secs(idle_timeout.max(1)),
                    dial_timeout: Duration::from_secs(5),
                },
                agent: sceadu_core::AgentConfig {
                    description,
                    tags,
                    ..Default::default()
                },
                allowed_agents: allow_agents,
                inbox_path: Some(inbox_path.clone()),
                file_store_path: Some(file_store_path),
                biography_store_path: biography_store.or_else(|| {
                    inbox_path
                        .parent()
                        .map(|parent| parent.join("biographies.sqlite"))
                }),
                bootstrap: bootstrap_peers(bootstrap)?,
            };
            daemon::run(identity, Some(listen), config, socket_path(&cli.socket)?).await?;
        }
        Command::Send { to, message } => {
            let message = read_message(&message)?;
            let response = call(
                &cli.socket,
                IpcRequest::Send {
                    to: to.to_string(),
                    message,
                },
            )
            .await?;
            print_response(response)?;
        }
        Command::Inbox { limit } => {
            let response = call(&cli.socket, IpcRequest::Inbox { limit }).await?;
            print_response(response)?;
        }
        Command::Read { message_id } => {
            let response = call(&cli.socket, IpcRequest::Read { message_id }).await?;
            print_response(response)?;
        }
        Command::Reply {
            message_id,
            text,
            to,
        } => {
            let parts = vec![Part::Text(TextPart { text })];
            let response = call(
                &cli.socket,
                IpcRequest::Reply {
                    message_id,
                    parts,
                    to: to.map(|address| address.to_string()),
                },
            )
            .await?;
            print_response(response)?;
        }
        Command::File { command } => match command {
            FileCommand::Send { to, path } => {
                let response = call(
                    &cli.socket,
                    IpcRequest::FileSend {
                        to: to.to_string(),
                        path,
                    },
                )
                .await?;
                print_response(response)?;
            }
        },
        Command::Peers => {
            let response = call(&cli.socket, IpcRequest::Peers).await?;
            print_response(response)?;
        }
        Command::Status => {
            let response = call(&cli.socket, IpcRequest::Status).await?;
            print_response(response)?;
        }
        Command::Biographies => {
            let response = call(&cli.socket, IpcRequest::Biographies).await?;
            print_response(response)?;
        }
        Command::BiographyGet { agent_id } => {
            let response = call(
                &cli.socket,
                IpcRequest::Biography {
                    agent_id: agent_id.to_string(),
                },
            )
            .await?;
            print_response(response)?;
        }
        Command::RefreshBiographies => {
            let response = call(&cli.socket, IpcRequest::RefreshBiographies).await?;
            print_response(response)?;
        }
        Command::Mcp => {
            mcp::run(&socket_path(&cli.socket)?).await?;
        }
        Command::SkillPath { dir } => {
            let dir = skill_dir(dir)?;
            println!("{}", dir.join("SKILL.md").display());
        }
        Command::SkillInstall { dir, force } => {
            let dir = skill_dir(dir)?;
            fs::create_dir_all(&dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
            let path = dir.join("SKILL.md");
            if path.exists() && !force {
                bail!(
                    "{} already exists; pass --force to overwrite",
                    path.display()
                );
            }
            fs::write(&path, include_str!("../../../skills/sceadu/SKILL.md"))
                .with_context(|| format!("failed to write {}", path.display()))?;
            print_json(&json!({"path": path.display().to_string()}))?;
        }
        Command::Skill { command } => match command {
            SkillCommand::Install { dir, force } => {
                let dir = skill_dir(dir)?;
                fs::create_dir_all(&dir)
                    .with_context(|| format!("failed to create {}", dir.display()))?;
                let path = dir.join("SKILL.md");
                if path.exists() && !force {
                    bail!(
                        "{} already exists; pass --force to overwrite",
                        path.display()
                    );
                }
                fs::write(&path, include_str!("../../../skills/sceadu/SKILL.md"))
                    .with_context(|| format!("failed to write {}", path.display()))?;
                print_json(&json!({"path": path.display().to_string()}))?;
            }
            SkillCommand::Path { dir } => {
                let dir = skill_dir(dir)?;
                println!("{}", dir.join("SKILL.md").display());
            }
        },
        Command::Invite { peer, ttl, out } => {
            let identity = load_identity(&cli.path, &cli.passphrase_file)?;
            let ticket = InviteTicket::issue(&identity, peer, ttl)?;
            fs::write(&out, ticket.encode()?)
                .with_context(|| format!("failed to write {}", out.display()))?;
            print_json(&json!({"path": out.display().to_string()}))?;
        }
        Command::Join { ticket_file } => {
            let contents = fs::read_to_string(&ticket_file)
                .with_context(|| format!("failed to read {}", ticket_file.display()))?;
            let ticket = InviteTicket::decode(contents.trim())?;
            print_json(&json!({
                "issuer": ticket.issuer().to_string(),
                "seed": ticket.seed()?.to_string(),
                "expires_at": ticket.expires_at()
            }))?;
        }
        Command::Verify { file } => {
            let contents = fs::read_to_string(&file)
                .with_context(|| format!("failed to read {}", file.display()))?;
            let biography: Biography = serde_json::from_str(&contents)
                .with_context(|| format!("failed to parse {}", file.display()))?;
            let agent_id = biography
                .verify()
                .context("biography verification failed")?;
            print_json(&json!({
                "valid": true,
                "agent_id": agent_id.to_string(),
                "description": biography.description,
                "tags": biography.tags
            }))?;
        }
    }
    Ok(())
}

async fn call(socket: &Option<PathBuf>, request: IpcRequest) -> Result<IpcResponse> {
    daemon::call(&socket_path(socket)?, &request)
        .await
        .map_err(|error| anyhow!("daemon request failed: {error}"))
}

fn print_response(response: IpcResponse) -> Result<()> {
    if !response.ok {
        bail!(response
            .error
            .unwrap_or_else(|| "daemon request failed".to_owned()));
    }
    print_json(&response.data)
}

fn read_message(path: &Path) -> Result<AgentMessage> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let message: AgentMessage =
        serde_json::from_str(&contents).with_context(|| "invalid AgentMessage JSON")?;
    message
        .validate()
        .map_err(|error| anyhow!(error.to_string()))?;
    Ok(message)
}

fn parse_connectivity_policy(value: &str) -> Result<ConnectivityPolicy> {
    match value {
        "auto" => Ok(ConnectivityPolicy::Auto),
        "direct" => Ok(ConnectivityPolicy::Direct),
        "relay" => Ok(ConnectivityPolicy::Relay),
        "privacy" => Ok(ConnectivityPolicy::Privacy),
        _ => bail!("connectivity must be 'auto', 'direct', 'relay', or 'privacy'"),
    }
}

fn init(path: &Option<PathBuf>, passphrase_file: &Option<PathBuf>) -> Result<()> {
    let path = keystore_path(path.clone())?;
    let passphrase = read_passphrase(passphrase_file.as_deref(), true)?;
    let identity = Identity::generate();
    keystore::create(&path, &identity, passphrase.as_str())?;
    print_json(&json!({
        "path": path.display().to_string(),
        "agent_id": identity.agent_id().to_string()
    }))
}

fn load_identity(path: &Option<PathBuf>, passphrase_file: &Option<PathBuf>) -> Result<Identity> {
    let path = keystore_path(path.clone())?;
    let passphrase = read_passphrase(passphrase_file.as_deref(), false)?;
    Ok(keystore::unlock(&path, passphrase.as_str())?)
}

fn keystore_path(path: Option<PathBuf>) -> Result<PathBuf> {
    match path {
        Some(path) => Ok(path),
        None => Ok(keystore::default_path()?),
    }
}

fn default_inbox_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| anyhow!("cannot determine home directory"))?;
    Ok(PathBuf::from(home).join(".sceadu").join("inbox.sqlite"))
}

fn socket_path(explicit: &Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.clone());
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| anyhow!("cannot determine home directory"))?;
    Ok(PathBuf::from(home).join(".sceadu").join("daemon.sock"))
}

fn default_skill_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| anyhow!("cannot determine home directory"))?;
    Ok(PathBuf::from(home).join(".agents/skills/sceadu"))
}

fn skill_dir(dir: Option<PathBuf>) -> Result<PathBuf> {
    match dir {
        Some(dir) => Ok(dir),
        None => default_skill_dir(),
    }
}

fn read_passphrase(file: Option<&Path>, confirm: bool) -> Result<Zeroizing<String>> {
    let passphrase = if let Some(file) = file {
        let contents = fs::read_to_string(file)
            .with_context(|| format!("failed to read passphrase file {}", file.display()))?;
        Zeroizing::new(contents.trim_end_matches(['\r', '\n']).to_owned())
    } else {
        Zeroizing::new(prompt_password("Passphrase: ").context("failed to read passphrase")?)
    };

    if passphrase.len() < 8 {
        bail!("passphrase must be at least 8 characters");
    }
    if confirm {
        let again = if file.is_some() {
            passphrase.clone()
        } else {
            Zeroizing::new(
                prompt_password("Confirm passphrase: ").context("failed to read passphrase")?,
            )
        };
        if passphrase.as_str() != again.as_str() {
            bail!("passphrases do not match");
        }
    }
    Ok(passphrase)
}

fn build_biography(
    identity: &Identity,
    description: String,
    tags: Vec<String>,
    endpoints: Vec<String>,
) -> Result<Biography> {
    let endpoints = if endpoints.is_empty() {
        vec!["/ip6/::1/tcp/0".to_owned()]
    } else {
        endpoints
    };
    for value in &endpoints {
        let address = value
            .parse::<Multiaddr>()
            .with_context(|| format!("invalid endpoint multiaddr: {value}"))?;
        ensure_ipv6_peer_addr(&address)?;
    }
    let tags = if tags.is_empty() {
        vec!["tennis".to_owned()]
    } else {
        tags
    };
    Ok(Biography::new(identity, endpoints, description, tags)?)
}

fn bootstrap_peers(cli: Vec<Multiaddr>) -> Result<Vec<Multiaddr>> {
    if !cli.is_empty() {
        return Ok(cli);
    }
    if let Ok(value) = std::env::var("SCEADU_BOOTSTRAP") {
        if !value.trim().is_empty() {
            return value
                .split(',')
                .map(|value| {
                    value
                        .trim()
                        .parse::<Multiaddr>()
                        .with_context(|| format!("invalid SCEADU_BOOTSTRAP multiaddr: {value}"))
                })
                .collect();
        }
    }
    if DEFAULT_BOOTSTRAP.is_empty() {
        return Ok(Vec::new());
    }
    Ok(vec![DEFAULT_BOOTSTRAP
        .parse::<Multiaddr>()
        .context("invalid DEFAULT_BOOTSTRAP multiaddr")?])
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
