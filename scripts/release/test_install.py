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
# Logs every call to $YOKE_TEST_LOG, so a test can see what the installer ran.
RECORDING_YOKE = b"""#!/bin/sh
echo "$*" >> "$YOKE_TEST_LOG"
case $1 in
--version) echo 9.9.9 ;;
install) echo "changes: 0" ;;
health) echo '  "/h/.yoke/config.toml",false,false' ;;
esac
"""
# Reports a database from a newer Yoke the way `yoke health` does, and logs every call.
NEWER_STATE_YOKE = b"""#!/bin/sh
echo "$*" >> "$YOKE_TEST_LOG"
case $1 in
--version) echo 9.9.9 ;;
health) echo '  "cannot read /h/.yoke/state.db: state database schema 99 is newer than supported 12"' ;;
esac
"""
# Refuses the first install the way a moved executable does, then accepts it after `uninstall`.
MOVED_YOKE = b"""#!/bin/sh
echo "$*" >> "$YOKE_TEST_LOG"
case $1 in
--version) echo 9.9.9 ;;
install)
    if [ -f "$YOKE_TEST_MOVED" ]; then echo "changes: 3"; else
        echo "yoke: executable changed; uninstall before reinstalling: the claude hooks run $YOKE_TEST_OLD, not $0; run it" >&2
        exit 1
    fi ;;
uninstall) : > "$YOKE_TEST_MOVED"; echo "changes: 3" ;;
health) echo '  "/h/.yoke/config.toml",false,false' ;;
esac
"""


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

    def install(self, digest: str, **extra: str) -> subprocess.CompletedProcess[str]:
        (self.mirror / f"{self.name}.sha256").write_text(f"{digest}  {self.name}\n")
        env = {
            "PATH": os.environ["PATH"],
            "HOME": str(self.home),
            "TMPDIR": str(self.scratch),
            "YOKE_DOWNLOAD_URL": self.url,
            "YOKE_INSTALL_DIR": str(self.bin),
            "YOKE_AGENTS": "none",
            **extra,
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

    def test_yes_never_trusts_a_policy_without_a_terminal(self) -> None:
        log = self.home / "yoke-calls.log"
        (self.mirror / self.name).write_bytes(RECORDING_YOKE)

        result = self.install(
            hashlib.sha256(RECORDING_YOKE).hexdigest(),
            YOKE_AGENTS="claude",
            YOKE_YES="1",
            YOKE_TEST_LOG=str(log),
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        calls = log.read_text().splitlines()
        self.assertIn("install --agent claude", calls)
        self.assertEqual([c for c in calls if c.startswith(("trust", "preset"))], [])
        self.assertIn("yoke trust --global", result.stdout)

    def test_refuses_a_newer_state_database_before_replacing_anything(self) -> None:
        self.bin.mkdir()
        existing = self.bin / "yoke"
        existing.write_bytes(b"previous release")
        log = self.home / "yoke-calls.log"
        (self.mirror / self.name).write_bytes(NEWER_STATE_YOKE)

        result = self.install(
            hashlib.sha256(NEWER_STATE_YOKE).hexdigest(),
            YOKE_AGENTS="claude",
            YOKE_TEST_LOG=str(log),
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("newer than supported", result.stderr)
        self.assertIn("nothing was changed", result.stderr)
        self.assertEqual(existing.read_bytes(), b"previous release")
        self.assertNotIn("install --agent claude", log.read_text())

    def test_moving_the_hooks_names_the_old_executable_left_behind(self) -> None:
        old = self.home / "old-yoke"
        old.write_bytes(b"old")
        log = self.home / "yoke-calls.log"
        (self.mirror / self.name).write_bytes(MOVED_YOKE)

        result = self.install(
            hashlib.sha256(MOVED_YOKE).hexdigest(),
            YOKE_AGENTS="claude",
            YOKE_TEST_LOG=str(log),
            YOKE_TEST_MOVED=str(self.home / "moved"),
            YOKE_TEST_OLD=str(old),
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"still at {old}", result.stdout)
        self.assertIn(f"rm -f '{old}'", result.stdout)
        self.assertIn("records hook events", result.stdout)


if __name__ == "__main__":
    unittest.main()
