#!/usr/bin/env python3
"""Fail if tracked repository text contains CJK characters.

The project rule is that repository content is English. The Feishu design
document is Chinese, but its local working copies live in `.design/`, which is
git-ignored and therefore not checked by this script.
"""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys

CJK = re.compile(r"[\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff]")
BINARY_SUFFIXES = {
    ".png",
    ".jpg",
    ".jpeg",
    ".gif",
    ".webp",
    ".ico",
    ".pdf",
    ".zip",
    ".gz",
    ".png",
    ".woff",
    ".woff2",
    ".ttf",
    ".so",
    ".dylib",
    ".dll",
    ".exe",
}


def tracked_files() -> list[str]:
    output = subprocess.check_output(["git", "ls-files", "-z"])
    return [name for name in output.decode("utf-8").split("\0") if name]


def main() -> None:
    offenders: list[str] = []
    for name in tracked_files():
        path = pathlib.Path(name)
        if path.suffix.lower() in BINARY_SUFFIXES:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        if CJK.search(text):
            offenders.append(name)

    if offenders:
        print("CJK text found in tracked files:", file=sys.stderr)
        for name in offenders:
            print(f"  {name}", file=sys.stderr)
        print("Repository content must be English; move Chinese drafts to .design/.", file=sys.stderr)
        raise SystemExit(1)

    print("language check passed")


if __name__ == "__main__":
    main()
