#!/usr/bin/env python3
"""Refuse to commit anything that must never be in this repository.

The project is a fan work about a commercial game, and the author's rule is
absolute: no audio, no installers, no binaries, no game assets in the source
tree. It is easy to break that rule by accident -- a stray test clip, a build
artifact, a screenshot of an in-game screen. This script is the guard.

It runs three ways:
  * as a git pre-commit hook (.githooks/pre-commit)
  * in CI (ci.yml), over the whole tree
  * by hand:  python scripts/check_repo_hygiene.py [--all]

Exit code 0 means clean. Anything else lists the offenders.
"""
from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Anything that could be an audio recording, an installer or a compiled binary.
FORBIDDEN_SUFFIXES = {
    ".mp3", ".m4a", ".mp4", ".aac", ".wav", ".wave", ".ogg", ".oga", ".opus",
    ".flac", ".aif", ".aiff", ".wma", ".wem", ".mkv", ".webm", ".avi", ".mov",
    ".exe", ".dll", ".msi", ".appx", ".deb", ".rpm", ".so", ".a", ".o", ".lib",
    ".AppImage", ".zip", ".tar", ".gz", ".7z", ".rar", ".jar", ".apk", ".ipa",
    ".ttf", ".otf", ".ttc", ".woff", ".woff2",
}

# Small generated PNGs the UI needs. Everything else image-shaped is suspect:
# a screenshot of the game would land here.
ALLOWED_IMAGES = {
    "crates/eds-app/ui/icon.png",
    "crates/eds-app/ui/hatch.png",
}

MAX_FILE_BYTES = 200 * 1024
# A licence, a README and a .slint skin are legitimately long; nothing else in
# a project this size should be near it.
MAX_SOURCE_LINES = 1400

TRACK_IN_SPITE_OF_NAME = re.compile(r"(^|/)(LICENSE-code|LICENSE-content|NOTICE)$")


def staged_files() -> list[str]:
    out = subprocess.run(
        ["git", "diff", "--cached", "--name-only", "-z", "--diff-filter=ACM"],
        cwd=ROOT, capture_output=True, check=True,
    ).stdout
    return [p.decode() for p in out.split(b"\0") if p]


def tracked_files() -> list[str]:
    out = subprocess.run(
        ["git", "ls-files", "-z"], cwd=ROOT, capture_output=True, check=True
    ).stdout
    return [p.decode() for p in out.split(b"\0") if p]


def all_files() -> list[str]:
    skip = {".git", "target", "dist", "__pycache__", ".cargo"}
    found = []
    for p in ROOT.rglob("*"):
        parts = set(p.relative_to(ROOT).parts)
        if parts & skip or not p.is_file():
            continue
        found.append(p.relative_to(ROOT).as_posix())
    return found


def check(rel: str) -> list[str]:
    problems: list[str] = []
    path = ROOT / rel
    suffix = path.suffix.lower()
    if suffix in FORBIDDEN_SUFFIXES and not TRACK_IN_SPITE_OF_NAME.search(rel):
        problems.append(f"{rel}: 禁止提交 {suffix} 文件（音频/安装包/字体/二进制）")
        return problems
    if suffix in {".png", ".jpg", ".jpeg", ".gif", ".bmp", ".webp"} and rel not in ALLOWED_IMAGES:
        problems.append(f"{rel}: 图片必须登记在 ALLOWED_IMAGES 里（防止误提交游戏截图）")
    try:
        size = path.stat().st_size
    except OSError:
        return problems
    if size > MAX_FILE_BYTES:
        problems.append(f"{rel}: {size} 字节，超过 {MAX_FILE_BYTES} 的上限")
    if suffix in {".rs", ".slint", ".md", ".toml", ".py", ".sh", ".yml"}:
        try:
            lines = path.read_text(encoding="utf-8", errors="replace").count("\n") + 1
        except OSError:
            lines = 0
        if lines > MAX_SOURCE_LINES:
            problems.append(f"{rel}: {lines} 行，超过 {MAX_SOURCE_LINES} 行，考虑拆分")
    return problems


def non_ascii_paths(files: list[str]) -> list[str]:
    # The build has to work from a path like D:\works\edscene; a Chinese file
    # name in the tree has broken JVM and shell toolchains on this author's box.
    return [f for f in files if not f.isascii()]


def main(argv: list[str]) -> int:
    files = all_files() if "--all" in argv else (staged_files() or tracked_files() or all_files())
    problems: list[str] = []
    for f in files:
        problems += check(f)
    odd = non_ascii_paths(files)
    if odd:
        problems.append("仓库路径必须是纯 ASCII：" + ", ".join(odd[:5]))
    if problems:
        print("仓库卫生检查未通过：", file=sys.stderr)
        for p in problems:
            print(f"  ! {p}", file=sys.stderr)
        print("\n本仓库不得包含任何音频、安装包、字体或游戏素材。", file=sys.stderr)
        return 1
    print(f"仓库卫生检查通过（{len(files)} 个文件）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
