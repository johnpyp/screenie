#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# ///
"""
Sync the reference repos listed in `references/manifest.toml` into `references/<name>/`.

References are read-only material to learn from, so each one is a shallow (depth 1)
checkout of its tracked ref, and syncing hard-resets it to the latest upstream commit.
Checkouts with local modifications are skipped unless `--force` is given.

Usage:
    tools/fetch_references.py              # clone missing refs, update existing ones
    tools/fetch_references.py screendrop   # sync only the named refs
    tools/fetch_references.py --list       # show manifest entries and local status
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import tomllib
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REFERENCES_DIR = ROOT / "references"
MANIFEST = REFERENCES_DIR / "manifest.toml"
# Files in references/ that are part of this repo rather than checkouts.
TRACKED_FILES = {"manifest.toml", ".gitkeep"}


@dataclass(frozen=True)
class Reference:
    name: str
    url: str
    ref: str | None
    notes: str

    @property
    def path(self) -> Path:
        return REFERENCES_DIR / self.name


def load_manifest() -> list[Reference]:
    data = tomllib.loads(MANIFEST.read_text())
    refs = [
        Reference(
            name=entry["name"],
            url=entry["url"],
            ref=entry.get("ref"),
            notes=entry.get("notes", "").strip(),
        )
        for entry in data.get("repo", [])
    ]
    names = [r.name for r in refs]
    if dupes := {n for n in names if names.count(n) > 1}:
        sys.exit(f"duplicate reference names in manifest: {', '.join(sorted(dupes))}")
    return refs


def git(*args: str, cwd: Path) -> str:
    result = subprocess.run(
        ["git", *args], cwd=cwd, capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed:\n{result.stderr.strip()}")
    return result.stdout.strip()


def has_commit(path: Path) -> bool:
    """False for a checkout whose first fetch never completed."""
    result = subprocess.run(
        ["git", "rev-parse", "--verify", "--quiet", "HEAD"], cwd=path, capture_output=True, check=False
    )
    return result.returncode == 0


def is_dirty(path: Path) -> bool:
    return bool(git("status", "--porcelain", cwd=path))


def head_summary(path: Path) -> str:
    return git("log", "-1", "--format=%h %cs %s", cwd=path)


def sync(ref: Reference, force: bool) -> tuple[Reference, str, bool]:
    """Bring one reference up to date. Returns (ref, message, ok)."""
    try:
        initialized = (ref.path / ".git").exists()
        fresh = not initialized or not has_commit(ref.path)
        if not initialized:
            ref.path.mkdir(parents=True, exist_ok=True)
            git("init", "--quiet", cwd=ref.path)
            git("remote", "add", "origin", ref.url, cwd=ref.path)
        elif fresh:
            # A previous first fetch failed part-way; retry it.
            git("remote", "set-url", "origin", ref.url, cwd=ref.path)
        else:
            git("remote", "set-url", "origin", ref.url, cwd=ref.path)
            if is_dirty(ref.path) and not force:
                return ref, "skipped: local modifications (use --force)", False
            before = git("rev-parse", "HEAD", cwd=ref.path)

        git("fetch", "--quiet", "--depth", "1", "origin", ref.ref or "HEAD", cwd=ref.path)
        git("reset", "--quiet", "--hard", "FETCH_HEAD", cwd=ref.path)
        git("clean", "--quiet", "-fdx", cwd=ref.path)

        if fresh:
            action = "cloned"
        elif git("rev-parse", "HEAD", cwd=ref.path) == before:
            action = "up to date"
        else:
            action = "updated"
        return ref, f"{action}: {head_summary(ref.path)}", True
    except RuntimeError as err:
        return ref, str(err), False


def list_references(refs: list[Reference]) -> None:
    for ref in refs:
        if (ref.path / ".git").exists():
            status = head_summary(ref.path)
            if is_dirty(ref.path):
                status += " (modified)"
        else:
            status = "not fetched"
        print(f"{ref.name}  [{ref.url}{'@' + ref.ref if ref.ref else ''}]")
        print(f"  status: {status}")
        for line in ref.notes.splitlines():
            print(f"  {line}")
        print()


def warn_unmanaged(refs: list[Reference]) -> None:
    known = {r.name for r in refs} | TRACKED_FILES
    for entry in sorted(REFERENCES_DIR.iterdir()):
        if entry.name not in known:
            print(f"note: references/{entry.name} is not in the manifest", file=sys.stderr)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0].strip())
    parser.add_argument("names", nargs="*", help="only sync these references")
    parser.add_argument("--list", action="store_true", help="show references and status")
    parser.add_argument(
        "--force", action="store_true", help="discard local modifications when syncing"
    )
    args = parser.parse_args()

    refs = load_manifest()
    if args.names:
        by_name = {r.name: r for r in refs}
        if unknown := [n for n in args.names if n not in by_name]:
            parser.error(f"unknown reference(s): {', '.join(unknown)}")
        refs = [by_name[n] for n in args.names]

    if args.list:
        list_references(refs)
        return 0

    with ThreadPoolExecutor(max_workers=8) as pool:
        results = list(pool.map(lambda r: sync(r, args.force), refs))

    width = max(len(r.name) for r in refs)
    for ref, message, ok in results:
        print(f"{'ok ' if ok else 'ERR'} {ref.name:<{width}}  {message}")
    warn_unmanaged(load_manifest())
    return 0 if all(ok for _, _, ok in results) else 1


if __name__ == "__main__":
    sys.exit(main())
