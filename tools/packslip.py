#!/usr/bin/env python3
"""Describe the release archives for their packslip (https://packslip.dev).

    tools/packslip.py ARCHIVE... > release.toml

Prints a `packslip create --manifest` with what packslip doesn't read from the archives
by itself, taken from what's in them:

- the executable, `screenie`, which packslip finds in each archive;
- each archive's glibc floor, the newest `GLIBC_` symbol version its screenie needs
  (packslip reads the libraries it loads, but not that);
- each archive's man pages, as resources: mise puts only declared ones on MANPATH.
  packslip doesn't check that a resource's path exists, so they're listed, not written.

CI's packslip job runs it on tools/dist.py's archives. To look at the result locally:

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


def main(archives: list[str]) -> None:
    if not archives:
        sys.exit(__doc__)
    print(f"bin = [{json.dumps(BIN)}]")
    for archive in archives:
        name = Path(archive).name
        with tarfile.open(archive) as tar:
            files = [m for m in tar.getmembers() if m.isfile()]
            binaries = [m for m in files if PurePosixPath(m.name).match(f"*/bin/{BIN}")]
            if len(binaries) != 1:
                sys.exit(f"{archive} has {len(binaries)} bin/{BIN}, not one")
            glibc = glibc_min(tar.extractfile(binaries[0]).read())
            if not glibc:
                sys.exit(f"{archive}'s {BIN} needs no versioned glibc symbols")
            pages = sorted(m.name for m in files if PurePosixPath(m.name).match("share/man/*/*"))

        print(f"\n[[artifact]]\npath = {json.dumps(archive)}")
        print(f'requires = {{ glibc_min = "{glibc}" }}')
        for page in pages:
            print(f'\n[[resource]]\nkind = "man"\nartifact = {json.dumps(name)}')
            print(f"archive = {json.dumps(page)}")


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
