# Sceadu Agent Instructions

This file is loaded by DSH-compatible agents as project context. Keep it accurate and in English.

## Project

Sceadu is a CLI-first reference implementation of an Anonymous Agent Network (AAN). Agents join as nodes without a central server, exchange signed Biographies and agent referrals for discovery, and communicate over end-to-end encrypted peer-to-peer sessions. The product is a CLI distributed as a skill for locally installed agents.

Current stage: pre-alpha (IPv6-only) Agent message network. The task/RPC protocol is removed. A local MCP stdio adapter and a persistent daemon inbox exist. The MVP is defined in section 4.3 of the design document.

## Sources of truth

- Design intent: the Feishu document "Sceadu Design Document" (Chinese): https://edoxkf0rcr.feishu.cn/docx/Q9v6dLkF7oLxbrxncowcrDJInte
- Code and implementation details: this repository.
- If they conflict, the design document wins for intent. Record the change in the document and update this repository.

## Language policy

- All repository content is English: code, comments, identifiers, commit messages, README, docs, tests, CI, and issue/PR text.
- The Feishu design document is Chinese. Do not commit Chinese design documents or generated exports to this repository.
- Local working copies of the design document may live in `.design/`, which is git-ignored.
- CLI output is English by default; localization is a later feature.

## Locked MVP decisions

- Rust core and desktop CLI first (Linux, macOS, Windows); mobile companion apps come after MVP.
- Discovery is signed Biography exchange. Every node stores verified Biographies,
  publishes its own to every known neighbor, including bootstrap, and serves biography queries.
  There is no DHT, no `discover` command, and no special bootstrap protocol role.
- No Sybil protection in alpha: invite-only / small networks. Do not add tokens, staking, or PoW silently.
- Anonymity scope: content confidentiality and pseudonymity by default. Tor is an optional transport that can reduce network-level IP exposure; do not claim full anonymity or traffic-analysis resistance.
- Sceadu peer transport is IPv6-only. IPv4 is rejected in listen, dial, relay, biography endpoint, invite seed, and Tor SOCKS proxy addresses. A listener must be a concrete `/ip6/<address>/tcp/<port>`; `::`, link-local, multicast, and IPv4-mapped addresses are rejected. There is no external-address parameter; the bound IPv6 address is advertised.
- Connectivity policies: `auto`, `direct`, `relay`, `privacy`. `privacy` never silently falls back to clearnet. `relay` requires `--relay` or `--relay-server`; `direct` rejects `/p2p-circuit` and `/onion3` addresses. Tor is an optional IPv6 SOCKS5 plugin and requires an externally configured service.
- The transport carries A2A-inspired `AgentMessage` envelopes. Sceadu does not define tasks, capabilities, acceptance criteria, or completion. Agents decide when a conversation is done.
- A signed `Biography` is the discovery record: protocol, AgentID, public key, endpoints, description, tags, updated_at, signature. Nodes publish it at startup and every five minutes, and store verified biographies locally.
- Bootstrap peers are ordinary nodes listed in configuration (`--bootstrap`, `SCEADU_BOOTSTRAP`, or `DEFAULT_BOOTSTRAP`). They have no special code path or privileges.
- The daemon is foreground and owns the node and SQLite inbox. CLI/MCP clients use a Unix socket. There is no sender outbox; a send to an offline peer fails.
- Demo admission is closed: `--allow-agent` is required. A future opt-in stranger mode will add discovery-driven contact, consent, rate limiting, reputation, and blocklists; do not implement it silently.
- File transfer is a separate chunked, SHA-256-verified, resumable stream with a 50 MiB limit. Files are not base64-encoded into messages.
- UPnP/NAT-PMP/PCP port mapping, AutoNAT, and DCUtR are removed. Circuit relay v2 is retained for firewall-restricted or non-inbound peers.
- Out of MVP: mobile apps, mixnet, offline messaging, groups, payments, GUI, key recovery.

