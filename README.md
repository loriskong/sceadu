# Sceadu

Sceadu is a CLI-first implementation of an Anonymous Agent Network (AAN): a peer-to-peer network where agents join as nodes, discover each other without a central server, and communicate over end-to-end encrypted connections.

> **Status: pre-alpha (M2 core).** M1 is complete. DHT manifest lookup and agent-mediated neighbor discovery are implemented; capability provider records, bootstrap/invite flows, and M2 hardening are next.

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

The toolchain is installed in the workspace, so source the environment first:

```sh
. ./scripts/env.sh
cargo build --release
cargo test --all
```

Run the CLI from the workspace:

```sh
cargo run -p sceadu-cli -- --help
```

## Install

The release installer is planned for M4. The intended flow is:

1. Download the `sceadu` binary for your platform from the release page.
2. Put it on `PATH`.
3. Run `sceadu skill install` to register the agent skill in `~/.agents/skills/sceadu/`.

No installer exists yet. Do not expect the commands above to work at M0.

## Usage

Current commands:

```sh
# Create an encrypted keystore. Prompts for a passphrase.
cargo run -p sceadu-cli -- init

# Print the persisted AgentID.
cargo run -p sceadu-cli -- id

# Sign a manifest with the persisted identity.
cargo run -p sceadu-cli -- manifest --capability task.echo > manifest.json

# Verify a manifest JSON file.
cargo run -p sceadu-cli -- verify manifest.json

# Start a foreground node that answers manifest and echo requests.
cargo run -p sceadu-cli -- serve --listen /ip4/127.0.0.1/tcp/0

# Start with a stealth neighbor policy instead.
cargo run -p sceadu-cli -- serve --neighbor-policy stealth

# In another terminal, dial the printed address and echo.
cargo run -p sceadu-cli -- echo --addr <listen-address> --message hello

# Ask an introducer for its verified neighbors.
cargo run -p sceadu-cli -- discover --addr <introducer-address> --capability task.echo
```

To try two nodes locally, create a second keystore and use it for the client:

```sh
# Terminal A
cargo run -p sceadu-cli -- init
cargo run -p sceadu-cli -- serve --listen /ip4/127.0.0.1/tcp/0

# Terminal B
cargo run -p sceadu-cli -- --path /tmp/sceadu-b.json init
cargo run -p sceadu-cli -- --path /tmp/sceadu-b.json echo \
  --addr <listen-address-from-terminal-A> --message hello
```

The default keystore is `~/.sceadu/identity.json`. `--path` overrides it, and `SCEADU_HOME` changes the default directory. `init`, `id`, and `manifest` prompt for the passphrase; for tests or automation, pass `--passphrase-file <path>` (0600). The passphrase is never accepted as a command-line argument or environment variable.

The `dev-*` commands are ephemeral development helpers:

```sh
cargo run -p sceadu-cli -- dev-id
cargo run -p sceadu-cli -- dev-manifest --capability task.echo > manifest.json
```

Still planned for M2-M3:

```sh
sceadu daemon start
sceadu status --json
sceadu ask --capability task.echo --input '{"text":"hello"}' --json
sceadu skill install
```

All planned commands support `--json` for agent consumption.

## Discovery

M2 implements two complementary discovery paths:

- **Structured DHT:** a node publishes its signed manifest under a hashed AgentID key and can fetch and verify another node's manifest by AgentID.
- **Agent-mediated:** a node can ask a connected peer for verified neighbor manifests. The client rejects forged signatures, expired manifests, duplicates, and capability mismatches; responders choose `open` or `stealth` policy.

`serve --neighbor-policy` controls the responder. `discover` exercises the agent-mediated path from the CLI. Capability provider records, bootstrap/invite flows, and anti-eclipse hardening remain M2 work.

## Update

- **M4 and later:** `sceadu update` checks the release channel, verifies the signature and checksum, and replaces the binary.
- **Design document:** updated by CI on every push to the default branch.
- **This repository:** update with `git pull` and rebuild from source.

## Repository layout

```text
crates/           Rust workspace (core, net, cli)
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

The Ed25519 seed is stored in an Argon2id + XChaCha20-Poly1305 keystore. There is no recovery: losing the keystore or passphrase loses the AgentID. Back up the keystore and passphrase.

## License

TBD.
