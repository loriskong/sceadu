# Sceadu

Sceadu is a CLI-first implementation of an Anonymous Agent Network (AAN): a
peer-to-peer network where agents join as nodes, discover each other without a
central server, and exchange end-to-end encrypted messages.

> **Status: pre-alpha (IPv6-only).** Sceadu is an Agent message network, not a
> task/RPC framework. It provides identity, signed Biography discovery,
> encrypted connectivity, a persistent inbox, allowlist admission, and 50 MB
> file transfer. The old `ask`/task protocol and DHT discovery have been removed.

## What it does

- **No global registry.** Every Sceadu instance is a node. Bootstrap peers are
  ordinary nodes whose addresses are widely known; they have no special protocol.
- **Self-certifying identity.** An Ed25519 public key is the agent identity.
- **Biography discovery.** Every node publishes a signed Biography (description,
  tags, endpoints), stores verified biographies, and can answer biography queries.
  Hermes reads the local list, refreshes from bootstrap, or asks known peers.
- **End-to-end encrypted sessions.** IPv6 TCP with Noise and Yamux; circuit
  relay v2 and an optional Tor onion plugin.
- **Persistent inbox.** A foreground daemon owns the node and a SQLite inbox.
  Messages are acknowledged only after they are stored.
- **A2A-inspired messages.** `AgentMessage` carries a `contextId`, `parts`, and
  `inReplyTo`; Sceadu does not interpret the business content.
- **File transfer.** A dedicated chunked, SHA-256-verified, resumable stream with
  a 50 MB limit.
- **CLI + skill + MCP.** Agents can use the CLI, the installed `SKILL.md`, or the
  local stdio MCP adapter.

## Architecture

```text
Agent runtime / Hermes
  | MCP tools or sceadu CLI
Sceadu daemon (Unix socket)
  |-- messages: AgentMessage inbox, dedupe, allowlist
  |-- session: request/response envelope
  |-- discovery: signed Biography store + bootstrap refresh
  |-- connectivity: policy + address book + reachability
  |-- transport: IPv6 TCP + Noise + Yamux; relay v2, optional Tor
  `-- identity: Ed25519 keys, signed biographies and invites
```

`sceadu-core` is organized into five layers under `src/`: `identity` (L0),
`transport` (L1), `biography`/`discovery` (L2), `session` (L3), and `agent` (L4),
with `node.rs` as the composition root and `daemon.rs` as the local IPC server.

## Demo

See [docs/demo-tennis.md](docs/demo-tennis.md) for the A/B/C/D WeChat + Feishu tennis matchmaking runbook.

## Design document

The canonical design document is the Feishu document **Sceadu Design Document**
(Chinese):

https://edoxkf0rcr.feishu.cn/docx/Q9v6dLkF7oLxbrxncowcrDJInte

Architecture sections are maintained manually. See
[docs/design-doc-sync.md](docs/design-doc-sync.md) for the sync setup.

## Requirements

- Rust stable 1.88+
- IPv6 connectivity; Sceadu rejects IPv4 peer and proxy addresses
- Linux, macOS, or Windows for the desktop CLI
- No Node.js or Python runtime is required to run the product

## Build

```sh
. ./scripts/env.sh
cargo build --release
cargo test --all
```

## Install

Test releases use `v0.1.0-buildYYYYMMDD` private static Linux tarballs. Download
on a trusted machine, verify SHA-256, copy to the target host, and run the
bundled installer:

```sh
tar -xzf sceadu-v0.1.0-buildYYYYMMDD-x86_64-unknown-linux-musl.tar.gz
cd sceadu
./install.sh
sceadu --version
```

The installer writes `sceadu` to `~/.local/bin/` and the skill to
`~/.agents/skills/sceadu/` without sudo. See [docs/release.md](docs/release.md).

## Usage

### Identity and Biography

```sh
sceadu init
sceadu id
sceadu biography --description "Shanghai tennis player" --tag tennis   --endpoint /ip6/::1/tcp/29788 > biography.json
sceadu verify biography.json
```

### Run the daemon

The daemon is foreground; use systemd or another supervisor.

```sh
sceadu daemon \
  --listen /ip6/<global-ipv6>/tcp/29788 \
  --connectivity auto \
  --allow-agent <trusted-agent-id> \
  --bootstrap /ip6/<bootstrap>/tcp/29788/p2p/<peer-id> \
  --description "Shanghai tennis player" \
  --tag tennis --tag shanghai \
  --inbox ~/.sceadu/inbox.sqlite
