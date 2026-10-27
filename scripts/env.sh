#!/bin/sh
# Source this file to use the workspace-local Rust toolchain.
# Usage: . ./scripts/env.sh

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
export RUSTUP_HOME="$ROOT/.rustup"
export CARGO_HOME="$ROOT/.cargo"
export PATH="$CARGO_HOME/bin:$PATH"
