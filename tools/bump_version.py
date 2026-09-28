#!/usr/bin/env python3
"""Set the workspace's version for a release, in Cargo.toml and Cargo.lock, and print it.

    tools/bump_version.py patch|minor|major   # from the current version
    tools/bump_version.py 0.3.0-rc.1          # exactly this (or the current one, unchanged)

A bump from a pre-release finishes it where it can: `patch` makes 0.3.0-rc.1 into
0.3.0. Trigger release (.github/workflows/trigger_release.yml) runs this, then commits,
tags and pushes.
"""

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "Cargo.toml"
SEMVER = re.compile(r"(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?")
# The `version` in `[workspace.package]`, which every crate inherits.
WORKSPACE_VERSION = re.compile(r'(\[workspace\.package\][^\[]*?\nversion = ")([^"]+)(")')


def bumped(current: str, how: str) -> str:
    if how not in ("patch", "minor", "major"):
        if not SEMVER.fullmatch(how):
            sys.exit(f"not a version or patch/minor/major: {how}")
        return how
    match = SEMVER.fullmatch(current)
    if not match:
        sys.exit(f"Cargo.toml's version isn't semver: {current}")
    major, minor, patch = map(int, match.groups()[:3])
    pre = match.group(4)
    if how == "major":
        return f"{major if pre and minor == patch == 0 else major + 1}.0.0"
    if how == "minor":
        return f"{major}.{minor if pre and patch == 0 else minor + 1}.0"
    return f"{major}.{minor}.{patch if pre else patch + 1}"


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    text = MANIFEST.read_text()
    match = WORKSPACE_VERSION.search(text)
    if not match:
        sys.exit("no version in Cargo.toml's [workspace.package]")
    version = bumped(match.group(2), sys.argv[1])
    # The current version, asked for by name, is released as it is (the first release).
    if version != match.group(2):
        MANIFEST.write_text(WORKSPACE_VERSION.sub(rf"\g<1>{version}\g<3>", text, count=1))
        # The lock file records each crate's version; update only the workspace's.
        subprocess.run(["cargo", "update", "--workspace", "--quiet"], cwd=ROOT, check=True)
    print(version)


if __name__ == "__main__":
    main()
