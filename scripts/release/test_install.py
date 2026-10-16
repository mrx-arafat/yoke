"""Install script tests against a loopback mirror, without network access or the real home."""

import hashlib
import http.server
import json
import os
import platform
import stat
import subprocess
import sys
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
# Runs argv[1] under /bin/sh with a controlling terminal, typing argv[2] at every [y/N]
# question and Enter at every [Y/n] question. It runs in its own interpreter because forking
# a pseudo-terminal from the test process, which has server threads, is not safe.
TERMINAL_DRIVER = """
import json, os, pty, select, sys, time
answer = sys.argv[2].encode()
pid, fd = pty.fork()
if pid == 0:
    os.execv("/bin/sh", ["sh", sys.argv[1]])
output, answered = b"", 0
deadline = time.monotonic() + 60
while time.monotonic() < deadline:
    if not select.select([fd], [], [], 0.2)[0]:
        continue
    try:
        chunk = os.read(fd, 4096)
    except OSError:
        break
    if not chunk:
        break
    output += chunk
    if len(output) > answered and output.endswith(b"[y/N] "):
        os.write(fd, answer + b"\\n")
        answered = len(output)
    elif len(output) > answered and output.endswith(b"[Y/n] "):
        os.write(fd, b"\\n")
        answered = len(output)
_, status = os.waitpid(pid, 0)
print(json.dumps({"output": output.decode(errors="replace"), "status": os.waitstatus_to_exitcode(status)}))
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
# Answers the installer's statusline check the way `yoke install --statusline-if-absent
# --dry-run` does, according to $YOKE_TEST_STATUSLINE: absent, yoke (already Yoke's) or foreign.
STATUSLINE_YOKE = b"""#!/bin/sh
echo "$*" >> "$YOKE_TEST_LOG"
case $1 in
--version) echo 9.9.9 ;;
install)
    case " $* " in
    *" --statusline-if-absent --dry-run "*)
        case $YOKE_TEST_STATUSLINE in
        absent) echo "changes: 1" ;;
        yoke) echo "changes: 0" ;;
        foreign)
            echo "changes: 0"
            echo 'warning: "statusline kept: Claude already has a statusLine (`echo hi`); Yoke left it unchanged."' ;;
        esac ;;
    *) echo "changes: 1" ;;
    esac ;;
health) echo '  "/h/.yoke/config.toml",true,false' ;;
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

    def statusline_install(self, state: str, **extra: str) -> tuple[str, list[str]]:
        log = self.home / "yoke-calls.log"
        (self.mirror / self.name).write_bytes(STATUSLINE_YOKE)
        result = self.install(
            hashlib.sha256(STATUSLINE_YOKE).hexdigest(),
            YOKE_AGENTS="claude,codex",
            YOKE_YES="1",
            YOKE_TEST_LOG=str(log),
            YOKE_TEST_STATUSLINE=state,
            **extra,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout, log.read_text().splitlines()

    def terminal_install(self, state: str, answer: bytes) -> tuple[str, list[str]]:
        """Run install.sh with a controlling terminal; `answer` is typed at a [y/N] question
        and Enter at every [Y/n] question."""
        log = self.home / "yoke-calls.log"
        (self.mirror / self.name).write_bytes(STATUSLINE_YOKE)
        digest = hashlib.sha256(STATUSLINE_YOKE).hexdigest()
        (self.mirror / f"{self.name}.sha256").write_text(f"{digest}  {self.name}\n")
        env = {
            "PATH": os.environ["PATH"],
            "HOME": str(self.home),
            "TMPDIR": str(self.scratch),
            "YOKE_DOWNLOAD_URL": self.url,
            "YOKE_INSTALL_DIR": str(self.bin),
            "YOKE_AGENTS": "claude",
            "YOKE_NO_MODIFY_PATH": "1",
            "YOKE_TEST_LOG": str(log),
            "YOKE_TEST_STATUSLINE": state,
        }
        driven = subprocess.run(
            [sys.executable, "-c", TERMINAL_DRIVER, str(ROOT / "install.sh"), answer.decode()],
            env=env,
            capture_output=True,
            text=True,
            timeout=120,
        )
        self.assertEqual(driven.returncode, 0, driven.stderr)
        result = json.loads(driven.stdout)
        output = result["output"]
        self.assertEqual(result["status"], 0, output)
        return output, log.read_text().splitlines()

    def test_a_person_at_a_terminal_keeps_their_statusline_by_pressing_enter(
        self,
    ) -> None:
        output, calls = self.terminal_install("foreign", b"")

        self.assertIn("wrap it so Yoke's line comes first? ", output)
        self.assertNotIn("install --agent claude --statusline", calls)
        self.assertIn("keeping your statusline", output)
        self.assertNotIn("YOKE_YES", output.split("keeping your statusline")[1].split("\n")[0])

    def test_a_person_at_a_terminal_can_choose_to_wrap_their_statusline(self) -> None:
        output, calls = self.terminal_install("foreign", b"y")

        self.assertIn("install --agent claude --statusline", calls)
        self.assertIn("shows Yoke's line", output)

    def test_adds_yokes_statusline_when_claude_has_none_and_says_when_it_shows(
        self,
    ) -> None:
        stdout, calls = self.statusline_install("absent")

        self.assertIn("install --agent claude --statusline-if-absent", calls)
        self.assertIn("shows Yoke's line", stdout)
        self.assertIn("restart running sessions", stdout)
        self.assertIn("5-hour and weekly quota", stdout)
        self.assertIn("yoke usage", stdout)

    def test_yes_never_replaces_or_wraps_a_statusline_the_user_already_has(
        self,
    ) -> None:
        stdout, calls = self.statusline_install("foreign")

        # Only the dry run asked; nothing changed the user's command.
        self.assertEqual(
            [c for c in calls if "statusline" in c],
            ["install --agent claude --statusline-if-absent --dry-run"],
        )
        self.assertIn("keeping your statusline", stdout)
        self.assertIn("yoke install --agent claude --statusline", stdout)
        self.assertNotIn("shows Yoke's line", stdout)

    def test_a_statusline_that_is_already_yokes_is_left_alone(self) -> None:
        stdout, calls = self.statusline_install("yoke")

        self.assertEqual(
            [c for c in calls if "statusline" in c],
            ["install --agent claude --statusline-if-absent --dry-run"],
        )
        self.assertIn("already shows Yoke's line", stdout)

    def test_no_statusline_variable_skips_the_whole_step(self) -> None:
        stdout, calls = self.statusline_install("absent", YOKE_NO_STATUSLINE="1")

        self.assertEqual([c for c in calls if "statusline" in c], [])
        self.assertIn("YOKE_NO_STATUSLINE=1", stdout)


if __name__ == "__main__":
    unittest.main()
