#!/usr/bin/env sh
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
bin_dir=${SCEADU_BIN_DIR:-"$HOME/.local/bin"}
agents_home=${AGENTS_HOME:-"$HOME/.agents"}
skill_dir="$agents_home/skills/sceadu"

mkdir -p "$bin_dir" "$skill_dir"
install -m 0755 "$here/sceadu" "$bin_dir/sceadu"
install -m 0644 "$here/SKILL.md" "$skill_dir/SKILL.md"

printf 'installed %s\n' "$bin_dir/sceadu"
printf 'installed %s\n' "$skill_dir/SKILL.md"
"$bin_dir/sceadu" --version

case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) printf 'warning: add %s to PATH\n' "$bin_dir" >&2 ;;
esac
