"""Focused safety and regression-budget checks for the disposable benchmark."""

import argparse
import contextlib
import io
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import benchmark_hooks as bench


class BenchmarkSafetyTests(unittest.TestCase):
    def test_real_home_is_refused_before_program_execution(self):
        with (
            tempfile.TemporaryDirectory() as scratch,
            mock.patch.object(bench.subprocess, "run") as program,
        ):
            home = Path(scratch).resolve()
            with self.assertRaisesRegex(ValueError, "disposable"):
                bench.run(Path("unused"), home, home, home, ["usage"])
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

    def test_ci_fails_on_a_large_hook_regression(self):
        measured = {"hooks": [{"p99_ms": 251}], "status": "pass"}
        with (
            mock.patch.object(bench, "benchmark", return_value=measured),
            mock.patch("sys.argv", ["benchmark_hooks.py", "--ci", "--json"]),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            self.assertEqual(bench.main(), 1)


if __name__ == "__main__":
    unittest.main()
