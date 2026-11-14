# Release Convention

Sceadu's Cargo version is `0.1.0`. The first tagged release is `v0.1.0`.
Test builds use:

```text
v0.1.0-buildYYYYMMDD
```

Create at most one build tag per date. Test builds are private pre-releases;
do not use a moving `latest` tag. `v0.1.0` is the first non-prerelease release.

The release workflow builds a static `x86_64-unknown-linux-musl` binary and
publishes either `sceadu-v0.1.0-...` or `sceadu-v0.1.0-buildYYYYMMDD-...`.
The tarball contains `sceadu`, `install.sh`, `SKILL.md`, all README
translations, and `AGENTS.md`. `install.sh` installs to `~/.local/bin/sceadu` and
`~/.agents/skills/sceadu/SKILL.md` without sudo.

## Private download and scp

The repository is private for the test phase. Download the asset on a trusted
machine with access (for example via `gh release download`), verify the
checksum, and copy it to the target machines:

```sh
gh release download v0.1.0 \
  --pattern 'sceadu-v0.1.0-x86_64-unknown-linux-musl.tar.gz*'
sha256sum -c sceadu-v0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256
scp sceadu-v0.1.0-x86_64-unknown-linux-musl.tar.gz sceadu-c:/tmp/
```

Then on each target:

```sh
tar -xzf /tmp/sceadu-v0.1.0-build20261103-x86_64-unknown-linux-musl.tar.gz -C /tmp
cd /tmp/sceadu
./install.sh
sceadu --version
sceadu skill path
```

No Rust toolchain is installed on the target machines.
