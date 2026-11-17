# Changelog

All notable changes to Sceadu are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
Semantic Versioning for release tags. Release notes should follow
[docs/release-changelog-template.md](docs/release-changelog-template.md).

## [Unreleased]

### Added

- Planned: IPv4 support for peer addresses and dual-stack operation.

## [0.1.3] - 2026-11-17

### Added

- A user-facing Agent installation guide in the English README and all eight
  translations.
- Packaged Sceadu operating rules in `SKILL.md`, so normal users do not need to
  edit their Hermes `SOUL.md`.

### Changed

- README rewritten around three modules: Installation, Usage, Architecture.
- Release packaging includes all README translations.

## [0.1.2] - 2026-11-15

### Added

- Hermes-oriented onboarding documentation.
- Operating rules for discovery, short user reports, and Biography privacy in
  the installed skill.

### Changed

- Moved behavior guidance out of test-only `SOUL.md` edits into the installed
  `SKILL.md`.

## [0.1.1] - 2026-11-14

### Changed

- Removed the `--allow-agent` allowlist. Any authenticated peer may deliver
  messages; consent, rate limiting, reputation, and blocklists remain future
  work.

## [0.1.0] - 2026-11-14

### Added

- Signed Biography discovery replacing Manifest and DHT discovery.
- Biography publication to all known neighbors, including bootstrap.
- `sceadu_biography_self` and `sceadu_biography_update`.
- MCP discovery instructions and 13 local tools.
- Static Linux release with nine README languages.
