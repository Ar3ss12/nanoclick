#!/usr/bin/env python3
"""Count lines of code in this repository - Markdown excluded by default.

Why this exists: the docs quote counts and sizes, and AGENTS.md 4 requires every number
to come from a real run. This is that run for "lines of code".

Usage:
  python scripts/count-loc.py                  # whole repo, .md skipped
  python scripts/count-loc.py --include-md     # count Markdown too
  python scripts/count-loc.py src src-tauri    # only these paths (relative to the root)
  python scripts/count-loc.py --by-file        # add a per-file listing
  python scripts/count-loc.py --json           # machine-readable output

Per language: lines / blank / comment / code. A line with code AND a trailing comment
counts as code (the usual SLOC convention). Standard library only - no virtualenv, in the
same spirit as the frontend rule (no bundlers, no node_modules).
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

# Directories that are build output, VCS metadata or caches - never source.
SKIP_DIRS = {
    ".git", "target", "node_modules", "__pycache__", ".venv", "venv",
    "dist", "build", "bundle", ".idea", ".vscode", ".pytest_cache", ".mypy_cache",
}

# Extensions we never read as text (binary assets and compiled artefacts).
BINARY_EXTS = {
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".ico", ".icns",
    ".exe", ".dll", ".pdb", ".lib", ".rlib", ".so", ".dylib", ".a", ".o", ".obj",
    ".zip", ".7z", ".gz", ".br", ".tar", ".pdf", ".sig",
    ".woff", ".woff2", ".ttf", ".otf", ".eot",
    ".mp3", ".mp4", ".webm", ".wav", ".ogg",
}

# Machine-written files that are not code: a lock file is generated, a log is output,
# and *.txt here is scratch (the .gitignore treats it that way too). Excluded by default,
# so "lines of code" stays honest; --include-generated brings them back.
GENERATED_EXTS = {".lock", ".log", ".txt"}
GENERATED_NAMES = {"cargo.lock", "package-lock.json", "yarn.lock", "pnpm-lock.yaml"}

LANGUAGES = {
    ".rs": "Rust",
    ".js": "JavaScript", ".mjs": "JavaScript", ".cjs": "JavaScript",
    ".html": "HTML", ".css": "CSS", ".json": "JSON", ".svg": "SVG",
    ".ps1": "PowerShell", ".py": "Python", ".sh": "Shell", ".cmd": "Batch", ".bat": "Batch",
    ".toml": "TOML", ".yaml": "YAML", ".yml": "YAML", ".xml": "XML",
    ".md": "Markdown",
    ".c": "C", ".h": "C", ".cpp": "C++", ".hpp": "C++", ".cs": "C#",
    ".java": "Java", ".go": "Go", ".ts": "TypeScript",
}

# Single-line comment prefixes, per extension.
LINE_COMMENTS = {
    ".rs": "//", ".js": "//", ".mjs": "//", ".cjs": "//", ".ts": "//",
    ".c": "//", ".h": "//", ".cpp": "//", ".hpp": "//", ".cs": "//",
    ".java": "//", ".go": "//",
    ".py": "#", ".ps1": "#", ".sh": "#", ".toml": "#", ".yaml": "#", ".yml": "#",
}

# Block comment delimiters, per extension.
BLOCK_COMMENTS = {
    ".rs": ("/*", "*/"), ".js": ("/*", "*/"), ".mjs": ("/*", "*/"), ".cjs": ("/*", "*/"),
    ".ts": ("/*", "*/"), ".c": ("/*", "*/"), ".h": ("/*", "*/"), ".cpp": ("/*", "*/"),
    ".hpp": ("/*", "*/"), ".cs": ("/*", "*/"), ".java": ("/*", "*/"), ".go": ("/*", "*/"),
    ".css": ("/*", "*/"),
    ".html": ("<!--", "-->"), ".svg": ("<!--", "-->"), ".xml": ("<!--", "-->"),
    ".md": ("<!--", "-->"),
}


def classify(text: str, ext: str) -> tuple[int, int, int, int]:
    """Return (lines, blank, comment, code) for one file's text."""
    line_prefix = LINE_COMMENTS.get(ext)
    block = BLOCK_COMMENTS.get(ext)
    blank = comment = code = 0
    in_block = False

    for raw in text.splitlines():
        stripped = raw.strip()
        if not stripped:
            blank += 1
            continue

        # Finish a block comment opened on an earlier line.
        if in_block and block:
            end = stripped.find(block[1])
            if end == -1:
                comment += 1
                continue
            in_block = False
            stripped = stripped[end + len(block[1]):].strip()
            if not stripped:
                comment += 1
                continue

        # A block comment that opens here.
        if block and stripped.startswith(block[0]):
            end = stripped.find(block[1], len(block[0]))
            if end == -1:
                in_block = True
                comment += 1
                continue
            stripped = stripped[end + len(block[1]):].strip()
            if not stripped:
                comment += 1
                continue
            code += 1  # real code follows the closed block comment on the same line
            continue

        if line_prefix and stripped.startswith(line_prefix):
            comment += 1
            continue

        code += 1

    return (blank + comment + code, blank, comment, code)
