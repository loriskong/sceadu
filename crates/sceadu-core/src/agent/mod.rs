//! L4 agent protocol: task orchestration and public task handle.

use tokio::sync::mpsc;

use crate::session::TaskEvent;

/// A handle to an in-flight task. Events arrive in sequence order.
pub struct TaskHandle {
    pub task_id: String,
    events: mpsc::Receiver<TaskEvent>,
}

impl TaskHandle {
    pub(crate) fn from_receiver(task_id: String, events: mpsc::Receiver<TaskEvent>) -> Self {
        Self { task_id, events }
    }

    pub async fn next(&mut self) -> Option<TaskEvent> {
        self.events.recv().await
    }
}
