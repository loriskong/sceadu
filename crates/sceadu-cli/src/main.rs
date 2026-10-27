use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use sceadu_core::{Capability, Identity, Manifest, ManifestBody};
use serde_json::json;

#[derive(Debug, Parser)]
#[command(name = "sceadu", version, about = "Sceadu AAN command-line client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
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

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::DevId => {
            let identity = Identity::generate();
            let agent_id = identity.agent_id();
            let output = json!({
                "agent_id": agent_id.to_string(),
                "base58": agent_id.to_base58(),
                "note": "ephemeral development identity; the persistent keystore is the next M1 slice"
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        Command::DevManifest {
            capability,
            endpoint,
            ttl,
        } => {
            let identity = Identity::generate();
            let mut body = ManifestBody::new(identity.agent_id(), 1, ttl);
            body.capabilities.push(Capability::new(capability));
            body.endpoints = if endpoint.is_empty() {
                vec!["/ip4/127.0.0.1/tcp/0".to_owned()]
            } else {
                endpoint
            };
            body.protocols.push("/sceadu/echo/1.0.0".to_owned());

            let manifest = Manifest::sign(body, &identity)?;
            println!("{}", serde_json::to_string_pretty(&manifest)?);
        }
        Command::Verify { file } => {
            let contents = fs::read_to_string(&file)
                .with_context(|| format!("failed to read {}", file.display()))?;
            let manifest: Manifest = serde_json::from_str(&contents)
                .with_context(|| format!("failed to parse {}", file.display()))?;
            let agent_id = manifest.verify().context("manifest verification failed")?;
            let output = json!({
                "valid": true,
                "agent_id": agent_id.to_string()
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
    }

    Ok(())
}
