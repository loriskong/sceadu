use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::Signature;
use serde::{Deserialize, Serialize};

use crate::identity::{AgentId, Identity};
use crate::Error;

/// Current manifest wire version.
pub const MANIFEST_VERSION: u32 = 1;

/// A capability advertised by an agent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    pub id: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub streaming: bool,
}

impl Capability {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            version: "1.0.0".to_owned(),
            description: String::new(),
            streaming: false,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn streaming(mut self) -> Self {
        self.streaming = true;
        self
    }
}

/// The signed part of a manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ManifestBody {
    pub version: u32,
    pub agent_id: AgentId,
    pub seq: u64,
    pub issued_at: u64,
    pub expires_at: u64,
    pub capabilities: Vec<Capability>,
    pub endpoints: Vec<String>,
    pub protocols: Vec<String>,
    pub discoverable: bool,
}

impl ManifestBody {
    pub fn new(agent_id: AgentId, seq: u64, ttl_seconds: u64) -> Self {
        let issued_at = now_unix();
        Self {
            version: MANIFEST_VERSION,
            agent_id,
            seq,
            issued_at,
            expires_at: issued_at.saturating_add(ttl_seconds),
            capabilities: Vec::new(),
            endpoints: Vec::new(),
            protocols: Vec::new(),
            discoverable: true,
        }
    }

    /// Deterministic bytes covered by the signature.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        ciborium::into_writer(self, &mut bytes)
            .map_err(|error| Error::Serialization(error.to_string()))?;
        Ok(bytes)
    }
}

/// A signed manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub body: ManifestBody,
    /// Base64-encoded Ed25519 signature over `body.canonical_bytes()`.
    pub signature: String,
}

impl Manifest {
    pub fn sign(body: ManifestBody, identity: &Identity) -> Result<Self, Error> {
        if body.agent_id != identity.agent_id() {
            return Err(Error::AgentIdMismatch);
        }

        let payload = body.canonical_bytes()?;
        let signature = identity.sign(&payload);

        Ok(Self {
            body,
            signature: STANDARD.encode(signature.to_bytes()),
        })
    }

    pub fn verify(&self) -> Result<AgentId, Error> {
        self.verify_at(now_unix())
    }

    pub fn verify_at(&self, now: u64) -> Result<AgentId, Error> {
        if self.body.version != MANIFEST_VERSION {
            return Err(Error::UnsupportedVersion(self.body.version));
        }
        if now < self.body.issued_at {
            return Err(Error::NotYetValid);
        }
        if now > self.body.expires_at {
            return Err(Error::Expired);
        }

        let signature_bytes = STANDARD
            .decode(&self.signature)
            .map_err(|_| Error::InvalidSignatureEncoding)?;
        let signature =
            Signature::from_slice(&signature_bytes).map_err(|_| Error::InvalidSignatureEncoding)?;
        let payload = self.body.canonical_bytes()?;

        self.body
            .agent_id
            .verifying_key()
            .verify_strict(&payload, &signature)
            .map_err(|_| Error::InvalidSignature)?;

        Ok(self.body.agent_id.clone())
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

    fn body(identity: &Identity) -> ManifestBody {
        let mut body = ManifestBody::new(identity.agent_id(), 1, 3600);
        body.capabilities.push(Capability::new("task.echo"));
        body.endpoints.push("/ip6/::1/tcp/4001".to_owned());
        body.protocols.push("/sceadu/echo/1.0.0".to_owned());
        body
    }

    #[test]
    fn signs_and_verifies() {
        let identity = Identity::from_seed([1u8; 32]);
        let manifest = Manifest::sign(body(&identity), &identity).unwrap();

        assert_eq!(manifest.verify_at(now_unix()).unwrap(), identity.agent_id());
    }

    #[test]
    fn rejects_tampering() {
        let identity = Identity::from_seed([2u8; 32]);
        let mut manifest = Manifest::sign(body(&identity), &identity).unwrap();
        manifest.body.capabilities[0].id = "task.other".to_owned();

        assert!(matches!(
            manifest.verify_at(now_unix()),
            Err(Error::InvalidSignature)
        ));
    }

    #[test]
    fn rejects_expired_manifest() {
        let identity = Identity::from_seed([3u8; 32]);
        let mut manifest = Manifest::sign(body(&identity), &identity).unwrap();
        manifest.body.expires_at = manifest.body.issued_at + 10;

        assert!(matches!(
            manifest.verify_at(manifest.body.expires_at + 1),
            Err(Error::Expired)
        ));
    }

    #[test]
    fn rejects_not_yet_valid_manifest() {
        let identity = Identity::from_seed([4u8; 32]);
        let mut manifest = Manifest::sign(body(&identity), &identity).unwrap();
        manifest.body.issued_at = 1_000;

        assert!(matches!(manifest.verify_at(999), Err(Error::NotYetValid)));
    }

    #[test]
    fn rejects_agent_id_mismatch_when_signing() {
        let signer = Identity::from_seed([5u8; 32]);
        let other = Identity::from_seed([6u8; 32]);

        assert!(matches!(
            Manifest::sign(body(&other), &signer),
            Err(Error::AgentIdMismatch)
        ));
    }
}

/// A signed manifest observed during discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactRecord {
    pub manifest: Manifest,
    pub observed_at: u64,
}
