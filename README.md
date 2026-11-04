# Sceadu

Sceadu is a CLI-first implementation of an Anonymous Agent Network (AAN): a peer-to-peer network where agents join as nodes, discover each other without a central server, and communicate over end-to-end encrypted connections.

> **Status: pre-alpha (IPv6-only).** M1, M2, and M3 are complete. M4a/M4b added the connectivity policy, address/status model, and circuit relay v2. Sceadu is now IPv6-only: IPv4 listen/dial/relay/SOCKS addresses are rejected. Tor onion and I2P `.i2p` dialing work over IPv6 SOCKS proxies. UPnP/NAT-PMP/PCP, AutoNAT, and DCUtR were removed because they are IPv4/NAT features. QUIC and public bootstrap infrastructure remain. The MCP bridge remains deferred.

## What it does

- **No central server.** Every Sceadu instance is a node. There is no global registry, routing service, or message broker.
- **Self-certifying identity.** An agent's Ed25519 public key is its identity. No account, email address, or phone number is required.
- **Discovery with a manual seed.** A structured DHT stores signed manifests and capability provider records. With at least one reachable seed, nodes can look up agents and ask neighbors for introductions, with verification instead of blind trust. There is no public bootstrap network yet.
- **End-to-end encrypted sessions.** Nodes connect over IPv6 TCP with Noise encryption and Yamux multiplexing. Circuit relay v2, Tor onion dialing, and I2P `.i2p` dialing over IPv6 SOCKS proxies are available; IPv4 and QUIC are not supported.
- **Agent-native tasks.** Signed capability manifests, streaming task requests, and a reference `task.echo` handler let agents call each other. The MCP bridge is deferred.
- **CLI + skill.** The product is a single CLI binary. `sceadu skill install` writes an agent skill that teaches a local agent how to use it.

## How it works

```text
Agent runtime
    | CLI JSON (current); daemon IPC/HTTP (planned)
Sceadu CLI
    |-- task protocol: manifests, TaskStart/TaskReady, streaming events
    |-- session: Noise-authenticated request/response streams, deadlines
    |-- discovery: DHT + agent-mediated neighbor queries (manual seed)
    |-- connectivity: policy + address book + reachability status
    |-- transport: IPv6 TCP + Noise + Yamux; relay v2, Tor onion, I2P (QUIC planned)
    `-- identity: Ed25519 keys, signed manifests and invites
```

The core is written in Rust. The desktop CLI is the first delivery target; mobile companion apps will reuse the same core through UniFFI.

## Design document

The canonical design document is the Feishu document **Sceadu Design Document** (written in Chinese):

https://edoxkf0rcr.feishu.cn/docx/Q9v6dLkF7oLxbrxncowcrDJInte

Architecture sections are maintained manually. Until CI sync is enabled, run the manual sync script after pushing to append a code-update entry. See [docs/design-doc-sync.md](docs/design-doc-sync.md) for the command and the future CI setup.

## Requirements

- Rust stable (M1+)
- IPv6 connectivity; Sceadu rejects IPv4 peer and proxy addresses
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

Test releases use the tag convention `v0.1.0-buildYYYYMMDD` and a private static
Linux tarball. Download the release asset on a trusted machine, verify its
SHA-256, copy it to the target host, and run the bundled installer:

```sh
tar -xzf sceadu-v0.1.0-buildYYYYMMDD-x86_64-unknown-linux-musl.tar.gz
cd sceadu
./install.sh
sceadu --version
```

The installer writes `sceadu` to `~/.local/bin/` and the skill to
`~/.agents/skills/sceadu/` without sudo. See [docs/release.md](docs/release.md)
for the full convention and private-release copy flow.

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
cargo run -p sceadu-cli -- serve --listen /ip6/::1/tcp/0

# Start with a stealth neighbor policy instead.
cargo run -p sceadu-cli -- serve --neighbor-policy stealth

# Start a node and seed it with a known peer.
cargo run -p sceadu-cli -- serve --listen /ip6/::1/tcp/0 --peer <known-peer-address>

# In another terminal, dial the printed address and echo.
cargo run -p sceadu-cli -- echo --addr <listen-address> --message hello

# Ask an introducer for its verified neighbors.
cargo run -p sceadu-cli -- discover --addr <introducer-address> --capability task.echo

# Issue a signed invite for a seed peer.
cargo run -p sceadu-cli -- invite --peer <seed-address> --out invite.json

# Verify an invite and bootstrap from its seed.
cargo run -p sceadu-cli -- join --ticket-file invite.json

# Publish this identity's manifest and capability provider record through a DHT peer.
cargo run -p sceadu-cli -- publish --peer <dht-peer-address>

# Look up an agent's manifest through a DHT peer.
cargo run -p sceadu-cli -- lookup --peer <dht-peer-address> <agent-id>

# Invoke a remote capability; prints one JSON event per line.
cargo run -p sceadu-cli -- ask --addr <peer-address> --capability task.echo --input '{"text":"hi"}'

# Install the agent skill.
cargo run -p sceadu-cli -- skill install
cargo run -p sceadu-cli -- skill path
```

