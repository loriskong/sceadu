use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::Signature;
use libp2p::Multiaddr;
use rand_core::{OsRng, RngCore};
use sceadu_core::{AgentId, Identity};
use serde::{Deserialize, Serialize};

use crate::Error;

const INVITE_VERSION: u32 = 1;
const INVITE_PREFIX: &str = "aan-invite:";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InviteBody {
    version: u32,
    issuer: AgentId,
    seed: String,
    issued_at: u64,
    expires_at: u64,
    nonce: String,
}

/// A signed bootstrap ticket carrying a seed peer address.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteTicket {
    body: InviteBody,
    signature: String,
}

impl InviteTicket {
    pub fn issue(identity: &Identity, seed: Multiaddr, ttl_seconds: u64) -> Result<Self, Error> {
        crate::ensure_ipv6_peer_addr(&seed)?;
        let issued_at = now_unix();
        let mut nonce = [0u8; 16];
        OsRng.fill_bytes(&mut nonce);
        let body = InviteBody {
            version: INVITE_VERSION,
            issuer: identity.agent_id(),
            seed: seed.to_string(),
            issued_at,
            expires_at: issued_at.saturating_add(ttl_seconds),
            nonce: URL_SAFE_NO_PAD.encode(nonce),
        };
        let payload = canonical_body(&body)?;
        let signature = identity.sign(&payload);

        Ok(Self {
            body,
            signature: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
        })
    }

    pub fn verify(&self) -> Result<AgentId, Error> {
        if self.body.version != INVITE_VERSION {
            return Err(Error::Invite(format!(
                "unsupported invite version {}",
                self.body.version
            )));
        }
        let now = now_unix();
        if now < self.body.issued_at {
            return Err(Error::Invite("invite is not yet valid".to_owned()));
        }
        if now > self.body.expires_at {
            return Err(Error::Invite("invite expired".to_owned()));
        }

        let signature_bytes = URL_SAFE_NO_PAD
            .decode(&self.signature)
            .map_err(|error| Error::Invite(error.to_string()))?;
        let signature = Signature::from_slice(&signature_bytes)
            .map_err(|error| Error::Invite(error.to_string()))?;
        let payload = canonical_body(&self.body)?;

        self.body
            .issuer
            .verifying_key()
            .verify_strict(&payload, &signature)
            .map_err(|_| Error::Invite("invite signature verification failed".to_owned()))?;

        Ok(self.body.issuer.clone())
    }

    pub fn seed(&self) -> Result<Multiaddr, Error> {
        let seed = self
            .body
            .seed
            .parse()
            .map_err(|error| Error::Invite(format!("invalid seed multiaddr: {error}")))?;
        crate::ensure_ipv6_peer_addr(&seed)?;
        Ok(seed)
    }

    pub fn issuer(&self) -> &AgentId {
        &self.body.issuer
    }

    pub fn expires_at(&self) -> u64 {
        self.body.expires_at
    }

    pub fn encode(&self) -> Result<String, Error> {
        let json = serde_json::to_vec(self).map_err(|error| Error::Invite(error.to_string()))?;
        Ok(format!("{INVITE_PREFIX}{}", URL_SAFE_NO_PAD.encode(json)))
    }

    pub fn decode(value: &str) -> Result<Self, Error> {
        let encoded = value
            .trim()
            .strip_prefix(INVITE_PREFIX)
            .ok_or_else(|| Error::Invite("missing invite prefix".to_owned()))?;
        let json = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|error| Error::Invite(error.to_string()))?;
        let ticket: Self =
            serde_json::from_slice(&json).map_err(|error| Error::Invite(error.to_string()))?;
        ticket.verify()?;
        Ok(ticket)
    }
}

fn canonical_body(body: &InviteBody) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    ciborium::into_writer(body, &mut bytes).map_err(|error| Error::Invite(error.to_string()))?;
    Ok(bytes)
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

    fn seed_addr() -> Multiaddr {
        "/ip6/::1/tcp/4001".parse().unwrap()
    }

    #[test]
    fn round_trips_through_text() {
        let identity = Identity::generate();
        let ticket = InviteTicket::issue(&identity, seed_addr(), 3600).unwrap();
        let encoded = ticket.encode().unwrap();
        let decoded = InviteTicket::decode(&encoded).unwrap();

        assert_eq!(decoded.issuer(), &identity.agent_id());
        assert_eq!(decoded.seed().unwrap(), seed_addr());
    }

    #[test]
    fn rejects_tampered_body() {
        let identity = Identity::generate();
        let mut ticket = InviteTicket::issue(&identity, seed_addr(), 3600).unwrap();
        ticket.body.seed = "/ip6/::1/tcp/9999".to_owned();

        assert!(ticket.verify().is_err());
    }

    #[test]
    fn rejects_expired_ticket() {
        let identity = Identity::generate();
        let body = InviteBody {
            version: INVITE_VERSION,
            issuer: identity.agent_id(),
            seed: seed_addr().to_string(),
            issued_at: 1,
            expires_at: 2,
            nonce: "test".to_owned(),
        };
        let payload = canonical_body(&body).unwrap();
        let signature = identity.sign(&payload);
        let ticket = InviteTicket {
            body,
            signature: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
        };

        assert!(ticket.verify().is_err());
    }

    #[test]
    fn rejects_missing_prefix() {
        assert!(InviteTicket::decode("not-an-invite").is_err());
    }
}