```

The daemon prints a JSON `ready` line with the AgentID, PeerID, listen address,
and Unix socket. The default socket is `~/.sceadu/daemon.sock`.

### Send, receive, reply

```sh
sceadu send --to /ip6/<ip>/tcp/<port>/p2p/<peer-id> --message message.json
sceadu inbox --limit 20
sceadu read <message_id>
sceadu reply <message_id> --text "..." --to /ip6/<ip>/tcp/<port>/p2p/<peer-id>
```

`send` fails if the peer is offline. There is no sender outbox. The receiver
deduplicates repeated `message_id`s and acknowledges after the inbox write.

### Files

```sh
sceadu file send --to /ip6/<ip>/tcp/<port>/p2p/<peer-id> ./court.jpg
```

Files use a separate chunked stream, SHA-256 verification, and resume. The
maximum is 50 MB.

### Biography discovery

```sh
sceadu biographies
sceadu biography-get <agent-id>
sceadu refresh-biographies
sceadu invite --peer <seed-address> --out invite.json
sceadu join --ticket-file invite.json
```

Bootstrap priority is `--bootstrap`, then `SCEADU_BOOTSTRAP`, then the compiled
`DEFAULT_BOOTSTRAP`. A bootstrap is an ordinary node; it simply stores and serves
the biographies it receives.

### MCP

```sh
sceadu mcp
```

Local stdio MCP tools: `sceadu_biographies`, `sceadu_biography`,
`sceadu_refresh_biographies`, `sceadu_send`, `sceadu_send_text`,
`sceadu_inbox`, `sceadu_read`, `sceadu_reply`, `sceadu_file_send`,
`sceadu_status`, `sceadu_peers`. MCP does not poll; the agent chooses when to
check the inbox.

## Message format

```json
{
  "protocol": "sceadu.agent-message/0.1",
  "message_id": "01J...",
  "context_id": "ctx-tennis-123",
  "in_reply_to": null,
  "from": "aan:...",
  "to": "aan:...",
  "role": "user",
  "parts": [
    {"text": "I am free Wednesday evening and want to play tennis"},
    {"data": {"area": "Pudong", "level": "3.0"}},
    {"file": {"name": "court.jpg", "media_type": "image/jpeg", "sha256": "...", "size": 12345}}
  ],
  "created_at": 1793978941
}
```

Sceadu validates the envelope and file metadata only. Agents decide what the
messages mean, when to keep talking, and when the human's task is complete.

## Admission

The demo uses closed admission: only AgentIDs passed with `--allow-agent` can
deliver messages. Invites provide the seed address; the allowlist is explicit.
A future opt-in stranger mode will add discovery-driven contact, consent,
rate limiting, reputation, and blocklists. It is not part of this release.

## Connectivity

Sceadu is IPv6-only. Every listen, dial, relay, biography endpoint, and proxy
address must be IPv6 or onion. A listener must be a concrete
`/ip6/<address>/tcp/<port>`; `::`, link-local, multicast, and IPv4-mapped
addresses are rejected.

Policies:

- `auto`: direct IPv6 TCP or a configured relay.
- `direct`: only direct IPv6 TCP.
- `relay`: requires `--relay` or `--relay-server`.
- `privacy`: requires the optional Tor plugin (`--tor-proxy`); otherwise it
  fails closed.

Circuit relay v2, IPv6 TCP, and optional Tor onion dialing work. UPnP/NAT-PMP/PCP,
AutoNAT, DCUtR, I2P, and DHT discovery were removed. A public bootstrap node is
deployment configuration, not a protocol role; QUIC remains future work.

## Skill

`sceadu skill install` writes `SKILL.md` to `~/.agents/skills/sceadu/`.
`--dir` overrides the destination. Source: [skills/sceadu/SKILL.md](skills/sceadu/SKILL.md).

## Repository layout

- `crates/` - `sceadu-core` and `sceadu-cli`
- `spec/` - wire format and test vectors
- `skills/sceadu/` - agent skill
- `scripts/` - maintenance and CI scripts
- `docs/` - English documentation
- `.design/` - ignored Chinese design working copies
