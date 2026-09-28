#!/usr/bin/env python3
"""Describe the release archives for their packslip (https://packslip.dev).

    tools/packslip.py ARCHIVE... > release.toml

Prints a `packslip create --manifest` with what packslip doesn't read from the archives
by itself, taken from what's in them:

- the executable, `screenie`, which packslip finds in each archive;
- each archive's glibc floor, the newest `GLIBC_` symbol version its screenie needs
  (packslip reads the libraries it loads, but not that);
- each archive's man pages and shell completions, as resources: mise installs only
  declared ones. packslip doesn't check that a resource's path exists, so they're
  listed, not written;
- `screenie completions` for every shell, for those without a script in the archive.

The Packslip workflow runs it on tools/dist.py's archives. To look at the result locally:

    tools/dist.py && tools/packslip.py .cache/dist/*.tar.gz > .cache/release.toml
    packslip keygen --out .cache/packslip.key
    packslip create --key .cache/packslip.key --no-log --out .cache \\
      --project github.com/johnpyp/screenie --version VERSION \\
      --manifest .cache/release.toml .cache/dist/*.tar.gz
    packslip show .cache/packslip.sigstore.json
"""

import json
import re
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path, PurePosixPath

BIN = "screenie"
# The shell each completion script is for, by where it is in an archive.
COMPLETIONS = {
    "bash": "share/bash-completion/completions/*",
    "zsh": "share/zsh/site-functions/_*",
    "fish": "share/fish/vendor_completions.d/*.fish",
}
# What `screenie completions` takes (crates/screenie/src/completions.rs).
SHELLS = ["bash", "elvish", "fish", "nushell", "powershell", "zsh"]


def main(archives: list[str]) -> None:
    if not archives:
        sys.exit(__doc__)
    print(f"bin = [{json.dumps(BIN)}]")
    for archive in archives:
        name = Path(archive).name
        with tarfile.open(archive) as tar:
            members = [m for m in tar.getmembers() if m.isfile()]
            files = sorted(m.name for m in members)
            binaries = [m for m in members if matches(m.name, f"bin/{BIN}")]
            if len(binaries) != 1:
                sys.exit(f"{archive} has {len(binaries)} bin/{BIN}, not one")
            glibc = glibc_min(tar.extractfile(binaries[0]).read())
            if not glibc:
                sys.exit(f"{archive}'s {BIN} needs no versioned glibc symbols")

        print(f"\n[[artifact]]\npath = {json.dumps(archive)}")
        print(f'requires = {{ glibc_min = "{glibc}" }}')
        for page in (f for f in files if matches(f, "share/man/*/*")):
            resource(kind="man", artifact=name, archive=page)
        for shell, pattern in COMPLETIONS.items():
            for script in (f for f in files if matches(f, pattern)):
                resource(kind="completion", shell=shell, artifact=name, archive=script)

    # Consumers take a script from the archive first.
    resource(kind="completion", shells=SHELLS, exec=[BIN, "completions", "{shell}"])


def matches(path: str, pattern: str) -> bool:
    """Whether `path`, under the archive's top-level directory, is `pattern`."""
    return PurePosixPath(path).match(f"*/{pattern}")


def resource(**fields: str | list[str]) -> None:
    print("\n[[resource]]")
    for key, value in fields.items():
        print(f"{key} = {json.dumps(value)}")


def glibc_min(binary: bytes) -> str | None:
    """The newest glibc symbol version an executable needs, as `2.35`."""
    with tempfile.NamedTemporaryFile() as file:
        file.write(binary)
        file.flush()
        needs = subprocess.run(
            ["readelf", "--wide", "--version-info", file.name],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    versions = [tuple(map(int, v.split("."))) for v in re.findall(r"GLIBC_(\d+(?:\.\d+)+)", needs)]
    return ".".join(map(str, max(versions))) if versions else None


if __name__ == "__main__":
    main(sys.argv[1:])
