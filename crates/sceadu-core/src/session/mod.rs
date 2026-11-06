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
use crate::identity::{ContactRecord, Manifest};
use serde::{Deserialize, Serialize};

pub(crate) const PROTOCOL: &str = "/sceadu/message/1.0.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum RpcRequest {
    Manifest(Box<Manifest>),
    Neighbors {
        skill: Option<String>,
        max: u32,
        nonce: u64,
    },
    Message(Box<AgentMessage>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum RpcResponse {
    Manifest(Box<Manifest>),
    Neighbors {
        records: Vec<ContactRecord>,
        nonce: u64,
        policy: String,
    },
    MessageAck {
        message_id: String,
    },
    Error {
        message: String,
    },
}
