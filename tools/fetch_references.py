#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# ///
"""
Sync the reference repos listed in `references/manifest.toml` into `references/<name>/`,
along with those in `references/manifest.local.toml`: the same format, but gitignored, for
references that stay out of the repo.

References are read-only material to learn from, so each one is a shallow (depth 1)
checkout of its tracked ref. Syncing asks every upstream where its ref points now (one
round trip each, all at once) and fetches only the references that moved or are
missing, at depth 1, then hard-resets them to it. Checkouts with local modifications are
left alone unless `--force` is given.

Usage:
    tools/fetch_references.py              # clone missing refs, update stale ones
    tools/fetch_references.py screendrop   # sync only the named refs
    tools/fetch_references.py --list       # show both manifests' entries and checkout status
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import tomllib
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REFERENCES_DIR = ROOT / "references"
MANIFEST = REFERENCES_DIR / "manifest.toml"
LOCAL_MANIFEST = REFERENCES_DIR / "manifest.local.toml"  # optional, gitignored
# Files in references/ that aren't checkouts.
MANIFEST_FILES = {MANIFEST.name, LOCAL_MANIFEST.name, ".gitkeep"}
COMMIT_ID = re.compile(r"[0-9a-f]{40}")
# Upstreams are public: fail rather than prompt for credentials on a bad URL.
GIT_ENV = {**os.environ, "GIT_TERMINAL_PROMPT": "0"}


@dataclass(frozen=True)
class Reference:
    name: str
    url: str
    ref: str | None
    notes: str
    local: bool

    @property
    def path(self) -> Path:
        return REFERENCES_DIR / self.name


def load_manifest() -> list[Reference]:
    """The shared manifest's references, then the local one's."""
    refs = []
    for manifest in (MANIFEST, LOCAL_MANIFEST):
        if manifest is LOCAL_MANIFEST and not manifest.exists():
            continue
        data = tomllib.loads(manifest.read_text())
        refs += [
            Reference(
                name=entry["name"],
                url=entry["url"],
                ref=entry.get("ref"),
                notes=entry.get("notes", "").strip(),
                local=manifest is LOCAL_MANIFEST,
            )
            for entry in data.get("repo", [])
        ]
    names = [r.name for r in refs]
    if dupes := {n for n in names if names.count(n) > 1}:
        sys.exit(f"duplicate reference names across the manifests: {', '.join(sorted(dupes))}")
    return refs


def git(*args: str, cwd: Path) -> str:
    result = subprocess.run(
        ["git", *args], cwd=cwd, capture_output=True, text=True, check=False, env=GIT_ENV
    )
    if result.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed:\n{result.stderr.strip()}")
    return result.stdout.strip()


def git_remote(ref: Reference, *args: str, cwd: Path) -> str:
    """A git command that talks to the reference's upstream, anonymously at its own URL.
    Rewrite rules (`url.<base>.insteadOf`) are bypassed: one sending GitHub over SSH, say,
    would authenticate every connection, which costs seconds with a hardware-backed key.
    The longest matching rule wins, and none can be longer than the whole URL."""
    return git("-c", f"url.{ref.url}.insteadOf={ref.url}", *args, cwd=cwd)


def upstream_commit(ref: Reference) -> str:
    """The commit the reference's ref points to upstream now."""
    if ref.ref and COMMIT_ID.fullmatch(ref.ref):
        return ref.ref
    name = ref.ref or "HEAD"
    # An annotated tag's commit is only listed when asked for, as `<tag>^{}`.
    out = git_remote(ref, "ls-remote", ref.url, name, f"{name}^{{}}", cwd=ROOT)
    listed = dict(reversed(line.split("\t")) for line in out.splitlines())
    # A branch, else a tag (the commit an annotated one points to), else the exact name.
    for candidate in (f"refs/heads/{name}", f"refs/tags/{name}^{{}}", f"refs/tags/{name}", name):
        if candidate in listed:
            return listed[candidate]
    raise RuntimeError(f"{ref.url} has no ref {name!r}")


def local_commit(path: Path) -> str | None:
    """The checkout's commit: None if it's missing or its first fetch never completed."""
    if not (path / ".git").exists():
        return None
    result = subprocess.run(
        ["git", "rev-parse", "--verify", "--quiet", "HEAD"],
        cwd=path,
        capture_output=True,
        text=True,
        check=False,
    )
    return result.stdout.strip() if result.returncode == 0 else None


def is_dirty(path: Path) -> bool:
    return bool(git("status", "--porcelain", cwd=path))


def head_summary(path: Path) -> str:
    return git("log", "-1", "--format=%h %cs %s", cwd=path)


def sync(ref: Reference, force: bool) -> tuple[Reference, str, bool]:
    """Bring one reference up to date. Returns (ref, message, ok)."""
    try:
        have = local_commit(ref.path)
        if have is not None and have == upstream_commit(ref):
            return ref, f"up to date: {head_summary(ref.path)}", True

        if not (ref.path / ".git").exists():
            ref.path.mkdir(parents=True, exist_ok=True)
            git("init", "--quiet", cwd=ref.path)
            git("remote", "add", "origin", ref.url, cwd=ref.path)
        else:
            git("remote", "set-url", "origin", ref.url, cwd=ref.path)
            if have is not None and is_dirty(ref.path) and not force:
                return ref, "skipped: local modifications (use --force)", False

        # Just the new snapshot; objects the checkout already has aren't sent again.
        git_remote(
            ref,
            "fetch",
            "--quiet",
            "--depth",
            "1",
            "--no-tags",
            "origin",
            ref.ref or "HEAD",
            cwd=ref.path,
        )
        git("reset", "--quiet", "--hard", "FETCH_HEAD", cwd=ref.path)
        git("clean", "--quiet", "-fdx", cwd=ref.path)
        # No reflog, so the superseded snapshot is unreachable at once and gc can drop it.
        git("config", "core.logAllRefUpdates", "false", cwd=ref.path)
        git("reflog", "expire", "--expire=now", "--all", cwd=ref.path)
        return ref, f"{'cloned' if have is None else 'updated'}: {head_summary(ref.path)}", True
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
        local = "  (local)" if ref.local else ""
        print(f"{ref.name}  [{ref.url}{'@' + ref.ref if ref.ref else ''}]{local}")
        print(f"  status: {status}")
        for line in ref.notes.splitlines():
            print(f"  {line}")
        print()


def warn_unmanaged(refs: list[Reference]) -> None:
    known = {r.name for r in refs} | MANIFEST_FILES
    for entry in sorted(REFERENCES_DIR.iterdir()):
        if entry.name not in known:
            print(f"note: references/{entry.name} is in neither manifest", file=sys.stderr)


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

    # Mostly waiting on the network, so all at once; results print in order as they land.
    width = max(len(r.name) for r in refs)
    failed = False
    with ThreadPoolExecutor(max_workers=min(32, len(refs))) as pool:
        for ref, message, ok in pool.map(lambda r: sync(r, args.force), refs):
            print(f"{'ok ' if ok else 'ERR'} {ref.name:<{width}}  {message}", flush=True)
            failed |= not ok
    warn_unmanaged(load_manifest())
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
