#!/usr/bin/env python3
"""Build and package screenie for release, as CI does.

Builds the `dist` profile for this machine and writes, in .cache/dist/:

    screenie-<version>-<target>.tar.gz          the archive
    screenie-<version>-<target>.tar.gz.sha256   its checksum, as sha256sum prints it

The archive holds one directory with bin/screenie, the man pages in share/man/man1
(from `screenie man`), README.md and LICENSE. mise's github backend strips that
directory and puts bin/ on PATH, and man finds share/man beside it. The archive is
reproducible: entries are sorted, owned by root and dated to the commit.

    tools/dist.py
"""

import gzip
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / ".cache/dist"


def run(*args: str) -> str:
    return subprocess.run(args, cwd=ROOT, check=True, capture_output=True, text=True).stdout


def main() -> None:
    subprocess.run(
        ["cargo", "build", "--profile", "dist", "--locked", "-p", "screenie"], cwd=ROOT, check=True
    )

    metadata = json.loads(run("cargo", "metadata", "--no-deps", "--format-version", "1"))
    version = next(p["version"] for p in metadata["packages"] if p["name"] == "screenie")
    target = next(
        line.split()[1] for line in run("rustc", "-vV").splitlines() if line.startswith("host:")
    )
    binary = Path(metadata["target_directory"]) / "dist/screenie"
    name = f"screenie-{version}-{target}"
    mtime = int(run("git", "log", "-1", "--format=%ct"))

    with tempfile.TemporaryDirectory() as tmp:
        stage = Path(tmp) / name
        (stage / "bin").mkdir(parents=True)
        shutil.copy2(binary, stage / "bin/screenie")
        subprocess.run([str(binary), "man", str(stage / "share/man")], check=True)
        for doc in ("README.md", "LICENSE"):
            shutil.copy2(ROOT / doc, stage / doc)

        OUT.mkdir(parents=True, exist_ok=True)
        archive = OUT / f"{name}.tar.gz"
        with (
            archive.open("wb") as file,
            gzip.GzipFile(filename="", fileobj=file, mode="wb", mtime=mtime) as gz,
            tarfile.open(fileobj=gz, mode="w", format=tarfile.PAX_FORMAT) as tar,
        ):
            for path in [stage, *sorted(stage.rglob("*"))]:
                add(tar, path, path.relative_to(tmp), mtime)

    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_name(archive.name + ".sha256").write_text(f"{digest}  {archive.name}\n")
    print(archive.relative_to(ROOT))


def add(tar: tarfile.TarFile, path: Path, name: Path, mtime: int) -> None:
    info = tar.gettarinfo(path, arcname=str(name))
    info.uid = info.gid = 0
    info.uname = info.gname = ""
    info.mtime = mtime
    info.mode = 0o755 if path.is_dir() or os.access(path, os.X_OK) else 0o644
    if path.is_file():
        with path.open("rb") as file:
            tar.addfile(info, file)
    else:
        tar.addfile(info)


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as e:
        sys.exit(e.returncode)
