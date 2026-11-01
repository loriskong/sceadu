# Task Protocol

Protocol ID: `/sceadu/rpc/1.0.0`. Task messages are JSON request/response frames on
the existing libp2p request-response stream. The session is already authenticated
by Noise and bound to the peer's verified manifest; task frames do not carry a
separate signature.

## Flow

```text
client                                   server
  | TaskStart(task_id, capability, input)  |
  |--------------------------------------->|
  | TaskAccepted(task_id)                  |
  |<---------------------------------------|
  | TaskReady(task_id)                     |
  |--------------------------------------->|
  | TaskReadyAck(task_id)                  |
  |<---------------------------------------|
  | TaskEventPush(seq=1, started)          |
  |<---------------------------------------|
  | TaskEventAck(task_id, seq=1)           |
  |--------------------------------------->|
  | ... progress / result ...              |
  | TaskEventPush(seq=n, done)             |
  |<---------------------------------------|
```

The client registers the task locally before sending `TaskReady`, so events
cannot race ahead of the event channel. The server sends `failed` followed by
`done` for execution failures. A successful task sends `done` as the final event.

## Fields

`TaskStart`:

```text
task_id      opaque string, 1..=128 bytes
capability   advertised capability ID
input        JSON value, at most 64 KiB serialized
deadline_at  Unix seconds
```

The server requires a completed manifest exchange, rejects unknown capabilities,
expired deadlines, duplicate active task IDs, and oversized input.

`TaskEventPush`:

```text
task_id   task identifier
sequence  monotonically increasing, starting at 1
event     started | progress | result | failed | done
```

`started` carries the capability. `progress` carries a message and optional
fraction. `result` carries a JSON output value. `failed` carries a message.
`done` is terminal.

## Limits and policy

- One client may have multiple concurrent tasks; events are routed by `task_id`.
- Events with a non-increasing sequence are acknowledged and ignored.
- Task handles are local; dropping a handle does not cancel the remote task.
- The MCP bridge is not part of this protocol and is intentionally deferred.
