"""Focused local packaging tests, without builds, network access or publication."""

import hashlib
import tarfile
import tempfile
import unittest
from pathlib import Path

from package import TARGETS, generate_notices, package_release


class ReleasePackagingTests(unittest.TestCase):
    def test_rejects_binaries_outside_updater_size_limit_before_creating_output(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "binary"
            for name in ("LICENSE-MIT", "LICENSE-APACHE"):
                (root / name).write_text(name)
            for size in (0, 64 * 1024 * 1024 + 1):
                with binary.open("wb") as fixture:
                    fixture.truncate(size)
                output = root / "dist"
                with self.assertRaisesRegex(ValueError, "Binary size"):
                    package_release(root, binary, output, TARGETS[0], "Fixture notices")
                self.assertFalse(output.exists())

    def test_all_targets_bundle_notices_and_matching_checksums(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "binary"
            binary.write_bytes(b"#!/bin/sh\nprintf fixture")
            for name in ("LICENSE-MIT", "LICENSE-APACHE"):
                (root / name).write_text(name)
            for target in TARGETS:
                output = root / target
                package_release(
                    root, binary, output, target, "Fixture dependency NOTICE"
                )
                for asset in output.iterdir():
                    if asset.suffix == ".sha256":
                        continue
                    digest, filename = (
                        asset.with_name(asset.name + ".sha256").read_text().split()
                    )
                    self.assertEqual(filename, asset.name)
                    self.assertEqual(
                        digest, hashlib.sha256(asset.read_bytes()).hexdigest()
                    )
                with tarfile.open(output / f"yoke-{target}.tar.gz") as archive:
                    self.assertEqual(
                        set(archive.getnames()),
                        {
                            "yoke",
                            "THIRD_PARTY_LICENSES",
                            "LICENSE-MIT",
                            "LICENSE-APACHE",
                        },
                    )
                    self.assertEqual(
                        archive.extractfile("THIRD_PARTY_LICENSES").read(),
                        b"Fixture dependency NOTICE",
                    )
                    self.assertEqual(archive.getmember("yoke").mode, 0o755)

    def test_graph_notices_include_nested_notice_and_fail_without_license(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "LICENSE").write_text("Fixture permissive license")
            (root / "vendor").mkdir()
            (root / "vendor" / "NOTICE").write_text("Fixture attribution")
            metadata = {
                "resolve": {"nodes": [{"id": "fixture"}]},
                "workspace_members": [],
                "packages": [
                    {
                        "id": "fixture",
                        "name": "fixture",
                        "version": "1.0.0",
                        "license": "MIT",
                        "manifest_path": str(root / "Cargo.toml"),
                    }
                ],
            }
            result = generate_notices(metadata)
            self.assertIn("Fixture permissive license", result)
            self.assertIn("Fixture attribution", result)
            (root / "LICENSE").unlink()
            (root / "vendor" / "NOTICE").unlink()
            with self.assertRaisesRegex(ValueError, "No license text"):
                generate_notices(metadata)


if __name__ == "__main__":
    unittest.main()
