use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::Signature;
use serde::{Deserialize, Serialize};

use super::Error;
use crate::identity::{AgentId, Identity};

/// Current Biography wire version.
pub const BIOGRAPHY_PROTOCOL: &str = "sceadu.biography/0.1";
pub const MAX_BIOGRAPHY_DESCRIPTION: usize = 1024;
pub const MAX_BIOGRAPHY_TAGS: usize = 16;
pub const MAX_BIOGRAPHY_TAG_LEN: usize = 32;
pub const MAX_BIOGRAPHY_ENDPOINTS: usize = 8;

/// A signed, structured self-description of an agent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Biography {
    /// Wire protocol identifier.
    pub protocol: String,
    /// Self-certifying agent identifier.
    pub agent_id: AgentId,
    /// Base64-encoded Ed25519 public key.
    pub public_key: String,
    /// Reachable multiaddrs, including `/p2p/<peer-id>`.
    pub endpoints: Vec<String>,
    /// Human-readable self-description. Never include secrets or private contact details.
    pub description: String,
    /// Capability/region/interest labels used by agent-side matching.
    pub tags: Vec<String>,
    /// Unix seconds when this record was last changed.
    pub updated_at: u64,
    /// Base64-encoded Ed25519 signature over the canonical unsigned record.
    pub signature: String,
}

#[derive(Serialize)]
struct Unsigned<'a> {
    protocol: &'a str,
    agent_id: &'a AgentId,
    public_key: &'a str,
    endpoints: &'a [String],
    description: &'a str,
    tags: &'a [String],
    updated_at: u64,
}

impl Biography {
    pub fn new(
        identity: &Identity,
        endpoints: Vec<String>,
        description: impl Into<String>,
        tags: Vec<String>,
    ) -> Result<Self, Error> {
        let public_key = STANDARD.encode(identity.verifying_key().as_bytes());
        let mut biography = Self {
            protocol: BIOGRAPHY_PROTOCOL.to_owned(),
            agent_id: identity.agent_id(),
            public_key,
            endpoints,
            description: description.into(),
            tags,
            updated_at: now_unix(),
            signature: String::new(),
        };
        biography.validate_fields()?;
        biography.sign(identity)?;
        Ok(biography)
    }

    fn unsigned(&self) -> Unsigned<'_> {
        Unsigned {
            protocol: &self.protocol,
            agent_id: &self.agent_id,
            public_key: &self.public_key,
            endpoints: &self.endpoints,
            description: &self.description,
            tags: &self.tags,
            updated_at: self.updated_at,
        }
    }

    fn canonical_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        ciborium::into_writer(&self.unsigned(), &mut bytes)
            .map_err(|error| Error::Serialization(error.to_string()))?;
        Ok(bytes)
    }

    pub fn sign(&mut self, identity: &Identity) -> Result<(), Error> {
        if self.agent_id != identity.agent_id() {
            return Err(Error::AgentIdMismatch);
        }
        let payload = self.canonical_bytes()?;
        let signature = identity.sign(&payload);
        self.signature = STANDARD.encode(signature.to_bytes());
        Ok(())
    }

    pub fn verify(&self) -> Result<AgentId, Error> {
        self.validate_fields()?;
        if self.protocol != BIOGRAPHY_PROTOCOL {
            return Err(Error::UnsupportedVersion(1));
        }
        let public_key = STANDARD
            .decode(&self.public_key)
            .map_err(|_| Error::InvalidAgentId)?;
        let public_key: [u8; 32] = public_key.try_into().map_err(|_| Error::InvalidAgentId)?;
        let expected = AgentId::from_bytes(&public_key)?;
        if expected != self.agent_id {
            return Err(Error::AgentIdMismatch);
        }

        let signature_bytes = STANDARD
            .decode(&self.signature)
            .map_err(|_| Error::InvalidSignatureEncoding)?;
        let signature =
            Signature::from_slice(&signature_bytes).map_err(|_| Error::InvalidSignatureEncoding)?;
        let payload = self.canonical_bytes()?;
        expected
            .verifying_key()
            .verify_strict(&payload, &signature)
            .map_err(|_| Error::InvalidSignature)?;
        Ok(self.agent_id.clone())
    }

    fn validate_fields(&self) -> Result<(), Error> {
        if self.description.len() > MAX_BIOGRAPHY_DESCRIPTION {
            return Err(Error::InvalidBiography(
                "description is too long".to_owned(),
            ));
        }
        if self.endpoints.len() > MAX_BIOGRAPHY_ENDPOINTS {
            return Err(Error::InvalidBiography(
                "biography may contain at most 8 endpoints".to_owned(),
            ));
        }
        if self.tags.len() > MAX_BIOGRAPHY_TAGS {
            return Err(Error::InvalidBiography("too many tags".to_owned()));
        }
        for tag in &self.tags {
            if tag.is_empty()
                || tag.len() > MAX_BIOGRAPHY_TAG_LEN
                || !tag
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                return Err(Error::InvalidBiography(format!("invalid tag: {tag}")));
            }
        }
        Ok(())
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn biography(identity: &Identity) -> Biography {
        Biography::new(
            identity,
            vec!["/ip6/::1/tcp/4001".to_owned()],
            "test agent",
            vec!["tennis".to_owned()],
        )
        .unwrap()
    }

    #[test]
    fn signs_and_verifies() {
        let identity = Identity::from_seed([1u8; 32]);
        assert_eq!(biography(&identity).verify().unwrap(), identity.agent_id());
    }

    #[test]
    fn rejects_tampering() {
        let identity = Identity::from_seed([2u8; 32]);
        let mut value = biography(&identity);
        value.tags.push("other".to_owned());
        assert!(matches!(value.verify(), Err(Error::InvalidSignature)));
    }

    #[test]
    fn rejects_agent_id_mismatch() {
        let signer = Identity::from_seed([3u8; 32]);
        let other = Identity::from_seed([4u8; 32]);
        assert!(matches!(
            Biography::new(&signer, vec!["/ip6/::1/tcp/4001".to_owned()], "x", vec![]).and_then(
                |mut value| {
                    value.agent_id = other.agent_id();
                    value.sign(&signer)
                }
            ),
            Err(Error::AgentIdMismatch)
        ));
    }

    #[test]
    fn accepts_empty_endpoints_for_relay_only_nodes() {
        let identity = Identity::from_seed([5u8; 32]);
        assert!(Biography::new(&identity, vec![], "x", vec![]).is_ok());
    }
}
