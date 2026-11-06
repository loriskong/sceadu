//! A2A-inspired agent message envelope.
//!
//! Sceadu transports these messages but does not interpret their business
//! content. The envelope is deliberately close to A2A's Message shape so a
//! future A2A bridge can map it without changing the transport.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Result;
use data_encoding::HEXLOWER;
use rand_core::{OsRng, RngCore};

/// Current Sceadu message envelope version.
pub const MESSAGE_PROTOCOL: &str = "sceadu.agent-message/0.1";

/// Maximum serialized message envelope size. Files travel separately.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// A text, structured-data, or file part.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Part {
    Text(TextPart),
    Data(DataPart),
    File(FilePart),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextPart {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DataPart {
    pub data: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FilePart {
    pub file: FileRef,
}

/// Metadata for a file transferred out of band.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRef {
    pub name: String,
    pub media_type: String,
    /// Lowercase hex SHA-256 of the file contents.
    pub sha256: String,
    pub size: u64,
}

/// Speaker role. Sceadu only validates the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Agent,
}

/// A single agent-to-agent message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentMessage {
    pub protocol: String,
    pub message_id: String,
    pub context_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    pub from: String,
    pub to: String,
    pub role: Role,
    pub parts: Vec<Part>,
    /// Unix seconds.
    pub created_at: u64,
}

/// Generate a random, transport-level message ID.
pub fn new_message_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    HEXLOWER.encode(&bytes)
}

/// Generate a random context ID for a new conversation.
pub fn new_context_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    HEXLOWER.encode(&bytes)
}

impl AgentMessage {
    pub fn new(
        message_id: impl Into<String>,
        context_id: impl Into<String>,
        from: impl Into<String>,
        to: impl Into<String>,
        role: Role,
        parts: Vec<Part>,
    ) -> Self {
        Self {
            protocol: MESSAGE_PROTOCOL.to_owned(),
            message_id: message_id.into(),
            context_id: context_id.into(),
            in_reply_to: None,
            from: from.into(),
            to: to.into(),
            role,
            parts,
            created_at: now_unix(),
        }
    }

    pub fn with_reply_to(mut self, message_id: impl Into<String>) -> Self {
        self.in_reply_to = Some(message_id.into());
        self
    }

    /// Validate only the transport envelope, never business semantics.
    pub fn validate(&self) -> Result<()> {
        if self.protocol != MESSAGE_PROTOCOL {
            return Err(super::Error::Message(format!(
                "unsupported message protocol: {}",
                self.protocol
            )));
        }
        validate_id("message_id", &self.message_id)?;
        validate_id("context_id", &self.context_id)?;
        if let Some(reply) = &self.in_reply_to {
            validate_id("in_reply_to", reply)?;
        }
        if self.from.is_empty() || self.to.is_empty() {
            return Err(super::Error::Message("from and to are required".to_owned()));
        }
        if self.parts.is_empty() {
            return Err(super::Error::Message("message has no parts".to_owned()));
        }
        for part in &self.parts {
            match part {
                Part::Text(text) => {
                    if text.text.is_empty() {
                        return Err(super::Error::Message("empty text part".to_owned()));
                    }
                }
                Part::Data(_) => {}
                Part::File(file) => {
                    if file.file.name.is_empty()
                        || file.file.media_type.is_empty()
                        || file.file.sha256.len() != 64
                        || !file
                            .file
                            .sha256
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit())
                    {
                        return Err(super::Error::Message(
                            "invalid file part metadata".to_owned(),
                        ));
                    }
                    if file.file.size == 0 || file.file.size > crate::agent::MAX_FILE_BYTES {
                        return Err(super::Error::Message(
                            "file size is outside the supported range".to_owned(),
                        ));
                    }
                }
            }
        }
        let encoded =
            serde_json::to_vec(self).map_err(|error| super::Error::Message(error.to_string()))?;
        if encoded.len() > MAX_MESSAGE_BYTES {
            return Err(super::Error::Message(format!(
                "message exceeds {MAX_MESSAGE_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

fn validate_id(field: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 128 {
        return Err(super::Error::Message(format!("invalid {field}")));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(super::Error::Message(format!("invalid {field}")));
    }
    Ok(())
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message() -> AgentMessage {
        AgentMessage::new(
            "msg-1",
            "ctx-1",
            "agent:a",
            "agent:b",
            Role::User,
            vec![Part::Text(TextPart {
                text: "hello".to_owned(),
            })],
        )
    }

    #[test]
    fn validates_a_text_message() {
        message().validate().unwrap();
    }

    #[test]
    fn rejects_unknown_protocol() {
        let mut msg = message();
        msg.protocol = "other/1".to_owned();
        assert!(msg.validate().is_err());
    }

    #[test]
    fn rejects_oversized_file_reference() {
        let mut msg = message();
        msg.parts = vec![Part::File(FilePart {
            file: FileRef {
                name: "big.bin".to_owned(),
                media_type: "application/octet-stream".to_owned(),
                sha256: "0".repeat(64),
                size: crate::agent::MAX_FILE_BYTES + 1,
            },
        })];
        assert!(msg.validate().is_err());
    }
}
