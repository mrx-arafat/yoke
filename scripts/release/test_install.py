"""Install script tests against a loopback mirror, without network access or the real home."""

import hashlib
import http.server
import json
import os
import platform
import shutil
import signal
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
# Sends the installer's requests for api.github.com to the loopback mirror, under /api/, and lets
# real curl make every request, so the headers the installer sends are the real ones.
CURL_SHIM = """#!/bin/sh
count=$#
i=1
while [ "$i" -lt "$count" ]; do
    set -- "$@" "$1"
    shift
    i=$((i + 1))
done
last=$1
shift
echo "$* $last" >> "$YOKE_TEST_CURL_LOG"
case $last in
https://api.github.com/repos/mrx-arafat/yoke/*)
    last="$YOKE_TEST_MIRROR/api/${last#https://api.github.com/repos/mrx-arafat/yoke/}"
    set -- "$@" --proto '=http,https' ;;
esac
case $last in
"$YOKE_TEST_MIRROR"/*) ;;
*) echo "test refused non-mirror request: $last" >&2; exit 1 ;;
esac
exec "$YOKE_TEST_REAL_CURL" "$@" "$last"
"""
# Answers `git ls-remote` with $YOKE_TEST_GIT_SHA, the way a reachable GitHub would, or fails when
# it is empty; it is always on PATH so that a real git never reaches the network from a test.
GIT_SHIM = """#!/bin/sh
[ -n "$YOKE_TEST_GIT_SHA" ] || exit 1
printf '%s\\trefs/heads/main\\n' "$YOKE_TEST_GIT_SHA"
"""
# Records its arguments and builds a fake executable into the --root it was given.
CARGO_SHIM = """#!/bin/sh
echo "$*" >> "$YOKE_TEST_CARGO_LOG"
while [ $# -gt 0 ]; do
    if [ "$1" = --root ]; then root=$2; fi
    shift
done
mkdir -p "$root/bin"
printf '#!/bin/sh\\necho 9.9.9\\n' > "$root/bin/yoke"
chmod 755 "$root/bin/yoke"
"""
RELEASE_JSON = json.dumps({"tag_name": "v1.2.3", "draft": False, "prerelease": False})


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass

    def do_GET(self) -> None:
        # /api/... stands in for api.github.com: the test sets what each path answers and reads
        # back what was asked, including the Accept header.
        if not self.path.startswith("/api/"):
            super().do_GET()
            return
        self.server.requests.append((self.path, self.headers.get("Accept")))  # type: ignore[attr-defined]
        code, body = self.server.canned.get(self.path, (404, ""))  # type: ignore[attr-defined]
        payload = body.encode()
        self.send_response(code)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


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
        server.canned = {}  # type: ignore[attr-defined]
        server.requests = []  # type: ignore[attr-defined]
        self.server = server
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        self.url = f"http://127.0.0.1:{server.server_address[1]}"

    def install(self, digest: str, **extra: str) -> subprocess.CompletedProcess[str]:
        manifest = extra.pop("YOKE_TEST_MANIFEST", f"{digest}  {self.name}\n")
        if manifest != "missing":
            (self.mirror / "SHA256SUMS").write_text(manifest)
        env = {
            **self.shimmed(),
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

    def shimmed(self, git_sha: str | None = None) -> dict[str, str]:
        """An environment whose curl reaches GitHub's API through the mirror and whose cargo
        builds a fake executable, so the source build's choice of revision can be read from
        its log. A `git` on PATH answers `ls-remote` with `git_sha`, or fails when it is None."""
        shims = self.home / "shims"
        shims.mkdir(exist_ok=True)
        for name, body in (
            ("curl", CURL_SHIM),
            ("cargo", CARGO_SHIM),
            ("cc", "#!/bin/sh\n"),
            ("git", GIT_SHIM),
        ):
            (shims / name).write_text(body)
            (shims / name).chmod(0o755)
        return {
            "PATH": f"{shims}{os.pathsep}{os.environ['PATH']}",
            "YOKE_TEST_REAL_CURL": shutil.which("curl") or "curl",
            "YOKE_TEST_MIRROR": self.url,
            "YOKE_TEST_CURL_LOG": str(self.home / "curl.log"),
            "YOKE_TEST_CARGO_LOG": str(self.home / "cargo.log"),
            "YOKE_TEST_GIT_SHA": git_sha or "",
        }

    def cargo_calls(self) -> list[str]:
        log = self.home / "cargo.log"
        return log.read_text().splitlines() if log.exists() else []

    def test_source_build_uses_one_exact_revision_resolved_at_install_time(
        self,
    ) -> None:
        self.assertNotIn(
            "PINNED_COMMIT",
            (ROOT / "install.sh").read_text(),
            "a hard-coded commit goes stale; the installer resolves main when it runs",
        )
        main = "b" * 40
        elsewhere = "c" * 40
        explicit = "a" * 40
        no_release = (404, "")
        release = (200, RELEASE_JSON)
        # (label, answers by API path, extra env, git sha, expected cargo flag or None)
        cases = [
            ("newest release", {"releases/latest": release}, {}, None, "--tag v1.2.3"),
            (
                "no release yet builds main as it is now",
                {"releases/latest": no_release, "commits/main": (200, main + "\n")},
                {},
                None,
                f"--rev {main}",
            ),
            (
                "explicit version",
                {"releases/latest": (500, "")},
                {"YOKE_VERSION": "v9.9.9"},
                None,
                "--tag v9.9.9",
            ),
            (
                "explicit commit overrides main and asks nothing",
                {"releases/latest": no_release, "commits/main": (200, main)},
                {"YOKE_COMMIT": explicit},
                None,
                f"--rev {explicit}",
            ),
            (
                "git answers when the API does not",
                {"releases/latest": no_release, "commits/main": (500, "")},
                {},
                elsewhere,
                f"--rev {elsewhere}",
            ),
            ("release lookup failed", {"releases/latest": (500, "")}, {}, None, None),
            (
                "garbage tag",
                {"releases/latest": (200, '{"tag_name": "main"}')},
                {},
                None,
                None,
            ),
            (
                "main lookup failed",
                {"releases/latest": no_release, "commits/main": (500, "")},
                {},
                None,
                None,
            ),
            (
                "main lookup answered garbage",
                {"releases/latest": no_release, "commits/main": (200, "<html>")},
                {},
                None,
                None,
            ),
            (
                "neither API nor git answers",
                {"releases/latest": no_release, "commits/main": (500, "")},
                {},
                "not-a-commit",
                None,
            ),
        ]
        for label, answers, extra, git_sha, expected in cases:
            with self.subTest(label):
                (self.home / "cargo.log").unlink(missing_ok=True)
                (self.mirror / self.name).unlink(missing_ok=True)
                self.server.canned = {f"/api/{path}": a for path, a in answers.items()}
                self.server.requests.clear()
                result = self.install(
                    hashlib.sha256(FAKE_YOKE).hexdigest(),
                    **self.shimmed(git_sha),
                    **extra,
                )
                calls = self.cargo_calls()
                asked = [path for path, _ in self.server.requests]
                if expected is not None:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(len(calls), 1, calls)
                    self.assertIn(expected, calls[0])
                    self.assertNotIn("--branch", calls[0])
                    self.assertIn(expected.split()[-1], result.stdout)
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(calls, [])
                    self.assertIn("YOKE_", result.stderr)
                if "YOKE_COMMIT" in extra or "YOKE_VERSION" in extra:
                    self.assertEqual(asked, [], "an explicit revision needs no lookup")
                if label == "no release yet builds main as it is now":
                    # The sha media type returns the bare hash, never the commit JSON.
                    self.assertEqual(
                        self.server.requests[-1],
                        ("/api/commits/main", "application/vnd.github.sha"),
                    )
                if label == "main lookup failed":
                    # Nothing is built and the message gives the command to run instead.
                    self.assertIn(
                        "YOKE_COMMIT=<40-character commit hash>", result.stderr
                    )
                    self.assertIn("bash", result.stderr)
                    # The failed request is named by its URL, not by curl's options.
                    self.assertIn(
                        "HTTP 500 from https://api.github.com/repos/mrx-arafat/yoke/commits/main",
                        result.stderr,
                    )

    def test_downloads_use_tls_1_2_and_size_and_time_limits(self) -> None:
        result = self.install(hashlib.sha256(FAKE_YOKE).hexdigest(), **self.shimmed())

        self.assertEqual(result.returncode, 0, result.stderr)
        downloads = [
            line
            for line in (self.home / "curl.log").read_text().splitlines()
            if self.name in line or "SHA256SUMS" in line
        ]
        self.assertEqual(len(downloads), 2, downloads)
        for line in downloads:
            for flag in ("--tlsv1.2", "--max-time", "--max-filesize", "--proto"):
                self.assertIn(flag, line)

    def test_refuses_a_plain_http_mirror_unless_it_is_loopback(self) -> None:
        for url in (
            "http://mirror.example.invalid/yoke",
            "http://127.0.0.1.example.invalid/yoke",
            "http://127.0.0.1:80@example.invalid/yoke",
            "ftp://127.0.0.1/yoke",
        ):
            with self.subTest(url):
                result = self.install(
                    hashlib.sha256(FAKE_YOKE).hexdigest(), YOKE_DOWNLOAD_URL=url
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("YOKE_DOWNLOAD_URL", result.stderr)
                self.assertFalse((self.bin / "yoke").exists())

    def test_refuses_an_install_directory_that_would_break_the_path_line(self) -> None:
        for name in ('a"b', "a$HOME", "a`b", "a\\b", "a\nb"):
            with self.subTest(name):
                result = self.install(
                    hashlib.sha256(FAKE_YOKE).hexdigest(),
                    YOKE_INSTALL_DIR=str(self.home / name),
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("YOKE_INSTALL_DIR", result.stderr)
                self.assertFalse((self.home / name).exists())

    def test_install_binary_refuses_invalid_candidates_and_stages_before_execution(
        self,
    ) -> None:
        source = (ROOT / "install.sh").read_text()
        function = source.split("install_binary() {", 1)[1].split("\n}", 1)[0]
        executable = self.home / "executable"
        executable.write_bytes(RECORDING_YOKE)
        executable.chmod(0o755)
        script = (
            """set -eu
candidate=$1; bin=$2; dir=$3; staged=
test -n "$HOME" && test "$HOME" != "$4" && test -d "$HOME"
trap 'if [ -n "$staged" ]; then rm -f "$staged"; fi' EXIT
die() { printf '%s\\n' "$*" >&2; exit 1; }
check_state() { :; }
chmod() {
    command chmod "$@"
    if [ "$YOKE_TEST_STAGE_LINK" = 1 ]; then
        rm -f "$2"
        ln -s "$candidate" "$2"
    fi
}
install_binary() {"""
            + function
            + "\n}\ninstall_binary\n"
        )
        for kind in (
            "missing",
            "directory",
            "fifo",
            "non-executable",
            "symlink",
            "staged-link",
            "valid",
        ):
            with self.subTest(kind=kind):
                case = self.home / kind
                case.mkdir()
                candidate = case / "candidate"
                log = case / "executed.log"
                directory = case / "bin"
                directory.mkdir()
                installed = directory / "yoke"
                installed.write_bytes(b"previous release")
                if kind == "directory":
                    candidate.mkdir()
                elif kind == "fifo":
                    os.mkfifo(candidate)
                elif kind == "symlink":
                    candidate.symlink_to(executable)
                elif kind != "missing":
                    candidate.write_bytes(RECORDING_YOKE)
                    candidate.chmod(0o644 if kind == "non-executable" else 0o755)
                env = {
                    **os.environ,
                    "HOME": str(self.home),
                    "YOKE_TEST_LOG": str(log),
                    "YOKE_TEST_STAGE_LINK": "1" if kind == "staged-link" else "0",
                }
                env.pop("CLAUDE_CONFIG_DIR", None)
                env.pop("CODEX_HOME", None)
                process = subprocess.Popen(
                    [
                        "sh",
                        "-c",
                        script,
                        "install-binary-test",
                        str(candidate),
                        str(installed),
                        str(directory),
                        os.environ["HOME"],
                    ],
                    env=env,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                    start_new_session=True,
                )
                try:
                    _, stderr = process.communicate(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.communicate()
                    self.fail(
                        f"{kind}: installer blocked instead of refusing the candidate"
                    )
                if kind == "valid":
                    self.assertEqual(process.returncode, 0, stderr)
                    self.assertEqual(installed.read_bytes(), RECORDING_YOKE)
                    self.assertEqual(log.read_text().splitlines(), ["--version"])
                else:
                    self.assertNotEqual(process.returncode, 0, stderr)
                    self.assertIn("regular executable file", stderr)
                    self.assertEqual(installed.read_bytes(), b"previous release")
                    self.assertFalse(log.exists(), "invalid executable was run")
                self.assertEqual(list(directory.iterdir()), [installed])

    def test_installs_release_with_valid_checksum(self) -> None:
        digest = hashlib.sha256(FAKE_YOKE).hexdigest()
        result = self.install(
            digest,
            YOKE_TEST_MANIFEST=f"{digest}  unrelated.tar.gz\n{digest}  {self.name}\n{digest}  unrelated.sha256\n",
        )

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

    def test_refuses_missing_duplicate_or_malformed_manifest_without_replacement(
        self,
    ) -> None:
        digest = hashlib.sha256(FAKE_YOKE).hexdigest()
        entry = f"{digest}  {self.name}\n"
        self.bin.mkdir()
        existing = self.bin / "yoke"
        existing.write_bytes(b"previous release")
        for manifest in (
            "missing",
            "",
            f"{digest}  other\n",
            entry * 2,
            "bad\n" + entry,
            "x" * 65537,
        ):
            with self.subTest(manifest=manifest[:80]):
                (self.mirror / "SHA256SUMS").unlink(missing_ok=True)
                result = self.install(digest, YOKE_TEST_MANIFEST=manifest)
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertIn("SHA256SUMS", result.stderr)
                self.assertIn("YOKE_VERSION", result.stderr)
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
        self.assertIn("log --all --limit 1", calls)
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
        (self.mirror / "SHA256SUMS").write_text(f"{digest}  {self.name}\n")
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
            [
                sys.executable,
                "-c",
                TERMINAL_DRIVER,
                str(ROOT / "install.sh"),
                answer.decode(),
            ],
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
        self.assertNotIn(
            "YOKE_YES", output.split("keeping your statusline")[1].split("\n")[0]
        )

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
