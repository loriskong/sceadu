//! L2 discovery: DHT records, neighbor exchange, and invites.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invite error: {0}")]
    Invite(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub mod invite;

pub use invite::InviteTicket;

/// How a node answers neighbor queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NeighborPolicy {
    /// Return verified manifests from the local peer cache.
    #[default]
    Open,
    /// Return no manifests.
    Stealth,
}

impl NeighborPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Stealth => "stealth",
        }
    }
}

use std::collections::HashSet;

use libp2p::kad::RecordKey;
use sha2::{Digest, Sha256};

use crate::identity::{AgentId, ContactRecord};

pub(crate) fn manifest_key(agent_id: &AgentId) -> RecordKey {
    let digest = Sha256::digest(format!("aan/id/{agent_id}").as_bytes());
    RecordKey::new(&digest)
}

pub(crate) fn capability_key(capability: &str) -> RecordKey {
    let normalized = capability.trim().to_ascii_lowercase();
    let digest = Sha256::digest(format!("aan/cap/{normalized}").as_bytes());
    RecordKey::new(&digest)
}

pub(crate) fn validate_contacts(
    records: Vec<ContactRecord>,
    capability: Option<&str>,
    max: u32,
) -> Vec<ContactRecord> {
    let limit = max.clamp(1, 32) as usize;
    let mut seen = HashSet::new();
    let mut valid = Vec::new();

    for record in records {
        if valid.len() >= limit {
            break;
        }
        if record.manifest.verify().is_err() {
            continue;
        }
        if let Some(cap) = capability {
            if !record
                .manifest
                .body
                .capabilities
                .iter()
                .any(|capability| capability.id == cap)
            {
                continue;
            }
        }
        if seen.insert(record.manifest.body.agent_id.to_string()) {
            valid.push(record);
        }
    }

    valid
}
