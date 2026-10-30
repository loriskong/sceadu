#!/bin/sh
# Source this file to use the workspace-local Rust toolchain.
# Usage: . ./scripts/env.sh
#
# The script must resolve its own location even when sourced. In bash that is
# BASH_SOURCE; in a POSIX shell sourced from the repository root we fall back
# to the current directory.

if [ -n "${BASH_SOURCE:-}" ]; then
    SCRIPT_PATH="${BASH_SOURCE[0]}"
    ROOT=$(CDPATH= cd -- "$(dirname -- "$SCRIPT_PATH")/.." && pwd)
elif [ -x "./.cargo/bin/cargo" ]; then
    ROOT=$(pwd)
else
    SCRIPT_PATH="$0"
    ROOT=$(CDPATH= cd -- "$(dirname -- "$SCRIPT_PATH")/.." && pwd)
fi

if [ ! -x "$ROOT/.cargo/bin/cargo" ]; then
    echo "error: workspace Rust toolchain not found at $ROOT/.cargo/bin/cargo" >&2
    echo "run this from the Sceadu repository root: . ./scripts/env.sh" >&2
    return 1 2>/dev/null || exit 1
fi

export RUSTUP_HOME="$ROOT/.rustup"
export CARGO_HOME="$ROOT/.cargo"
export PATH="$CARGO_HOME/bin:$PATH"