def iter_files(roots: list[Path]):
    """Yield every countable file under roots, skipping artefacts and binaries."""
    for root in roots:
        if root.is_file():
            yield root
            continue
        for path in sorted(root.rglob("*")):
            if not path.is_file():
                continue
            if any(part in SKIP_DIRS for part in path.parts):
                continue
            if path.suffix.lower() in BINARY_EXTS:
                continue
            yield path


def read_text(path: Path) -> str | None:
    """Text, or None when the file is binary/undecodable (NUL byte or bad UTF-8)."""
    try:
        data = path.read_bytes()
    except OSError:
        return None
    if b"\x00" in data:
        return None
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        return None


def resolve_root(explicit: str | None) -> Path:
    """The repository root: --root, else the parent directory of this script."""
    return Path(explicit).resolve() if explicit else Path(__file__).resolve().parent.parent


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Count lines of code (Markdown excluded unless --include-md).",
    )
    parser.add_argument("paths", nargs="*", help="paths to scan, relative to the repo root")
    parser.add_argument("--root", help="repository root (default: this script's parent directory)")
    parser.add_argument("--include-md", action="store_true", help="count Markdown files too")
    parser.add_argument("--include-generated", action="store_true",
                        help="count lock files and logs too")
    parser.add_argument("--by-file", action="store_true", help="print a per-file listing")
    parser.add_argument("--json", action="store_true", help="print JSON instead of a table")
    args = parser.parse_args(argv)

    root = resolve_root(args.root)
    if not root.is_dir():
        print(f"error: root does not exist: {root}", file=sys.stderr)
        return 2

    targets = [root] if not args.paths else [root / p for p in args.paths]
    missing = [str(t) for t in targets if not t.exists()]
    if missing:
        print("error: not found: " + ", ".join(missing), file=sys.stderr)
        return 2

    stats: dict[str, list[int]] = {}
    per_file: list[tuple[str, int, int, int, int]] = []
    skipped_binary = 0
    skipped_md = 0
    skipped_generated = 0

    for path in iter_files(targets):
        ext = path.suffix.lower()
        if ext == ".md" and not args.include_md:
            skipped_md += 1
            continue
        if not args.include_generated and (ext in GENERATED_EXTS
                                           or path.name.lower() in GENERATED_NAMES):
            skipped_generated += 1
            continue
        text = read_text(path)
        if text is None:
            skipped_binary += 1
            continue

        language = LANGUAGES.get(ext, "Other (" + (ext[1:] or "no extension") + ")")
        lines, blank, comment, code = classify(text, ext)
        bucket = stats.setdefault(language, [0, 0, 0, 0, 0])
        bucket[0] += 1
        bucket[1] += lines
        bucket[2] += blank
        bucket[3] += comment
        bucket[4] += code
        if args.by_file:
            per_file.append((str(path.relative_to(root)), lines, blank, comment, code))

    totals = [sum(v[i] for v in stats.values()) for i in range(5)]

    if args.json:
        print(json.dumps({
            "root": str(root),
            "include_markdown": args.include_md,
            "skipped_markdown_files": skipped_md,
            "skipped_generated_files": skipped_generated,
            "skipped_binary_files": skipped_binary,
            "languages": {
                name: {"files": v[0], "lines": v[1], "blank": v[2], "comment": v[3], "code": v[4]}
                for name, v in sorted(stats.items(), key=lambda kv: kv[1][4], reverse=True)
            },
            "total": {"files": totals[0], "lines": totals[1], "blank": totals[2],
                      "comment": totals[3], "code": totals[4]},
        }, indent=2))
        return 0

    scope = "Markdown excluded" if not args.include_md else "Markdown included"
    print("Lines of code - " + str(root))
    print("(" + scope + "; skipped dirs: " + ", ".join(sorted(SKIP_DIRS)) + ")")
    print()
    header = "{:<20}{:>7}{:>10}{:>9}{:>10}{:>10}".format(
        "language", "files", "lines", "blank", "comment", "code")
    print(header)
    print("-" * len(header))
    for name, v in sorted(stats.items(), key=lambda kv: kv[1][4], reverse=True):
        print("{:<20}{:>7}{:>10,}{:>9,}{:>10,}{:>10,}".format(name, v[0], v[1], v[2], v[3], v[4]))
    print("-" * len(header))
    print("{:<20}{:>7}{:>10,}{:>9,}{:>10,}{:>10,}".format(
        "TOTAL", totals[0], totals[1], totals[2], totals[3], totals[4]))

    if args.by_file and per_file:
        print()
        print("Per file (code / lines / blank / comment):")
        for name, lines, blank, comment, code in sorted(per_file, key=lambda r: r[4], reverse=True):
            print("  {:>7,}  {:>8,} {:>7,} {:>8,}  {}".format(code, lines, blank, comment, name))

    print()
    tail = "" if args.include_md else " (pass --include-md to count them)"
    generated_tail = "" if args.include_generated else " (pass --include-generated to count them)"
    print("skipped: {} Markdown file(s){}, {} lock/log file(s){}, {} binary/undecodable file(s)".format(
        skipped_md, tail, skipped_generated, generated_tail, skipped_binary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
