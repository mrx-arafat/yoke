"""Focused safety and regression-budget checks for the disposable benchmark."""

import argparse
import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import benchmark_hooks as bench


class BenchmarkSafetyTests(unittest.TestCase):
    def test_real_home_is_refused_before_program_execution(self):
        with (
            tempfile.TemporaryDirectory() as scratch,
            mock.patch.object(bench.subprocess, "Popen") as program,
        ):
            home = Path(scratch).resolve()
            with self.assertRaisesRegex(ValueError, "disposable"):
                bench.Launcher(Path("unused"), home, home, home)
            program.assert_not_called()

    def test_oversized_seed_is_refused_before_fixture_creation(self):
        args = argparse.Namespace(
            binary=bench.ROOT / "target/debug/yoke",
            samples=8,
            decisions=50001,
            usage_samples=0,
        )
        with (
            mock.patch.object(Path, "is_file", return_value=True),
            mock.patch.object(bench.tempfile, "TemporaryDirectory") as fixture,
        ):
            with self.assertRaisesRegex(ValueError, "capped below 50 MiB"):
                bench.benchmark(args)
            fixture.assert_not_called()

    def test_percentiles_preserve_the_tail_latency(self):
        self.assertEqual(bench.percentile([100.0, 1.0, 2.0], 0.50), 2.0)
        self.assertEqual(bench.percentile([100.0, 1.0, 2.0], 0.99), 100.0)

    def test_release_and_external_binaries_are_refused_before_fixture_creation(self):
        for binary in (bench.ROOT / "target/release/yoke", Path("/outside/yoke")):
            with (
                self.subTest(binary=binary),
                mock.patch.object(Path, "is_file", return_value=True),
                mock.patch.object(bench.tempfile, "TemporaryDirectory") as fixture,
            ):
                args = argparse.Namespace(
                    binary=binary, samples=8, decisions=1000, usage_samples=2000
                )
                with self.assertRaisesRegex(ValueError, "debug binary"):
                    bench.benchmark(args)
                fixture.assert_not_called()

    def test_ci_fails_on_a_large_hook_regression(self):
        measured = {"hooks": [{"p99_ms": 251}], "status": "pass"}
        with (
            mock.patch.object(bench, "benchmark", return_value=measured),
            mock.patch("sys.argv", ["benchmark_hooks.py", "--ci", "--json"]),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            self.assertEqual(bench.main(), 1)


class BenchmarkLauncherTests(unittest.TestCase):
    """Functional checks use only the system shell inside disposable HOME."""

    def test_calls_reuse_one_parent_and_preserve_arguments_payload_and_streams(self):
        with tempfile.TemporaryDirectory(prefix="benchmark-launcher-") as scratch:
            home = Path(scratch).resolve()
            project = home / "project"
            project.mkdir()
            payload = {"text": "quotes ' \" and $(printf injected)\nsecond line"}
            literal = "$(touch injected); 'quote'"
            with bench.Launcher(Path("/bin/sh"), home, project, home / "real") as run:
                first = run.run(
                    ["-c", 'printf "%s\\n" "$PPID"; cat; printf "warning" >&2'],
                    payload,
                )
                second = run.run(
                    ["-c", 'printf "%s\\n%s" "$PPID" "$1"', "helper", literal]
                )
                self.assertEqual(
                    first.stdout.splitlines()[0], second.stdout.splitlines()[0]
                )
                self.assertEqual(json.loads(first.stdout.split("\n", 1)[1]), payload)
                self.assertEqual(first.stderr, "warning")
                self.assertEqual(second.stdout.split("\n", 1)[1], literal)
                self.assertFalse((project / "injected").exists())
                with self.assertRaisesRegex(RuntimeError, "exited 7: failure"):
                    run.run(["-c", "printf failure >&2; exit 7"])
                self.assertEqual(
                    run.run(["-c", "printf recovered"]).stdout, "recovered"
                )
                shell = run.process
            self.assertIsNotNone(shell.poll())

    def test_unresponsive_launcher_raises_timeout_and_is_reaped(self):
        with tempfile.TemporaryDirectory(prefix="benchmark-launcher-") as scratch:
            home = Path(scratch).resolve()
            with self.assertRaises(bench.subprocess.TimeoutExpired):
                with bench.Launcher(Path("/bin/sh"), home, home, home / "real") as run:
                    shell = run.process
                    with mock.patch.object(
                        bench.select, "select", return_value=([], [], [])
                    ):
                        run.run(["-c", ":"])
            self.assertIsNotNone(shell.poll())

    def test_home_resolving_to_real_state_is_refused_before_a_later_call(self):
        with tempfile.TemporaryDirectory(prefix="benchmark-launcher-") as scratch:
            home = Path(scratch).resolve()
            real_home = home / "real"
            with bench.Launcher(Path("/bin/sh"), home, home, real_home) as run:
                with mock.patch.object(Path, "resolve", return_value=real_home):
                    with self.assertRaisesRegex(ValueError, "disposable"):
                        run.run(["-c", ":"])


if __name__ == "__main__":
    unittest.main()
