# Release Changelog Template

Use this template for every GitHub Release. Write for users, not as a commit
log. Keep entries short, concrete, and verifiable. Do not include secrets,
private hostnames, API keys, or internal-only credentials.

```markdown
# Sceadu vX.Y.Z

**Release date:** YYYY-MM-DD  
**Release type:** stable | pre-release | test build  
**Previous release:** vA.B.C

## Highlights

- One to three user-visible outcomes.
- What changed for a normal user or Agent operator.

## Added

- New user-facing capability, command, tool, or platform support.

## Changed

- Behavior changes, defaults, compatibility changes, renamed options.

## Fixed

- User-visible bugs and regressions.

## Security

- Security-relevant changes.
- If there are none, write `No security-relevant changes.`

## Compatibility and upgrade

- Supported platforms and architectures.
- Wire/protocol compatibility.
- Upgrade or rollback steps.
- Any required configuration changes.

## Known limitations

- Remaining constraints that users must know.

## Verification

- [ ] `python3 scripts/check-language.py`
- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --all`
- [ ] Release asset SHA-256 verified
- [ ] Clean install smoke test
- [ ] Bootstrap/peer smoke test (if networking changed)

## Links

- Full changelog: `CHANGELOG.md`
- Compare: `https://github.com/loriskong/sceadu/compare/vA.B.C...vX.Y.Z`
```

## Rules

- Follow Keep a Changelog categories: Added, Changed, Fixed, Security.
- Use Semantic Versioning for release tags.
- Put the most important change under Highlights; do not repeat the same item
  in multiple sections.
- `test build` releases use the same headings, but mark the release as
  pre-release and name the build date in the title.
- Release notes must match `CHANGELOG.md`.
- Add the template checklist result to the release notes before publishing.
