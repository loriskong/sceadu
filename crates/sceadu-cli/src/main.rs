use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use rpassword::prompt_password;
use sceadu_core::keystore;
use sceadu_core::{Capability, Identity, Manifest, ManifestBody};
use sceadu_net::{spawn as spawn_node, spawn_with_config, Multiaddr, NeighborPolicy, NodeConfig};
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
        #[arg(long, default_value = "/ip4/127.0.0.1/tcp/0")]
        listen: Multiaddr,
        /// Neighbor query policy: open or stealth.
        #[arg(long, default_value = "open")]
        neighbor_policy: String,
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
        } => {
            let identity = load_identity(&cli.path, &cli.passphrase_file)?;
            let policy = parse_neighbor_policy(&neighbor_policy)?;
            let (node, address) = spawn_with_config(
                &identity,
                Some(listen),
                NodeConfig {
                    neighbor_policy: policy,
                },
            )
            .await?;
            let address = address.ok_or_else(|| anyhow!("no listen address"))?;

            print_json(&json!({
                "agent_id": node.agent_id().to_string(),
                "peer_id": node.peer_id().to_string(),
                "listen": address.to_string(),
                "capabilities": ["task.echo"],
                "neighbor_policy": policy.as_str(),
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
    body.endpoints = if endpoint.is_empty() {
        vec!["/ip4/127.0.0.1/tcp/0".to_owned()]
    } else {
        endpoint
    };
    body.protocols.push("/sceadu/echo/1.0.0".to_owned());

    Ok(Manifest::sign(body, identity)?)
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
