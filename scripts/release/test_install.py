"""Install script tests against a loopback mirror, without network access or the real home."""

import hashlib
import http.server
import os
import platform
import stat
import subprocess
import tempfile
import threading
import unittest
from functools import partial
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TARGETS = {
    ("Darwin", "arm64"): "aarch64-apple-darwin",
    ("Darwin", "x86_64"): "x86_64-apple-darwin",
    ("Linux", "x86_64"): "x86_64-unknown-linux-gnu",
    ("Linux", "aarch64"): "aarch64-unknown-linux-gnu",
}
FAKE_YOKE = b"#!/bin/sh\necho 9.9.9\n"


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass


class InstallScriptTests(unittest.TestCase):
    def setUp(self) -> None:
        target = TARGETS.get((platform.system(), platform.machine()))
        if target is None:
            self.skipTest("install.sh supports no release target for this host")
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        self.mirror = root / "mirror"
        self.home = root / "home"
        self.bin = root / "bin"
        self.scratch = root / "scratch"
        for path in (self.mirror, self.home, self.scratch):
            path.mkdir()
        self.name = f"yoke-{target}"
        (self.mirror / self.name).write_bytes(FAKE_YOKE)
        server = http.server.ThreadingHTTPServer(
            ("127.0.0.1", 0), partial(QuietHandler, directory=str(self.mirror))
        )
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        self.url = f"http://127.0.0.1:{server.server_address[1]}"

    def install(self, digest: str) -> subprocess.CompletedProcess[str]:
        (self.mirror / f"{self.name}.sha256").write_text(f"{digest}  {self.name}\n")
        env = {
            "PATH": os.environ["PATH"],
            "HOME": str(self.home),
            "TMPDIR": str(self.scratch),
            "YOKE_DOWNLOAD_URL": self.url,
            "YOKE_INSTALL_DIR": str(self.bin),
            "YOKE_AGENTS": "none",
        }
        # A new session has no controlling terminal, so no question can wait for input.
        return subprocess.run(
            ["sh", str(ROOT / "install.sh")],
            env=env,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=60,
            start_new_session=True,
        )

    def test_installs_release_with_valid_checksum(self) -> None:
        result = self.install(hashlib.sha256(FAKE_YOKE).hexdigest())

        self.assertEqual(result.returncode, 0, result.stderr)
        installed = self.bin / "yoke"
        self.assertEqual(installed.read_bytes(), FAKE_YOKE)
        self.assertEqual(stat.S_IMODE(installed.stat().st_mode), 0o755)
        self.assertIn(
            f"yoke-install: installed Yoke 9.9.9 at {installed}", result.stdout
        )
        self.assertEqual(list(self.scratch.iterdir()), [])

    def test_refuses_checksum_mismatch_and_keeps_existing_binary(self) -> None:
        self.bin.mkdir()
        existing = self.bin / "yoke"
        existing.write_bytes(b"previous release")

        result = self.install("0" * 64)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum mismatch", result.stderr)
        self.assertEqual(list(self.bin.iterdir()), [existing])
        self.assertEqual(existing.read_bytes(), b"previous release")
        self.assertEqual(list(self.scratch.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
