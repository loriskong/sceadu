#!/bin/sh
# Source this file to switch GitHub identities in the current shell:
#   . ./scripts/github-account.sh
#   github_use imloriskong
#
# The script never stores tokens. It selects an SSH key and exports Git
# author/committer variables. Inside a repository it also sets the local
# user.name and user.email so commits stay attributed correctly.

SCEADU_IMLORISKONG_NAME="${SCEADU_IMLORISKONG_NAME:-imloriskong}"
SCEADU_IMLORISKONG_EMAIL="${SCEADU_IMLORISKONG_EMAIL:-imloriskong@gmail.com}"
SCEADU_IMLORISKONG_KEY="${SCEADU_IMLORISKONG_KEY:-$HOME/.ssh/id_ed25519_imloriskong}"
SCEADU_IMLORISKONG_GITHUB="${SCEADU_IMLORISKONG_GITHUB:-loriskong}"

SCEADU_VALAXKONG_NAME="${SCEADU_VALAXKONG_NAME:-valaxkong}"
SCEADU_VALAXKONG_EMAIL="${SCEADU_VALAXKONG_EMAIL:-valaxkong@163.com}"
SCEADU_VALAXKONG_KEY="${SCEADU_VALAXKONG_KEY:-$HOME/.ssh/id_ed25519}"
SCEADU_VALAXKONG_GITHUB="${SCEADU_VALAXKONG_GITHUB:-valaxkong}"

github_use() {
  account="$1"
  case "$account" in
    imloriskong)
      name="$SCEADU_IMLORISKONG_NAME"
      email="$SCEADU_IMLORISKONG_EMAIL"
      key="$SCEADU_IMLORISKONG_KEY"
      github="$SCEADU_IMLORISKONG_GITHUB"
      ;;
    valaxkong)
      name="$SCEADU_VALAXKONG_NAME"
      email="$SCEADU_VALAXKONG_EMAIL"
      key="$SCEADU_VALAXKONG_KEY"
      github="$SCEADU_VALAXKONG_GITHUB"
      ;;
    *)
      printf 'usage: github_use imloriskong|valaxkong\n' >&2
      return 2
      ;;
  esac

  export GIT_AUTHOR_NAME="$name"
  export GIT_AUTHOR_EMAIL="$email"
  export GIT_COMMITTER_NAME="$name"
  export GIT_COMMITTER_EMAIL="$email"
  export GIT_SSH_COMMAND="ssh -i \"$key\" -o IdentitiesOnly=yes"
  export SCEADU_GITHUB_USER="$github"

  if git rev-parse --git-dir >/dev/null 2>&1; then
    git config --local user.name "$name"
    git config --local user.email "$email"
  fi

  printf 'GitHub account: %s (github.com/%s) <%s>\n' "$account" "$github" "$email"
  printf 'SSH key: %s\n' "$key"
  if [ ! -f "$key" ]; then
    printf 'warning: SSH key not found; create it before pushing\n' >&2
  fi
}

github_status() {
  author_name="${GIT_AUTHOR_NAME:-$(git config user.name 2>/dev/null || true)}"
  author_email="${GIT_AUTHOR_EMAIL:-$(git config user.email 2>/dev/null || true)}"
  printf 'author: %s <%s>\n' "$author_name" "$author_email"
  printf 'ssh: %s\n' "${GIT_SSH_COMMAND:-<not set>}"
  if git rev-parse --git-dir >/dev/null 2>&1; then
    printf 'local user: %s <%s>\n' "$(git config user.name 2>/dev/null || true)" "$(git config user.email 2>/dev/null || true)"
    printf 'remote: %s\n' "$(git remote get-url origin 2>/dev/null || echo '<none>')"
  fi
}

github_key() {
  account="$1"
  case "$account" in
    imloriskong) printf '%s\n' "$SCEADU_IMLORISKONG_KEY" ;;
    valaxkong) printf '%s\n' "$SCEADU_VALAXKONG_KEY" ;;
    *)
      printf 'usage: github_key imloriskong|valaxkong\n' >&2
      return 2
      ;;
  esac
}

case "$0" in
  *github-account.sh)
    printf 'Source this file instead of executing it:\n  . %s\n' "$0"
    ;;
esac
