# Discovery Wire Format

This document covers the M2 discovery surface. Manifests, identities, and keystore
formats are defined by the `sceadu-core` code and its tests.

## DHT keys

Keys are raw 32-byte SHA-256 digests:

```text
manifest key   = SHA-256("aan/id/" + agent_id)
skill key      = SHA-256("aan/skill/" + lowercase(trim(skill)))
```

The manifest value is the UTF-8 JSON encoding of the signed `Manifest`.
Skill keys use libp2p Kademlia provider records; the record payload is the
provider's PeerID and is not a custom signed object. Callers must fetch the
provider's manifest and verify it before use.

## RPC protocol

Protocol ID: `/sceadu/message/1.0.0`. Frames are JSON-encoded request/response
enum values over a libp2p request-response stream.

Requests:

```json
{"Manifest": {"body": {}, "signature": "..."}}
{"Message": {"protocol": "sceadu.agent-message/0.1", "message_id": "...", "context_id": "...", "from": "...", "to": "...", "role": "user", "parts": [{"text": "hello"}], "created_at": 0}}
{"Neighbors": {"skill": "tennis", "max": 8, "nonce": 123}}
```

Responses:

```json
{"Manifest": {"body": {}, "signature": "..."}}
{"MessageAck": {"message_id": "..."}}
{"Neighbors": {"records": [], "nonce": 123, "policy": "open"}}
{"Error": {"message": "..."}}
```

`Message` is accepted only from an allowlisted AgentID whose identity matches the
authenticated PeerID. `Neighbors` records are
full signed manifests; `observed_at` is an advisory Unix timestamp, not a trust
signal. Responders clamp `max` to 1..=32 and exclude the requester from the
result. `open` returns the local verified peer cache; `stealth` returns none.

The client verifies every returned manifest, rejects expired or invalid
signatures, filters by requested skill, de-duplicates by AgentID, and caps
the result. Local source quarantine is not part of the wire format.

## Invite tickets

An invite is a signed bootstrap hint, not a membership credential.

```text
encoded = "aan-invite:" + base64url_nopad(json({ "body": ..., "signature": "..." }))
```

The body is a CBOR map with fields:

```text
version      integer, currently 1
issuer       AgentID string
seed         multiaddr string, normally including /p2p/<peer-id>
issued_at    Unix seconds
expires_at   Unix seconds
nonce        base64url random 16-byte value
```

`signature` is base64url Ed25519 over the deterministic CBOR encoding of the
body. Receivers verify the version, time window, and signature, then parse the
seed multiaddr. Invites are currently reusable until expiry; single-use tickets
and proof of work are future work.

## Source quarantine

Each node keeps in-memory per-introducer state. Three failed or poisoned neighbor
responses quarantine that introducer for 60 seconds. A clean response resets the
counter and clears the quarantine. Quarantine is local policy, never propagated.
