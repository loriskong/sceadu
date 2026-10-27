# Sceadu

Sceadu is a CLI-first implementation of an Anonymous Agent Network (AAN): a peer-to-peer network where agents join as nodes, discover each other without a central server, and communicate over end-to-end encrypted connections.

> **Status: pre-alpha (M0).** The architecture is defined; the CLI and wire protocol are being implemented. The commands below describe the intended interface and may change.

## What it does

- **No central server.** Every Sceadu instance is a node. There is no global registry, routing service, or message broker.
- **Self-certifying identity.** An agent's Ed25519 public key is its identity. No account, email address, or phone number is required.
- **Discovery without a global index.** A structured DHT handles capability lookup and cold start. Agent-mediated discovery lets nodes ask neighbors for introductions, with verification instead of blind trust.
- **End-to-end encrypted sessions.** Nodes connect over Noise, with relay fallback when direct connections are not possible. Relays forward ciphertext only.
- **Agent-native tasks.** Signed capability manifests, task requests, streaming events, and an MCP bridge let agents call each other's tools.
- **CLI + skill.** The product is a single CLI binary. `sceadu skill install` writes an agent skill that teaches a local agent how to use it.

## How it works

```text
Agent runtime
    | local IPC / HTTP
Sceadu daemon
    |-- agent protocol: manifests, tasks, MCP bridge
    |-- session: envelopes, streams, deadlines
    |-- discovery: DHT + agent-mediated neighbor queries
    |-- transport: rust-libp2p, Noise, QUIC/TCP, relay
    `-- identity: Ed25519 keys, signed records
```

The core is written in Rust. The desktop CLI is the first delivery target; mobile companion apps will reuse the same core through UniFFI.

## Design document

The canonical design document is the Feishu document **Sceadu Design Document** (written in Chinese):

https://edoxkf0rcr.feishu.cn/docx/Q9v6dLkF7oLxbrxncowcrDJInte

Architecture sections are maintained manually. Until CI sync is enabled, run the manual sync script after pushing to append a code-update entry. See [docs/design-doc-sync.md](docs/design-doc-sync.md) for the command and the future CI setup.

## Requirements

- Rust stable (M1+)
- Linux, macOS, or Windows for the desktop CLI
- No Node.js or Python runtime is required to run the product

## Build

The Cargo workspace is added in M1. The toolchain is installed in the workspace, so source the environment first:

```sh
. ./scripts/env.sh
cargo build --release
cargo test --all
```

## Install

The release installer is planned for M4. The intended flow is:

1. Download the `sceadu` binary for your platform from the release page.
2. Put it on `PATH`.
3. Run `sceadu skill install` to register the agent skill in `~/.agents/skills/sceadu/`.

No installer exists yet. Do not expect the commands above to work at M0.

## Usage

The following commands are the planned CLI contract for M1-M3:

```sh
# Create or load the local identity.
sceadu init

# Start the background node.
sceadu daemon start

# Show node status as JSON.
sceadu status --json

# Find agents that advertise a capability.
sceadu discover --capability task.echo --json

# Send a task to a discovered agent.
sceadu ask --capability task.echo --input '{"text":"hello"}' --json

# Install the agent skill.
sceadu skill install
```

All commands are designed to support `--json` for agent consumption.

## Update

- **M4 and later:** `sceadu update` checks the release channel, verifies the signature and checksum, and replaces the binary.
- **Design document:** updated by CI on every push to the default branch.
- **This repository:** update with `git pull` and rebuild from source.

## Repository layout

```text
crates/           Rust workspace (M1+)
spec/             wire format and test vectors
skills/sceadu/    agent skill files
scripts/          maintenance and CI scripts
docs/             English documentation
.design/          ignored Chinese design working copies
```

## Contributing

Read [AGENTS.md](AGENTS.md) before changing code. Use Conventional Commits and keep repository content in English. Architecture changes also require an update to the Feishu design document.

Sceadu is developed under the `loriskong` GitHub account (`imloriskong@gmail.com`). See [docs/github-accounts.md](docs/github-accounts.md) for SSH setup and the account-switch helper.

## Security

Sceadu handles identity keys and untrusted network input. Do not log private keys, session keys, or message plaintext. Report vulnerabilities privately; do not open a public issue with exploit details.

## License

TBD.
