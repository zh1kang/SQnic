from __future__ import annotations

import hashlib
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PACKAGER = ROOT / "scripts" / "package_release.py"


class PackageReleaseTest(unittest.TestCase):
    def package(self, target: str, binary_name: str = "sqnic") -> tuple[Path, Path, Path]:
        root = Path(tempfile.mkdtemp(prefix="sqnic-package-test-"))
        binary = root / binary_name
        binary.write_bytes(b"fixture binary\n")
        output = root / "dist"
        subprocess.run(
            [
                sys.executable,
                str(PACKAGER),
                "--binary",
                str(binary),
                "--version",
                "0.1.0",
                "--target",
                target,
                "--output-dir",
                str(output),
            ],
            check=True,
        )
        archive = next(output.glob("*.tar.gz"), None) or next(output.glob("*.zip"))
        checksum = Path(f"{archive}.sha256")
        return root, archive, checksum

    def test_tar_member_and_checksum(self) -> None:
        root, archive, checksum = self.package("aarch64-apple-darwin")
        self.addCleanup(lambda: shutil.rmtree(root))
        with tarfile.open(archive, "r:gz") as handle:
            self.assertEqual(handle.getnames(), ["sqnic"])
            self.assertEqual(handle.extractfile("sqnic").read(), b"fixture binary\n")
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        self.assertEqual(checksum.read_text(), f"{digest}  {archive.name}\n")

    def test_zip_member_and_checksum(self) -> None:
        root, archive, checksum = self.package(
            "x86_64-pc-windows-msvc", "sqnic.exe"
        )
        self.addCleanup(lambda: shutil.rmtree(root))
        with zipfile.ZipFile(archive) as handle:
            self.assertEqual(handle.namelist(), ["sqnic.exe"])
            self.assertEqual(handle.read("sqnic.exe"), b"fixture binary\n")
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        self.assertEqual(checksum.read_text(), f"{digest}  {archive.name}\n")

    def test_repeated_command_preserves_existing_artifacts(self) -> None:
        root, archive, checksum = self.package("aarch64-apple-darwin")
        self.addCleanup(lambda: shutil.rmtree(root))
        before = archive.read_bytes(), checksum.read_bytes()
        result = subprocess.run(
            [
                sys.executable,
                str(PACKAGER),
                "--binary",
                str(root / "sqnic"),
                "--version",
                "0.1.0",
                "--target",
                "aarch64-apple-darwin",
                "--output-dir",
                str(root / "dist"),
            ],
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((archive.read_bytes(), checksum.read_bytes()), before)

    @unittest.skipUnless(os.name == "posix", "symlink fixtures need Unix privileges")
    def test_existing_and_dangling_symlink_destinations_are_rejected(self) -> None:
        root, archive, checksum = self.package("aarch64-apple-darwin")
        self.addCleanup(lambda: shutil.rmtree(root))
        archive.unlink()
        archive.symlink_to(root / "outside")
        result = subprocess.run(
            [
                sys.executable,
                str(PACKAGER),
                "--binary",
                str(root / "sqnic"),
                "--version",
                "0.1.0",
                "--target",
                "aarch64-apple-darwin",
                "--output-dir",
                str(root / "dist"),
            ],
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(archive.is_symlink())
        self.assertFalse((root / "outside").exists())


if __name__ == "__main__":
    unittest.main()
