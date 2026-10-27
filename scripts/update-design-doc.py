#!/usr/bin/env python3
"""Append a code-update entry to the Sceadu Feishu design document.

Required environment:
    FEISHU_APP_ID
    FEISHU_APP_SECRET
    FEISHU_DOC_TOKEN

Optional environment:
    LARK_CLI_BIN       Path to lark-cli (default: lark-cli)
    SKIP_LARK_CONFIG   Set to 1 to skip `lark-cli config init` (local use)
"""

from __future__ import annotations

import datetime as dt
import json
import os
import subprocess
import sys

FILES_LIMIT = 12


def die(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(1)


def require(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        die(f"missing required environment variable {name}")
    return value


def cli_env() -> dict[str, str]:
    env = os.environ.copy()
    env["LARKSUITE_CLI_NO_UPDATE_NOTIFIER"] = "1"
    env["LARKSUITE_CLI_NO_SKILLS_NOTIFIER"] = "1"
    return env


def run(cmd: list[str], *, input_text: str | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        input=input_text,
        text=True,
        capture_output=True,
        env=cli_env(),
    )


def git(*args: str) -> str:
    return subprocess.check_output(
        ["git", *args],
        text=True,
        stderr=subprocess.DEVNULL,
    ).strip()


def xml_escape(text: str) -> str:
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def changed_files() -> list[str]:
    commands = (
        ("diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"),
        ("show", "--pretty=format:", "--name-only", "HEAD"),
    )
    for command in commands:
        try:
            output = git(*command)
        except subprocess.CalledProcessError:
            continue
        files = [line.strip() for line in output.splitlines() if line.strip()]
        if files:
            return files
    return []


def commit_time() -> str:
    try:
        raw = git("log", "-1", "--format=%cI")
        parsed = dt.datetime.fromisoformat(raw.replace("Z", "+00:00"))
        return parsed.astimezone(dt.timezone.utc).strftime("%Y-%m-%d %H:%M UTC")
    except Exception:
        return dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%d %H:%M UTC")


def main() -> None:
    doc_token = require("FEISHU_DOC_TOKEN")
    if doc_token.startswith("http"):
        doc_token = doc_token.rstrip("/").rsplit("/", 1)[-1]

    cli = os.environ.get("LARK_CLI_BIN", "lark-cli")
    as_identity = os.environ.get("FEISHU_AS", "user").strip() or "user"
    if as_identity not in ("user", "bot"):
        die("FEISHU_AS must be user or bot")

    if os.environ.get("SKIP_LARK_CONFIG") != "1":
        app_id = require("FEISHU_APP_ID")
        app_secret = require("FEISHU_APP_SECRET")
        configured = run(
            [
                cli,
                "config",
                "init",
                "--app-id",
                app_id,
                "--app-secret-stdin",
                "--brand",
                "feishu",
            ],
            input_text=app_secret + "\n",
        )
        if configured.returncode != 0:
            detail = (configured.stderr or configured.stdout or "").strip()[-800:]
            die(f"lark-cli config init failed: {detail}")

    sha = os.environ.get("GITHUB_SHA", "").strip() or git("rev-parse", "HEAD")
    short_sha = sha[:7]
    subject = os.environ.get("COMMIT_MESSAGE", "").strip() or git("log", "-1", "--pretty=%s")
    author = git("log", "-1", "--pretty=%an")
    files = changed_files()
    shown = files[:FILES_LIMIT]
    file_text = ", ".join(shown) if shown else "none recorded"
    if len(files) > FILES_LIMIT:
        file_text += f", and {len(files) - FILES_LIMIT} more"

    entry = (
        f"<p><b>Code update</b> · <code>{xml_escape(short_sha)}</code> · "
        f"{xml_escape(commit_time())} · {xml_escape(author)}<br/>"
        f"{xml_escape(subject)}<br/>Files: {xml_escape(file_text)}</p>"
    )

    updated = run(
        [
            cli,
            "docs",
            "+update",
            "--doc",
            doc_token,
            "--command",
            "append",
            "--content",
            entry,
            "--as",
            as_identity,
            "--format",
            "json",
        ]
    )
    if updated.returncode != 0:
        detail = (updated.stderr or updated.stdout or "").strip()[-800:]
        die(f"failed to update the design document: {detail}")

    try:
        payload = json.loads(updated.stdout)
    except json.JSONDecodeError:
        die(f"unexpected response from lark-cli: {updated.stdout.strip()[:400]}")

    if not payload.get("ok"):
        die(f"design document update was rejected: {json.dumps(payload)[:400]}")

    result = payload.get("data", {}).get("result")
    if result not in (None, "success"):
        warnings = payload.get("data", {}).get("warnings", [])
        die(f"design document update failed: result={result!r}; warnings={warnings}")

    print(f"appended {short_sha} to the design document")


if __name__ == "__main__":
    main()
