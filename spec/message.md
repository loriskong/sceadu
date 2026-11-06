# Agent Message Wire Format

Sceadu transports A2A-inspired messages. It does not define tasks,
capabilities, acceptance criteria, or business semantics.

## Protocol IDs

```text
request/response: /sceadu/message/1.0.0
file stream:      /sceadu/file/1.0.0
```

The request/response protocol is JSON over a libp2p request-response stream.
The file protocol is a raw libp2p stream with length-prefixed frames.

## Envelope

```json
{
  "protocol": "sceadu.agent-message/0.1",
  "message_id": "lowercase-hex",
  "context_id": "conversation-id",
  "in_reply_to": null,
  "from": "aan:...",
  "to": "aan:...",
  "role": "user",
  "parts": [
    {"text": "..."},
    {"data": {}},
    {"file": {"name": "...", "media_type": "...", "sha256": "...", "size": 1}}
  ],
  "created_at": 1793978941
}
```

Validation:

- `protocol` must be `sceadu.agent-message/0.1`.
- IDs are 1..=128 ASCII alphanumeric or `-_.:` characters.
- `from` and `to` are non-empty AgentID strings.
- At least one part is present.
- Text parts are non-empty.
- File parts carry a 64-hex SHA-256, a non-empty media type, and a size in
  `1..=50 MiB`.
- The serialized envelope is at most 1 MiB.

`from` must match the authenticated PeerID of the sender. `to` must be the local
AgentID. The receiver rejects unknown senders unless `allowed_agents` is empty
(tests) or the sender is in the allowlist.

## Delivery

1. Sender writes a JSON message file.
2. `sceadu send` asks the local daemon to dial the target.
3. The target verifies the envelope and sender.
4. The target inserts the message into SQLite by `message_id` (dedupe) and
   returns `MessageAck`.
5. The sender reports success only after the ACK.

There is no sender outbox. If the target is offline or rejects the message, the
command fails.

## Inbox

SQLite table `messages`:

```text
message_id   primary key
context_id
from_agent
to_agent
role
received_at
read
payload      full JSON envelope
```

The inbox is receiver-side and persistent. `read` marks a message read. Files are
stored separately in the content-addressed file store.

## File stream

Frames are `u32` big-endian length plus payload.

```text
header JSON  {name, media_type, size, sha256, chunk_size}
ack JSON     {offset, done, error}
chunk        raw bytes (about 48 KiB)
end          0-length frame
final ack    {offset, done: true}
```

The receiver writes `<root>/<sha256>.partial`, resumes from the partial length,
verifies SHA-256, and renames to `<root>/<sha256>`. The 50 MiB limit is enforced
on both sides.
