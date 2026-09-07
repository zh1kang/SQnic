#!/usr/bin/env python3
"""Package one built SQnic binary and write a checksum sidecar."""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import tarfile
from typing import BinaryIO
import zipfile
from pathlib import Path


TARGETS = {
    "aarch64-apple-darwin": ("darwin", "arm64", "tar.gz"),
    "x86_64-apple-darwin": ("darwin", "x86_64", "tar.gz"),
    "x86_64-unknown-linux-gnu": ("linux", "x86_64", "tar.gz"),
    "x86_64-pc-windows-msvc": ("windows", "x86_64", "zip"),
}
VERSION_RE = re.compile(r"\A\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?\Z")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--target", choices=sorted(TARGETS), required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    return parser.parse_args()


def archive_name(version: str, target: str) -> tuple[str, str]:
    platform, architecture, archive_format = TARGETS[target]
    stem = f"sqnic-v{version}-{platform}-{architecture}"
    return stem, f"{stem}.{archive_format}"


def write_tar(binary: Path, stream: BinaryIO, member_name: str) -> None:
    with tarfile.open(fileobj=stream, mode="w:gz") as archive:
        info = tarfile.TarInfo(member_name)
        info.size = binary.stat().st_size
        info.mode = 0o755
        info.mtime = 0
        with binary.open("rb") as source:
            archive.addfile(info, source)


def write_zip(binary: Path, stream: BinaryIO, member_name: str) -> None:
    with zipfile.ZipFile(stream, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        info = zipfile.ZipInfo(member_name, date_time=(1980, 1, 1, 0, 0, 0))
        info.create_system = 3
        info.external_attr = 0o755 << 16
        archive.writestr(info, binary.read_bytes())


def exclusive_binary(path: Path) -> BinaryIO:
    try:
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
    except FileExistsError as error:
        raise SystemExit(f"refusing to replace existing artifact: {path}") from error
    return os.fdopen(descriptor, "wb")


def reject_existing(path: Path) -> None:
    if os.path.lexists(path):
        raise SystemExit(f"refusing to replace existing artifact: {path}")


def main() -> None:
    args = parse_args()
    if not VERSION_RE.fullmatch(args.version):
        raise SystemExit(f"invalid version: {args.version!r}")
    binary = args.binary.resolve()
    if not binary.is_file():
        raise SystemExit(f"binary does not exist: {binary}")
    stem, filename = archive_name(args.version, args.target)
    if args.output_dir.is_symlink():
        raise SystemExit(f"refusing symlink output directory: {args.output_dir}")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    destination = args.output_dir / filename
    checksum = args.output_dir / f"{filename}.sha256"
    reject_existing(destination)
    reject_existing(checksum)
    member_name = "sqnic.exe" if args.target.endswith("windows-msvc") else "sqnic"
    with exclusive_binary(destination) as stream:
        if filename.endswith(".zip"):
            write_zip(binary, stream, member_name)
        else:
            write_tar(binary, stream, member_name)
    digest = hashlib.sha256(destination.read_bytes()).hexdigest()
    with exclusive_binary(checksum) as stream:
        stream.write(f"{digest}  {filename}\n".encode())
    print(f"{destination} ({stem})")


if __name__ == "__main__":
    main()
