use std::fs;
use std::net::SocketAddrV6;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use rpassword::prompt_password;
use sceadu_core::keystore;
use sceadu_core::{AgentId, Capability, Identity, Manifest, ManifestBody};
use sceadu_net::{
    ensure_ipv6_listen_addr, ensure_ipv6_peer_addr, spawn as spawn_node, spawn_with_config,
    ConnectivityPolicy, InviteTicket, Multiaddr, NeighborPolicy, NodeConfig, TaskEventKind,
};
use serde::Serialize;
use serde_json::json;
use zeroize::Zeroizing;

#[derive(Debug, Parser)]
#[command(name = "sceadu", version, about = "Sceadu AAN command-line client")]
struct Cli {
    /// Path to the identity keystore.
    #[arg(long, global = true)]
    path: Option<PathBuf>,

    /// Read the passphrase from a file (tests and automation only).
    #[arg(long, global = true)]
    passphrase_file: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum NetCommand {
    /// Print local connectivity status.
    Status {
        /// Listen multiaddr used for the status node.
        #[arg(long, default_value = "/ip6/::1/tcp/0")]
        listen: Multiaddr,
        /// Connectivity policy to report.
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
        /// SOCKS5 proxy for I2P dialing.
        #[arg(long)]
        i2p_proxy: Option<SocketAddrV6>,
        /// External I2P endpoint to advertise.
        #[arg(long)]
        i2p_address: Option<Multiaddr>,
    },
}

#[derive(Debug, Subcommand)]
enum SkillCommand {
    /// Write SKILL.md under the skill directory.
    Install {
        /// Override the skill directory.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Overwrite an existing SKILL.md.
        #[arg(long)]
        force: bool,
    },
    /// Print the installed skill path.
    Path {
        /// Override the skill directory.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create an encrypted identity keystore.
    Init,
    /// Print the AgentID from an existing keystore.
    Id,
    /// Sign a manifest with the persisted identity.
    Manifest {
        /// Capability to advertise.
        #[arg(long, default_value = "task.echo")]
        capability: String,
        /// Endpoint multiaddr. Repeat for multiple endpoints.
        #[arg(long = "endpoint")]
        endpoint: Vec<String>,
        /// Manifest lifetime in seconds.
        #[arg(long, default_value_t = 3600)]
        ttl: u64,
    },
    /// Start a node and answer manifest and echo requests until Ctrl-C.
    Serve {
        /// Listen multiaddr.
        #[arg(long, default_value = "/ip6/::1/tcp/0")]
        listen: Multiaddr,
        /// Neighbor query policy: open or stealth.
        #[arg(long, default_value = "open")]
        neighbor_policy: String,
        /// Connectivity policy: auto, direct, relay, or privacy.
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
        /// SOCKS5 proxy for I2P dialing.
        #[arg(long)]
        i2p_proxy: Option<SocketAddrV6>,
        /// External I2P endpoint to advertise.
        #[arg(long)]
        i2p_address: Option<Multiaddr>,
        /// Known peer to dial and exchange manifests with at startup.
        #[arg(long = "peer")]
        peers: Vec<Multiaddr>,
    },
    /// Inspect local connectivity policy and transports.
    Net {
        #[command(subcommand)]
        command: NetCommand,
    },
    /// Dial a node, exchange manifests, and send an echo request.
    Echo {
        /// Peer multiaddr, including /p2p/<peer-id>.
        #[arg(long)]
        addr: Multiaddr,
        /// Message to echo.
        #[arg(long, default_value = "hello")]
        message: String,
    },
    /// Install the Sceadu agent skill.
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// Issue a signed bootstrap invite for a known peer.
    Invite {
        /// Seed peer multiaddr, including /p2p/<peer-id>.
        #[arg(long)]
        peer: Multiaddr,
        /// Lifetime in seconds.
        #[arg(long, default_value_t = 3600)]
        ttl: u64,
        /// Write the encoded invite to a file instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Verify an invite and bootstrap from its seed.
    Join {
        /// Encoded invite string.
        #[arg(long)]
        ticket: Option<String>,
        /// File containing an encoded invite.
        #[arg(long)]
        ticket_file: Option<PathBuf>,
    },
    /// Publish this identity's manifest and capability provider records to the DHT.
    Publish {
        /// DHT peer multiaddr, including /p2p/<peer-id>.
        #[arg(long)]
        peer: Multiaddr,
    },
    /// Look up an agent's manifest in the DHT.
    Lookup {
        /// DHT peer multiaddr, including /p2p/<peer-id>.
        #[arg(long)]
        peer: Multiaddr,
        /// AgentID to look up.
        agent_id: AgentId,
    },
    /// Ask a peer to run a capability and stream task events.
    Ask {
        /// Peer multiaddr, including /p2p/<peer-id>.
        #[arg(long)]
        addr: Multiaddr,
        /// Capability to invoke.
        #[arg(long, default_value = "task.echo")]
        capability: String,
        /// JSON input object.
        #[arg(long, default_value = "{}")]
        input: String,
        /// Task timeout in seconds.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
        /// Connectivity policy: auto, direct, relay, or privacy.
        #[arg(long, default_value = "auto")]
        connectivity: String,
        /// Relay peer to reserve a circuit with.
        #[arg(long)]
        relay: Option<Multiaddr>,
        /// SOCKS5 proxy for Tor onion dialing.
        #[arg(long)]
        tor_proxy: Option<SocketAddrV6>,
        /// SOCKS5 proxy for I2P dialing.
        #[arg(long)]
        i2p_proxy: Option<SocketAddrV6>,
    },
    /// Ask a connected peer for its verified neighbors.
    Discover {
        /// Introducer multiaddr, including /p2p/<peer-id>.
        #[arg(long)]
        addr: Multiaddr,
        /// Filter by capability.
        #[arg(long)]
        capability: Option<String>,
        /// Maximum number of records to request.
        #[arg(long, default_value_t = 8)]
        max: u32,
    },
    /// Generate an ephemeral development identity and print it.
    DevId,
    /// Sign a development manifest with an ephemeral identity.
    DevManifest {
        /// Capability to advertise.
        #[arg(long, default_value = "task.echo")]
        capability: String,
        /// Endpoint multiaddr. Repeat for multiple endpoints.
        #[arg(long = "endpoint")]
        endpoint: Vec<String>,
        /// Manifest lifetime in seconds.
        #[arg(long, default_value_t = 3600)]
        ttl: u64,
    },
    /// Verify a manifest JSON file.
    Verify {
        /// Path to a manifest JSON file.
        file: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Init => {
            let path = keystore_path(cli.path)?;
            if path.exists() {
                bail!(
                    "keystore already exists at {}; move it aside before creating a new identity",
                    path.display()
                );
            }

            let passphrase = read_passphrase(cli.passphrase_file.as_deref(), true)?;
            let identity = Identity::generate();
            keystore::create(&path, &identity, passphrase.as_str())?;

            print_json(&json!({
                "agent_id": identity.agent_id().to_string(),
                "path": path.display().to_string(),
                "note": "back up this file and passphrase; there is no recovery"
            }))?;
        }
        Command::Id => {
            let path = keystore_path(cli.path)?;
            let passphrase = read_passphrase(cli.passphrase_file.as_deref(), false)?;
            let identity = keystore::unlock(&path, passphrase.as_str())?;

            print_json(&json!({
                "agent_id": identity.agent_id().to_string(),
                "path": path.display().to_string()
            }))?;
        }
        Command::Manifest {
            capability,
            endpoint,
            ttl,
        } => {
            let path = keystore_path(cli.path)?;
            let passphrase = read_passphrase(cli.passphrase_file.as_deref(), false)?;
            let identity = keystore::unlock(&path, passphrase.as_str())?;
            let manifest = build_manifest(&identity, capability, endpoint, ttl)?;

            print_json(&manifest)?;
        }
        Command::Serve {
            listen,
            neighbor_policy,
            connectivity,
            relay,
            relay_server,
            tor_proxy,
            onion,
            i2p_proxy,
            i2p_address,
            peers,
        } => {
            let identity = load_identity(&cli.path, &cli.passphrase_file)?;
            let policy = parse_neighbor_policy(&neighbor_policy)?;
            let connectivity_policy = parse_connectivity_policy(&connectivity)?;
            let listen = if connectivity_policy == ConnectivityPolicy::Relay && !relay_server {
                None
            } else {
                Some(listen)
            };
            if let Some(address) = &listen {
                ensure_ipv6_listen_addr(address)?;
            }
            let (node, address) = spawn_with_config(
                &identity,
                listen,
                NodeConfig {
                    neighbor_policy: policy,
                    connectivity_policy,
                    relay,
                    relay_server,
                    tor_proxy,
                    onion: onion.clone(),
                    i2p_proxy,
                    i2p_address: i2p_address.clone(),
                },
            )
            .await?;
            let address = address.ok_or_else(|| anyhow!("no listen address"))?;

            let mut bootstrapped = Vec::new();
            for peer_addr in peers {
                let peer = node
                    .dial(peer_addr.clone())
                    .await
                    .with_context(|| format!("failed to dial {}", peer_addr))?;
                let manifest = node
                    .request_manifest(peer)
                    .await
                    .with_context(|| format!("manifest exchange with {}", peer_addr))?;
                bootstrapped.push(json!({
                    "peer": peer.to_string(),
                    "agent_id": manifest.body.agent_id.to_string()
                }));
            }

            print_json(&json!({
                "agent_id": node.agent_id().to_string(),
                "peer_id": node.peer_id().to_string(),
                "listen": address.to_string(),
                "capabilities": ["task.echo"],
                "neighbor_policy": policy.as_str(),
                "connectivity_policy": connectivity_policy.as_str(),
                "relay_server": relay_server,
                "tor_proxy": tor_proxy.map(|proxy| proxy.to_string()),
                "onion": onion.as_ref().map(|addr| addr.to_string()),
                "i2p_proxy": i2p_proxy.map(|proxy| proxy.to_string()),
                "i2p_address": i2p_address.as_ref().map(|addr| addr.to_string()),
                "peers": bootstrapped,
                "note": "press Ctrl-C to stop"
            }))?;

            tokio::signal::ctrl_c()
                .await
                .context("failed to wait for Ctrl-C")?;
            node.shutdown().await;
        }
        Command::Echo { addr, message } => {
            let identity = load_identity(&cli.path, &cli.passphrase_file)?;
            let (node, _) = spawn_node(&identity, None).await?;
            let peer = node.dial(addr).await?;
            let manifest = node.request_manifest(peer).await?;
            let reply = node.echo(peer, message).await?;

            print_json(&json!({
                "peer": peer.to_string(),
                "agent_id": manifest.body.agent_id.to_string(),
                "echo": reply
            }))?;

            node.shutdown().await;
        }
        Command::Net { command } => match command {
            NetCommand::Status {
                listen,
                connectivity,
                relay,
                relay_server,
                tor_proxy,
                onion,
                i2p_proxy,
                i2p_address,
            } => {
                let identity = load_identity(&cli.path, &cli.passphrase_file)?;
                let connectivity_policy = parse_connectivity_policy(&connectivity)?;
                let listen = if connectivity_policy == ConnectivityPolicy::Relay && !relay_server {
                    None
                } else if matches!(
                    connectivity_policy,
                    ConnectivityPolicy::Auto | ConnectivityPolicy::Direct
                ) {
                    Some(listen)
                } else {
                    None
                };
                if let Some(address) = &listen {
                    ensure_ipv6_listen_addr(address)?;
                }
                let (node, _) = spawn_with_config(
                    &identity,
                    listen,
                    NodeConfig {
                        connectivity_policy,
                        relay,
                        relay_server,
                        tor_proxy,
                        onion,
                        i2p_proxy,
                        i2p_address,
                        ..Default::default()
                    },
                )
                .await?;
                let status = node.network_status().await?;
                print_json(&status)?;
                node.shutdown().await;
            }
        },
        Command::Skill { command } => match command {
            SkillCommand::Install { dir, force } => {
                let dir = skill_dir(dir)?;
                fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
                let path = dir.join("SKILL.md");
                if path.exists() && !force {
                    bail!(
                        "skill already exists at {}; use --force to overwrite",
                        path.display()
                    );
                }
                fs::write(&path, include_str!("../../../skills/sceadu/SKILL.md"))
                    .with_context(|| format!("writing {}", path.display()))?;
                print_json(&json!({
                    "installed": path.display().to_string()
                }))?;
            }
            SkillCommand::Path { dir } => {
                let dir = skill_dir(dir)?;
                print_json(&json!({
                    "path": dir.join("SKILL.md").display().to_string()
                }))?;
            }
        },
        Command::Invite { peer, ttl, out } => {
            let identity = load_identity(&cli.path, &cli.passphrase_file)?;
            let ticket = InviteTicket::issue(&identity, peer, ttl)?;
            let encoded = ticket.encode()?;
            let seed = ticket.seed()?;

            if let Some(path) = out {
                std::fs::write(&path, format!("{encoded}\n"))
                    .with_context(|| format!("writing {}", path.display()))?;
                print_json(&json!({
                    "out": path.display().to_string(),
                    "issuer": ticket.issuer().to_string(),
                    "seed": seed.to_string(),
                    "expires_at": ticket.expires_at()
                }))?;
            } else {
                print_json(&json!({
                    "invite": encoded,
                    "issuer": ticket.issuer().to_string(),
                    "seed": seed.to_string(),
                    "expires_at": ticket.expires_at()
                }))?;
            }
        }
        Command::Join {
            ticket,
            ticket_file,
        } => {
            let encoded = match (ticket, ticket_file) {
                (Some(ticket), None) => ticket,
                (None, Some(path)) => std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?
                    .trim()
                    .to_owned(),
                _ => bail!("provide exactly one of --ticket or --ticket-file"),
            };
            let ticket = InviteTicket::decode(&encoded)?;
            let seed = ticket.seed()?;
            let identity = load_identity(&cli.path, &cli.passphrase_file)?;
            let (node, _) = spawn_node(&identity, None).await?;
            let peer = node.dial(seed.clone()).await?;
            let manifest = node.request_manifest(peer).await?;

            print_json(&json!({
                "issuer": ticket.issuer().to_string(),
                "seed": seed.to_string(),
                "introducer_peer": peer.to_string(),
                "introducer_agent_id": manifest.body.agent_id.to_string()
            }))?;

            node.shutdown().await;
        }
        Command::Publish { peer } => {
            let identity = load_identity(&cli.path, &cli.passphrase_file)?;
            let (node, listen) =
                spawn_node(&identity, Some("/ip6/::1/tcp/0".parse::<Multiaddr>()?)).await?;
            let listen = listen.ok_or_else(|| anyhow!("no listen address"))?;
            let peer_id = node.dial(peer.clone()).await?;
            node.add_address(peer_id, peer.clone()).await?;
            node.bootstrap().await?;
            node.publish().await?;
            node.provide("task.echo").await?;

            print_json(&json!({
                "agent_id": node.agent_id().to_string(),
                "listen": listen.to_string(),
                "peer": peer_id.to_string(),
                "dht_peer": peer.to_string(),
                "capabilities": ["task.echo"]
            }))?;

            node.shutdown().await;
        }
        Command::Lookup { peer, agent_id } => {
            let identity = load_identity(&cli.path, &cli.passphrase_file)?;
            let (node, _) =
                spawn_node(&identity, Some("/ip6/::1/tcp/0".parse::<Multiaddr>()?)).await?;
            let peer_id = node.dial(peer.clone()).await?;
            node.add_address(peer_id, peer.clone()).await?;
            node.bootstrap().await?;
            let manifest = node.lookup(agent_id).await?;

            print_json(&json!({
                "dht_peer": peer_id.to_string(),
                "manifest": manifest
            }))?;

            node.shutdown().await;
        }
        Command::Ask {
            addr,
            capability,
            input,
            timeout,
            connectivity,
            relay,
            tor_proxy,
            i2p_proxy,
        } => {
            let input: serde_json::Value =
                serde_json::from_str(&input).with_context(|| "invalid --input JSON")?;
            let connectivity_policy = parse_connectivity_policy(&connectivity)?;
            let identity = load_identity(&cli.path, &cli.passphrase_file)?;
            let (node, _) = spawn_with_config(
                &identity,
                None,
                NodeConfig {
                    connectivity_policy,
                    relay,
                    tor_proxy,
                    i2p_proxy,
                    ..Default::default()
                },
            )
            .await?;
            let peer = node.dial(addr).await?;
            let _ = node.request_manifest(peer).await?;
            let mut handle = node
                .ask(peer, capability, input, Duration::from_secs(timeout))
                .await?;

            let mut failed = false;
            loop {
                match tokio::time::timeout(Duration::from_secs(timeout), handle.next()).await {
                    Ok(Some(event)) => {
                        let done = matches!(&event.event, TaskEventKind::Done);
                        if matches!(&event.event, TaskEventKind::Failed { .. }) {
                            failed = true;
                        }
                        println!("{}", serde_json::to_string(&event)?);
                        if done {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => {
                        node.shutdown().await;
                        bail!("task timed out");
                    }
                }
            }

            node.shutdown().await;
            if failed {
                bail!("task failed");
            }
        }
        Command::Discover {
            addr,
            capability,
            max,
        } => {
            let identity = load_identity(&cli.path, &cli.passphrase_file)?;
            let (node, _) = spawn_node(&identity, None).await?;
            let peer = node.dial(addr).await?;
            let _ = node.request_manifest(peer).await?;
            let records = node.neighbors(peer, capability.clone(), max).await?;

            let neighbors: Vec<_> = records
                .iter()
                .map(|record| {
                    json!({
                        "agent_id": record.manifest.body.agent_id.to_string(),
                        "endpoints": record.manifest.body.endpoints,
                        "observed_at": record.observed_at
                    })
                })
                .collect();

            print_json(&json!({
                "introducer": peer.to_string(),
                "capability": capability,
                "neighbors": neighbors
            }))?;

            node.shutdown().await;
        }
        Command::DevId => {
            let identity = Identity::generate();
            let agent_id = identity.agent_id();
            print_json(&json!({
                "agent_id": agent_id.to_string(),
                "base58": agent_id.to_base58(),
                "note": "ephemeral development identity; the persistent keystore is available via `sceadu init`"
            }))?;
        }
        Command::DevManifest {
            capability,
            endpoint,
            ttl,
        } => {
            let identity = Identity::generate();
            let manifest = build_manifest(&identity, capability, endpoint, ttl)?;
            print_json(&manifest)?;
        }
        Command::Verify { file } => {
            let contents = fs::read_to_string(&file)
                .with_context(|| format!("failed to read {}", file.display()))?;
            let manifest: Manifest = serde_json::from_str(&contents)
                .with_context(|| format!("failed to parse {}", file.display()))?;
            let agent_id = manifest.verify().context("manifest verification failed")?;

            print_json(&json!({
                "valid": true,
                "agent_id": agent_id.to_string()
            }))?;
        }
    }

    Ok(())
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

fn parse_connectivity_policy(value: &str) -> Result<ConnectivityPolicy> {
    match value {
        "auto" => Ok(ConnectivityPolicy::Auto),
        "direct" => Ok(ConnectivityPolicy::Direct),
        "relay" => Ok(ConnectivityPolicy::Relay),
        "privacy" => Ok(ConnectivityPolicy::Privacy),
        _ => bail!("connectivity must be 'auto', 'direct', 'relay', or 'privacy'"),
    }
}

fn parse_neighbor_policy(value: &str) -> Result<NeighborPolicy> {
    match value {
        "open" => Ok(NeighborPolicy::Open),
        "stealth" => Ok(NeighborPolicy::Stealth),
        _ => bail!("neighbor policy must be 'open' or 'stealth'"),
    }
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

fn build_manifest(
    identity: &Identity,
    capability: String,
    endpoint: Vec<String>,
    ttl: u64,
) -> Result<Manifest> {
    let mut body = ManifestBody::new(identity.agent_id(), 1, ttl);
    body.capabilities.push(Capability::new(capability));
    let endpoints = if endpoint.is_empty() {
        vec!["/ip6/::1/tcp/0".to_owned()]
    } else {
        endpoint
    };
    for value in &endpoints {
        let address = value
            .parse::<Multiaddr>()
            .with_context(|| format!("invalid endpoint multiaddr: {value}"))?;
        ensure_ipv6_peer_addr(&address)?;
    }
    body.endpoints = endpoints;
    body.protocols.push("/sceadu/echo/1.0.0".to_owned());

    Ok(Manifest::sign(body, identity)?)
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
