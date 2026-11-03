# Sceadu Agent Instructions

This file is loaded by DSH-compatible agents as project context. Keep it accurate and in English.

## Project

Sceadu is a CLI-first reference implementation of an Anonymous Agent Network (AAN). Agents join as nodes without a central server, discover each other through a structured DHT plus agent-mediated neighbor discovery, and communicate over end-to-end encrypted peer-to-peer sessions. The product is a CLI distributed as a skill for locally installed agents.

Current stage: pre-alpha (M4a connectivity foundation). M3 is complete and the MCP bridge is deferred. The MVP is defined in section 4.3 of the design document.

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
- DHT discovery first; agent-mediated neighbor discovery next.
- No Sybil protection in alpha: invite-only / small networks. Do not add tokens, staking, or PoW silently.
- Anonymity scope: content confidentiality and pseudonymity by default. Tor/I2P are optional transports that can reduce network-level IP exposure; do not claim full anonymity or traffic-analysis resistance.
- Connectivity is a first-class policy layer: native libp2p equivalents for STUN/TURN/ICE-like behavior (AutoNAT/DCUtR/relay v2), UPnP/NAT-PMP/PCP, and optional Tor/I2P. Agents select `auto`, `direct`, `relay`, or `privacy`; `privacy` never silently falls back to clearnet.
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
- Keep `sceadu-core` independent of libp2p.
- Wire-format changes require a spec update and test vectors under `spec/`.
- Never commit secrets, private keys, tokens, or `.design/` content.
- Prefer small modules with explicit interfaces.

## Security

- Treat all network input as untrusted.
- Never log private keys, session keys, or message plaintext.
- Report vulnerabilities privately; do not open public exploit issues.

## Repository layout

- `crates/` - Rust workspace (`sceadu-core`, `sceadu-net`, `sceadu-cli`)
- `spec/` - wire format and test vectors
- `skills/sceadu/` - agent skill files
- `scripts/` - maintenance and CI scripts
- `docs/` - English documentation
- `.design/` - ignored Chinese design working copies

## Current milestone

M1, M2, and M3 are complete. M4a is implemented: connectivity policy (`auto`/`direct`/`relay`/`privacy`), reachability classification, address book, `sceadu net status`, and fail-closed relay/privacy behavior. M4b is next: UPnP/NAT-PMP/PCP plus AutoNAT/DCUtR/circuit relay v2. M4c adds optional Tor/I2P transports. The MCP bridge remains deferred by explicit user decision; do not add it without a new decision. Remaining hardening: single-use invites, persistent reputation, and larger eclipse simulations.
