//! L3 session: request/response wire protocol.

pub mod file;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("request error: {0}")]
    Request(String),
    #[error("session closed")]
    Closed,
}

pub type Result<T> = std::result::Result<T, Error>;

use crate::agent::AgentMessage;
use crate::identity::Biography;
use serde::{Deserialize, Serialize};

pub(crate) const PROTOCOL: &str = "/sceadu/biography/1.0.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum RpcRequest {
    /// Publish or refresh a signed biography.
    Biography(Box<Biography>),
    /// Return biographies known by the receiver.
    Biographies {
        limit: u32,
    },
    Message(Box<AgentMessage>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum RpcResponse {
    BiographyAck,
    Biographies { biographies: Vec<Biography> },
    MessageAck { message_id: String },
    Error { message: String },
}
