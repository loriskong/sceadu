//! Core types for the Sceadu Anonymous Agent Network.

pub mod agent;
pub mod discovery;
pub mod identity;
pub mod node;
pub mod session;
pub mod transport;

pub use agent::TaskHandle;
pub use discovery::{InviteTicket, NeighborPolicy};
pub use identity::keystore;
pub use identity::{
    AgentId, Capability, Identity, Manifest, ManifestBody, SecretSeed, MANIFEST_VERSION,
};
pub use node::{
    spawn, spawn_with_config, AgentConfig, DiscoveryConfig, Multiaddr, NodeConfig, NodeHandle,
    PeerId, SessionConfig, TransportConfig,
};
pub use session::{TaskEvent, TaskEventKind};
pub use transport::TorTransport;
pub use transport::{
    ensure_ipv6_listen_addr, ensure_ipv6_peer_addr, ConnectivityPolicy, EndpointRecord,
    NetworkStatus, Reachability, TransportKind, TransportStatus,
};

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
    #[error("identity error: {0}")]
    Identity(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("behaviour error: {0}")]
    Behaviour(String),
    #[error("listen error: {0}")]
    Listen(String),
    #[error("dial error: {0}")]
    Dial(String),
    #[error("request error: {0}")]
    Request(String),
    #[error("peer mismatch: expected {expected}, got {actual}")]
    PeerMismatch { expected: String, actual: String },
    #[error("DHT error: {0}")]
    Dht(String),
    #[error("invite error: {0}")]
    Invite(String),
    #[error("Tor plugin unavailable: {0}")]
    TorUnavailable(String),
    #[error("unsupported operation: {0}")]
    Unsupported(String),
    #[error("node task stopped")]
    Closed,
}

/// Result type used by the network layer.
pub type Result<T> = std::result::Result<T, Error>;
