# Release Convention

Sceadu's Cargo version is `0.1.0`. Release tags use the form:

```text
v0.1.0-buildYYYYMMDD
```

Examples: `v0.1.0-build20261103`. Create at most one build tag per date. The
tag is a private pre-release; do not use a moving `latest` tag for test builds.

The release workflow builds a static `x86_64-unknown-linux-musl` binary and
publishes:

```text
sceadu-v0.1.0-buildYYYYMMDD-x86_64-unknown-linux-musl.tar.gz
sceadu-v0.1.0-buildYYYYMMDD-x86_64-unknown-linux-musl.tar.gz.sha256
```

The tarball contains `sceadu`, `install.sh`, `SKILL.md`, `README.md`, and
`AGENTS.md`. `install.sh` installs to `~/.local/bin/sceadu` and
`~/.agents/skills/sceadu/SKILL.md` without sudo.

## Private download and scp

The repository is private for the test phase. Download the asset on a trusted
machine with access (for example via `gh release download`), verify the
checksum, and copy it to the target machines:

```sh
gh release download v0.1.0-build20261103 \
  --pattern 'sceadu-v0.1.0-build20261103-x86_64-unknown-linux-musl.tar.gz*'
sha256sum -c sceadu-v0.1.0-build20261103-x86_64-unknown-linux-musl.tar.gz.sha256
scp sceadu-v0.1.0-build20261103-x86_64-unknown-linux-musl.tar.gz sceadu-b:/tmp/
scp sceadu-v0.1.0-build20261103-x86_64-unknown-linux-musl.tar.gz sceadu-c:/tmp/
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
