/// Errors returned by L0 identity and records.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid agent id")]
    InvalidAgentId,
    #[error("invalid signature encoding")]
    InvalidSignatureEncoding,
    #[error("signature verification failed")]
    InvalidSignature,
    #[error("unsupported biography version {0}")]
    UnsupportedVersion(u32),
    #[error("biography agent id does not match the signing identity")]
    AgentIdMismatch,
    #[error("invalid biography: {0}")]
    InvalidBiography(String),
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
    #[error("invite error: {0}")]
    Invite(String),
    #[error("Tor plugin unavailable: {0}")]
    TorUnavailable(String),
    #[error("unsupported operation: {0}")]
    Unsupported(String),
    #[error("node task stopped")]
    Closed,
}
