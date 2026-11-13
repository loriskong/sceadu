//! L0 identity, keys, keystore, and signed records.

mod error;
mod keys;
pub mod keystore;
mod records;

pub use error::Error;
pub use keys::{AgentId, Identity, SecretSeed};
pub use records::{
    Biography, BIOGRAPHY_PROTOCOL, MAX_BIOGRAPHY_DESCRIPTION, MAX_BIOGRAPHY_ENDPOINTS,
    MAX_BIOGRAPHY_TAGS, MAX_BIOGRAPHY_TAG_LEN,
};
