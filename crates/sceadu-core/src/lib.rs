//! Core types for the Sceadu Anonymous Agent Network.

mod identity;
pub mod keystore;
mod manifest;

pub use identity::{AgentId, Identity};
pub use manifest::{Capability, Manifest, ManifestBody, MANIFEST_VERSION};

/// Errors returned by the core types.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid agent id")]
    InvalidAgentId,
    #[error("invalid signature encoding")]
    InvalidSignatureEncoding,
    #[error("signature verification failed")]
    InvalidSignature,
    #[error("manifest expired")]
    Expired,
    #[error("manifest is not yet valid")]
    NotYetValid,
    #[error("unsupported manifest version {0}")]
    UnsupportedVersion(u32),
    #[error("manifest agent id does not match the signing identity")]
    AgentIdMismatch,
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("keystore already exists")]
    KeystoreExists,
    #[error("keystore not found")]
    KeystoreNotFound,
    #[error("keystore format error: {0}")]
    KeystoreFormat(String),
    #[error("keystore unlock failed")]
    KeystoreUnlockFailed,
    #[error("unsupported keystore version {0}")]
    KeystoreVersion(u32),
    #[error("keystore I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("home directory not found")]
    HomeNotFound,
}
