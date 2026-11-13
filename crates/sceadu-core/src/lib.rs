//! Core types for the Sceadu Anonymous Agent Network.

pub mod agent;
pub mod biography;
pub mod daemon;
pub mod discovery;
pub mod identity;
pub mod node;
pub mod session;
pub mod transport;

pub use agent::{AgentMessage, FilePart, FileRef, Inbox, Part, Role, StoredMessage, TextPart};
pub use discovery::InviteTicket;
pub use identity::keystore;
pub use identity::{AgentId, Biography, Identity, SecretSeed, BIOGRAPHY_PROTOCOL};
pub use node::{
    spawn, spawn_with_config, AgentConfig, Multiaddr, NodeConfig, NodeHandle, PeerId,
    SessionConfig, TransportConfig, DEFAULT_BOOTSTRAP,
};

pub use transport::TorTransport;
pub use transport::{
    ensure_ipv6_listen_addr, ensure_ipv6_peer_addr, ConnectivityPolicy, EndpointRecord,
    NetworkStatus, Reachability, TransportKind, TransportStatus,
};

/// Errors returned by the composition root and its layers.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("identity error: {0}")]
    Identity(#[from] identity::Error),
    #[error("identity error: {0}")]
    IdentityMessage(String),
    #[error("transport error: {0}")]
    Transport(#[from] transport::Error),
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
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("discovery error: {0}")]
    Discovery(#[from] discovery::Error),
    #[error("session error: {0}")]
    Session(#[from] session::Error),
    #[error("agent error: {0}")]
    Agent(#[from] agent::Error),
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