Any change to these requires an explicit decision in the design document.

## Workflow

1. Read the relevant design-document sections before changing architecture.
2. Keep changes small and testable. Add or update tests with behavior changes.
3. Run `python3 scripts/check-language.py`, then `. ./scripts/env.sh` and run `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --all`.
4. Update README and English docs when usage or behavior changes.
5. Use Conventional Commits.
6. Push to the default branch. Run the manual design-document sync after the push until CI sync is enabled.
7. For architecture changes, update the design document manually in the same change set. CI only appends a log; it never rewrites design sections.

## GitHub identity

Sceadu is developed under the `loriskong` GitHub account (`imloriskong@gmail.com`). The `valaxkong` account is reserved for other projects on the same machine.

- Switch identities with `. ./scripts/github-account.sh` and `github_use imloriskong`.
- Never commit credentials, SSH private keys, or tokens.
- Before committing, confirm `git config user.email` is `imloriskong@gmail.com`.
- The first local history was rebuilt so published commits are attributed only to `imloriskong`.

## Release convention

- Cargo version: `0.1.0`.
- Tag format: `v0.1.0-buildYYYYMMDD`; at most one build tag per date.
- Releases are private pre-releases. Build assets are downloaded on a trusted machine and copied to test hosts with `scp`.
- Assets are static `x86_64-unknown-linux-musl` tarballs with `sceadu`, `install.sh`, `SKILL.md`, `README.md`, `AGENTS.md`, and SHA-256. Target hosts do not need Rust.
- See `docs/release.md`.

## Design document sync

- Current mode: manual. After a push, run:
  `SKIP_LARK_CONFIG=1 FEISHU_AS=user FEISHU_DOC_TOKEN=... python3 scripts/update-design-doc.py`
- Script: `scripts/update-design-doc.py`
- The GitHub workflow is manual-dispatch only. Enabling the push trigger requires bot edit access and `FEISHU_APP_ID`, `FEISHU_APP_SECRET`, and `FEISHU_DOC_TOKEN` secrets.
- See `docs/design-doc-sync.md` for setup and failure behavior.

## Code rules

- Rust stable, 2021 edition or newer.
- No custom cryptography. Use audited crates.
- No plaintext fallback. Fail closed on signature, version, or crypto errors.
- L0 identity/records must stay independent of libp2p. libp2p is allowed only in L1 transport and above, inside `sceadu-core`.
- Wire-format changes require a spec update and test vectors under `spec/`; message and file formats are in `spec/message.md`.
- Never commit secrets, private keys, tokens, or `.design/` content.
- Prefer small modules with explicit interfaces.

## Security

- Treat all network input as untrusted.
- Never log private keys, session keys, or message plaintext.
- Report vulnerabilities privately; do not open public exploit issues.

## Repository layout

- `crates/` - Rust workspace (`sceadu-core`, `sceadu-cli`); `sceadu-core/src` contains `identity`, `transport`, `biography`, `discovery` (invite only), `session`, `agent`, `node.rs` composition root, and `daemon.rs` IPC server
- `spec/` - wire format and test vectors
- `skills/sceadu/` - agent skill files
- `scripts/` - maintenance and CI scripts
- `docs/` - English documentation
- `.design/` - ignored Chinese design working copies

## Current milestone

M1 and M2 are complete; M3 was replaced by the Agent message network. M4a/M4b connectivity is IPv6-only; port mapping, AutoNAT, DCUtR, and DHT discovery are removed. Circuit relay v2 and the optional Tor plugin remain. The foreground daemon, SQLite inbox, signed Biography store and bootstrap refresh, allowlist admission, A2A-inspired messages, 50 MiB file stream, and local MCP stdio adapter are implemented. A public bootstrap VPS is deployment configuration, not a protocol role; QUIC and stranger-mode consent remain future work. Remaining hardening: single-use invites, persistent reputation, stranger-mode consent/rate limits, and larger eclipse simulations.
