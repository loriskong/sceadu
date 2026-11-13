# Biography Wire Format

Sceadu has no DHT discovery and no `discover` command. Every node exchanges
signed Biographies and stores the verified records in a local SQLite table.

## Biography

```json
{
  "protocol": "sceadu.biography/0.1",
  "agent_id": "aan:...",
  "public_key": "base64-ed25519-public-key",
  "endpoints": ["/ip6/2001:db8::1/tcp/29788/p2p/12D3KooW..."],
  "description": "Shanghai tennis player, level 3.0",
  "tags": ["tennis", "shanghai", "pudong"],
  "updated_at": 1790000000,
  "signature": "base64-ed25519-signature"
}
```

The signature covers a deterministic CBOR encoding of all fields except
`signature`. The public key must derive the `agent_id`; verifiers must reject a
mismatch, an invalid signature, more than 8 endpoints, more than 16 tags, a tag
longer than 32 characters, or a description longer than 1024 bytes.

`endpoints` are IPv6 multiaddrs and should include `/p2p/<peer-id>`; they may be
empty for relay-only nodes. A receiver must verify the biography before trusting
its description, tags, or endpoints.

## Protocol

Protocol ID: `/sceadu/biography/1.0.0`. Frames are JSON request/response enum
values over a libp2p request-response stream.

Requests:

```json
{"Biography": {"protocol": "sceadu.biography/0.1", "agent_id": "...", "...": "..."}}
{"Biographies": {"limit": 1000}}
{"Message": {"protocol": "sceadu.agent-message/0.1", "...": "..."}}
```

Responses:

```json
{"BiographyAck": null}
{"Biographies": {"biographies": []}}
{"MessageAck": {"message_id": "..."}}
{"Error": {"message": "..."}}
```

`Biography` publishes or refreshes the sender's Biography. Receivers verify it,
store it, and acknowledge. `Biographies` returns verified local records; every
node, including a bootstrap, implements the same behavior.

## Bootstrap and refresh

Every node publishes at startup and every five minutes. Bootstrap priority:

```text
--bootstrap  >  SCEADU_BOOTSTRAP  >  DEFAULT_BOOTSTRAP
```

A bootstrap is an ordinary node with a widely known address. It has no special
protocol, role flag, or privilege. The node's local store is the source for
`sceadu_biographies`; `sceadu_refresh_biographies` performs an explicit
`Biographies` request to the first configured bootstrap and merges the verified
results.

There is no wire-level search API. Hermes filters the returned Biographies by
description and tags and decides whom to contact.

## Invite tickets

Invite tickets remain a signed seed hint, not a membership credential. The format
is unchanged and is documented in the code and tests.

## Source quarantine

The old DHT/neighbour quarantine is removed. A node records local failure counts
for observability only; it does not propagate them.