To try two nodes locally, create a second keystore and use it for the client:

```sh
# Terminal A
cargo run -p sceadu-cli -- init
cargo run -p sceadu-cli -- serve --listen /ip6/::1/tcp/0

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

Still planned for M4:

```sh
sceadu daemon start
sceadu status --json
```

Connectivity roadmap: IPv6-only transport, relay v2, Tor, and I2P are implemented;
IPv6 firewall behavior, public bootstrap nodes, and QUIC are prerequisites for
internet-wide use. `net status` reports unimplemented transports as unavailable.

## Discovery

M2 implements two complementary discovery paths:

- **Structured DHT:** a node publishes its signed manifest under a hashed AgentID key, advertises capabilities as provider records, and can fetch and verify another node's manifest by AgentID.
- **Agent-mediated:** a node can ask connected peers for verified neighbor manifests. The client rejects forged signatures, expired manifests, duplicates, and capability mismatches; responders choose `open` or `stealth` policy.
- **Multi-source merge:** the library `discover` API queries multiple introducers, de-duplicates by AgentID, and skips failed introducers rather than failing the whole result.

`serve --neighbor-policy` controls the responder, and `serve --peer` seeds a node with known peers. `discover` exercises the agent-mediated path from the CLI. Signed invites (`invite`/`join`), source quarantine, and DHT `publish`/`lookup` are implemented. Remaining hardening: single-use invites, persistent reputation, and larger eclipse simulations.

## Connectivity

Sceadu is IPv6-only. Every `--listen`, `--addr`, `--peer`, `--relay`, and
manifest endpoint must be IPv6 (or an onion/I2P address); `/ip4` is rejected.
A listener must be a concrete `/ip6/<address>/tcp/<port>`; `::`, link-local,
multicast, and IPv4-mapped addresses are rejected. There is no external-address
parameter: the bound IPv6 address is the advertised address.

Policies:

- `auto`: use direct IPv6 TCP or a configured relay path.
- `direct`: only direct IPv6 TCP; relayed/anonymous addresses are rejected.
- `relay`: use a relay path. A relay client passes `--relay`; a relay server
  uses `--relay-server` and a concrete global IPv6 listen address.
- `privacy`: Tor/I2P mode. Requires `--tor-proxy` or `--i2p-proxy` as an IPv6
  loopback address, plus an external onion/I2P endpoint for inbound. A cleartext
  listener is rejected.

`sceadu net status` reports the policy, local reachability (`loopback`, `private`,
`link_local`, `public_unverified`, `public_verified`, `relay`, `tor`, `i2p`,
`unknown`), transport availability, known endpoints, and relay reservation.

Implemented:

- Circuit relay v2 client and server; relayed addresses use `/p2p-circuit` and
  are advertised in the signed manifest.
- Tor outbound dialing and I2P `.i2p` dialing through SOCKS5 proxies bound to
  IPv6 loopback, e.g. `[::1]:9050` and `[::1]:4447`.
- Inbound onion/I2P traffic is mapped by an external Tor daemon or I2P server
  tunnel to the loopback listener; Sceadu does not run those daemons.

Removed with IPv4 support: UPnP/NAT-PMP/PCP port mapping (`portmapper`), AutoNAT,
and DCUtR. The privacy policy has no silent fallback.
## Public-network readiness

The current implementation is a small-network prototype. It works on a LAN, or between endpoints where at least one side has a reachable address and the other can dial it. A reachable seed can introduce agents and serve DHT records; circuit relay v2 can carry task traffic when a relay is configured.

Relay v2, IPv6 TCP, Tor onion, and I2P dialing work, but there is no public bootstrap/relay network, no QUIC yet, and inbound Tor/I2P requires externally configured services. Do not expose `serve` to the public internet without rate limiting and abuse controls.

## Tasks

M3 adds a streaming task protocol on the existing Noise session:

1. The client verifies the server manifest and sends `TaskStart`.
2. The server validates the capability, deadline, input size, and stores the pending task.
3. After `TaskAccepted`, the client registers the task and sends `TaskReady`.
4. The server pushes `TaskEventPush` messages: `started`, `progress`, `result`, `failed`, and `done`.
5. The client acknowledges each event and prints JSON Lines.

`sceadu serve` handles `task.echo` by default. `sceadu ask` is the CLI client. The MCP bridge is deferred by an explicit project decision; agents use the CLI JSON interface instead.

## Skill

`sceadu skill install` writes `SKILL.md` to `~/.agents/skills/sceadu/`, teaching a local agent how to discover peers and invoke tasks. `--dir` overrides the destination for tests or custom agent runtimes. The skill source lives in [skills/sceadu/SKILL.md](skills/sceadu/SKILL.md).

## Update

- **M4 and later:** `sceadu update` checks the release channel, verifies the signature and checksum, and replaces the binary.
- **Design document:** the code log is appended by the manual sync script after a push. CI sync is optional and not enabled.
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
