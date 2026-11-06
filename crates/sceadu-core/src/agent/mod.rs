//! L4 agent protocol: message envelope and validation.

pub mod inbox;
pub mod message;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("agent message error: {0}")]
    Message(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub use inbox::{Inbox, StoredMessage};
pub use message::{
    new_context_id, new_message_id, AgentMessage, DataPart, FilePart, FileRef, Part, Role,
    TextPart, MAX_MESSAGE_BYTES, MESSAGE_PROTOCOL,
};

/// Maximum file size transferred by the Sceadu file stream.
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
