//! L3 session: request/response and task wire protocol.

use crate::identity::{ContactRecord, Manifest};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One event in a remote task stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEventKind {
    Started {
        capability: String,
    },
    Progress {
        message: String,
        fraction: Option<f64>,
    },
    Result {
        output: Value,
    },
    Failed {
        message: String,
    },
    Done,
}

/// A sequenced task event sent by a remote agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEvent {
    pub task_id: String,
    pub sequence: u64,
    pub event: TaskEventKind,
}

pub(crate) const PROTOCOL: &str = "/sceadu/rpc/1.0.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum RpcRequest {
    Manifest(Box<Manifest>),
    Echo {
        message: String,
    },
    Neighbors {
        capability: Option<String>,
        max: u32,
        nonce: u64,
    },
    TaskStart {
        task_id: String,
        capability: String,
        input: Value,
        deadline_at: u64,
    },
    TaskReady {
        task_id: String,
    },
    TaskEventPush {
        task_id: String,
        sequence: u64,
        event: TaskEventKind,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum RpcResponse {
    Manifest(Box<Manifest>),
    Echo {
        message: String,
    },
    Neighbors {
        records: Vec<ContactRecord>,
        nonce: u64,
        policy: String,
    },
    TaskAccepted {
        task_id: String,
    },
    TaskReadyAck {
        task_id: String,
    },
    TaskEventAck {
        task_id: String,
        sequence: u64,
    },
    Error {
        message: String,
    },
}
