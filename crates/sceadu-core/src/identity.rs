use std::fmt;
use std::str::FromStr;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand_core::OsRng;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Error;

/// Prefix used by the self-certifying agent identifier.
pub const AGENT_ID_PREFIX: &str = "aan:";

/// A self-certifying agent identifier derived from an Ed25519 public key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentId(VerifyingKey);

impl AgentId {
    pub fn from_verifying_key(key: VerifyingKey) -> Self {
        Self(key)
    }

    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, Error> {
        VerifyingKey::from_bytes(bytes)
            .map(Self)
            .map_err(|_| Error::InvalidAgentId)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    pub fn verifying_key(&self) -> &VerifyingKey {
        &self.0
    }

    pub fn to_base58(&self) -> String {
        bs58::encode(self.0.as_bytes()).into_string()
    }

    /// Multibase base58btc representation (`z` prefix).
    pub fn to_multibase(&self) -> String {
        format!("z{}", self.to_base58())
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}{}", AGENT_ID_PREFIX, self.to_multibase())
    }
}

impl FromStr for AgentId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let encoded = value
            .strip_prefix(AGENT_ID_PREFIX)
            .ok_or(Error::InvalidAgentId)?;
        let encoded = encoded.strip_prefix('z').ok_or(Error::InvalidAgentId)?;
        let bytes = bs58::decode(encoded)
            .into_vec()
            .map_err(|_| Error::InvalidAgentId)?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| Error::InvalidAgentId)?;
        Self::from_bytes(&bytes)
    }
}

impl Serialize for AgentId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for AgentId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

/// An Ed25519 signing identity.
///
/// This type is intentionally not serializable. Persisting a signing key is a
/// keystore concern and must never happen through a generic serde path.
#[derive(Clone)]
pub struct Identity {
    signing_key: SigningKey,
}

impl Identity {
    pub fn generate() -> Self {
        Self {
            signing_key: SigningKey::generate(&mut OsRng),
        }
    }

    /// Reconstruct from a 32-byte seed.
    ///
    /// Keystore and test code only. Production callers must obtain the seed
    /// from a CSPRNG, never from a user passphrase or other low-entropy input.
    pub(crate) fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            signing_key: SigningKey::from_bytes(&seed),
        }
    }

    /// Export the 32-byte seed for encrypted storage.
    ///
    /// The returned array is secret material. Callers must zeroize it.
    pub(crate) fn to_seed(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    pub fn agent_id(&self) -> AgentId {
        AgentId(self.signing_key.verifying_key())
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    pub fn sign(&self, message: &[u8]) -> Signature {
        self.signing_key.sign(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_id_round_trips_through_text() {
        let identity = Identity::from_seed([7u8; 32]);
        let agent_id = identity.agent_id();
        let text = agent_id.to_string();

        assert!(text.starts_with("aan:z"));
        assert_eq!(text.parse::<AgentId>().unwrap(), agent_id);
    }

    #[test]
    fn rejects_agent_id_without_prefix() {
        let identity = Identity::from_seed([9u8; 32]);
        let text = identity.agent_id().to_base58();

        assert!(text.parse::<AgentId>().is_err());
    }
}
