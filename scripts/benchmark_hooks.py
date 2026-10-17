#!/usr/bin/env -S uv run --no-project python
"""Measure debug hooks in disposable state smaller than 50 MiB, then remove it."""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent
CAP = 48 * 1024 * 1024
EVENTS = (
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Stop",
    "PreCompact",
    "SessionEnd",
)


class Parser(argparse.ArgumentParser):
    """Keep usage errors structured and actionable for unattended callers."""

    def error(self, message: str) -> None:
        print(f"error: {json.dumps(message)}")
        print("help: uv run --no-project python scripts/benchmark_hooks.py --help")
        self.exit(2)


def percentile(values: list[float], fraction: float) -> float:
    """Use the nearest-rank percentile, including the worst sample for p99."""
    return sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)]


def run(
    binary: Path,
    home: Path,
    project: Path,
    real_home: Path,
    args: list[str],
    payload: dict | None = None,
) -> subprocess.CompletedProcess:
    """Assert isolated HOME in the invoking shell before every program execution."""
    if (
        not home.is_dir()
        or home.resolve() == real_home
        or home.resolve().is_relative_to(ROOT)
    ):
        raise ValueError(
            "benchmark HOME must be an existing disposable directory outside the repository"
        )
    env = os.environ.copy()
    env["HOME"] = str(home)
    for key in ("CLAUDE_CONFIG_DIR", "CODEX_HOME", "YOKE_OFF"):
        env.pop(key, None)
    guard = 'real_home=$1; shift; test -n "$HOME" && test "$HOME" != "$real_home" && test -d "$HOME" || exit 1; exec "$@"'
    result = subprocess.run(
        ["/bin/sh", "-c", guard, "yoke-benchmark", str(real_home), str(binary), *args],
        cwd=project,
        env=env,
        input=json.dumps(payload) if payload is not None else "",
        text=True,
        capture_output=True,
        timeout=30,
        check=False,
    )
    if result.returncode:
        raise RuntimeError(
            f"{' '.join(args)} exited {result.returncode}: {result.stderr[:500]}"
        )
    return result


def seed(
    db: sqlite3.Connection,
    home: Path,
    project: Path,
    decisions: int,
    usage_samples: int,
    now: int,
) -> None:
    """Seed exact source records and trusted offline configuration in disposable state."""
    page_size = db.execute("PRAGMA page_size").fetchone()[0]
    db.execute(f"PRAGMA max_page_count={CAP // page_size}")
    db.execute("PRAGMA journal_mode=WAL")
    for path in sorted((ROOT / "crates/yoke-core/src/store").glob("[0-9]*.sql")):
        if ".down." not in path.name:
            db.executescript(path.read_text())
            db.execute(f"PRAGMA user_version={int(path.name[:3])}")
    config = "mode='observe'\n[update]\ncheck=false\n"
    file = home / ".yoke/config.toml"
    file.write_text(config)
    db.execute(
        "INSERT INTO trust VALUES('global',?,?,?,?,?)",
        (
            str(home),
            str(file),
            hashlib.sha256(config.encode()).hexdigest(),
            config,
            now,
        ),
    )
    db.execute(
        "INSERT INTO sessions(id,agent,project,started_at) VALUES('seed','claude',?,?)",
        (str(project), now),
    )
    db.executemany(
        "INSERT INTO decisions(ts,session_id,agent,project,event,mode,outcome,gates,payload) VALUES(?,'seed','claude',?,'pre_tool_use','observe','allow','[]',?)",
        ((now, str(project), "x" * 256) for _ in range(decisions)),
    )
    db.executemany(
        "INSERT INTO usage_samples VALUES('claude',?,?,?,'unknown',3,2,1,0,NULL,?)",
        (
            (f"usage-{index % 1000}", str(project), str(index), now - 400 * 86400)
            for index in range(usage_samples)
        ),
    )
    for key, value in (
        ("notifications.decision_id", str(decisions)),
        ("retention.last_prune", str(now)),
    ):
        db.execute("INSERT INTO kv VALUES(?,?)", (key, value))
    db.commit()
    db.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchall()


def real_test_rows(real_home: Path, project: Path) -> int:
    """Read real state only to detect accidental test-project writes."""
    path = real_home / ".yoke/state.db"
    if not path.exists():
        return 0
    with sqlite3.connect(path.as_uri() + "?mode=ro", uri=True) as db:
        return db.execute(
            "SELECT count(*) FROM decisions WHERE project=?", (str(project),)
        ).fetchone()[0]


def benchmark(args: argparse.Namespace) -> dict:
    """Measure complete process latency, validate output, and remove every fixture."""
    binary = args.binary.resolve()
    if (
        not binary.is_file()
        or not binary.is_relative_to(ROOT / "target/debug")
        or "release" in binary.parts
    ):
        raise ValueError(
            "use a debug binary under target/debug; build it with cargo build -p yoke-cli"
        )
    if (
        not 1 <= args.samples <= 200
        or not 0 <= args.decisions <= 50000
        or not 0 <= args.usage_samples <= 100000
    ):
        raise ValueError(
            "samples must be 1..200, decisions 0..50000 and usage-samples 0..100000; fixtures are capped below 50 MiB"
        )
    if shutil.disk_usage(tempfile.gettempdir()).free < 1024**3:
        raise ValueError(
            "less than 1 GiB is free; remove only your own finished fixtures before retrying"
        )
    real_home = Path.home().resolve()
    with tempfile.TemporaryDirectory(prefix="yoke-benchmark-") as scratch:
        home = Path(scratch).resolve()
        project = home / "project"
        project.mkdir()
        (home / ".yoke").mkdir(mode=0o700)
        before = real_test_rows(real_home, project)
        db = sqlite3.connect(home / ".yoke/state.db")
        try:
            now = int(time.time())
            seed(db, home, project, args.decisions, args.usage_samples, now)
            session = "benchmark-" + uuid.uuid4().hex
            rows = []
            for event in EVENTS:
                payload = {
                    "session_id": session,
                    "cwd": str(project),
                    "hook_event_name": event,
                    "tool_name": "Bash",
                    "tool_input": {"command": "true"},
                }
                times = []
                for index in range(args.samples + 1):
                    started = time.perf_counter()
                    run(
                        binary,
                        home,
                        project,
                        real_home,
                        ["run-hook", "--agent", "claude", event],
                        payload,
                    )
                    if index:
                        times.append((time.perf_counter() - started) * 1000)
                rows.append(
                    {
                        "event": event,
                        "p50_ms": percentile(times, 0.50),
                        "p95_ms": percentile(times, 0.95),
                        "p99_ms": percentile(times, 0.99),
                    }
                )
            count = db.execute(
                "SELECT count(*) FROM decisions WHERE session_id=?", (session,)
            ).fetchone()[0]
            if count != len(EVENTS) * (args.samples + 1):
                raise RuntimeError(
                    "hook decisions are missing; timings cannot be accepted"
                )
            started = time.perf_counter()
            usage = json.loads(
                run(binary, home, project, real_home, ["usage", "--json"]).stdout
            )
            usage_ms = (time.perf_counter() - started) * 1000
            if args.usage_samples and not usage["sessions"]:
                raise RuntimeError("seeded usage sessions are missing from yoke usage")
            for session_usage in usage["sessions"]:
                index = int(session_usage["session_id"].removeprefix("usage-"))
                expected = (args.usage_samples - 1 - index) // 1000 + 1
                if (
                    session_usage["input_tokens"],
                    session_usage["output_tokens"],
                    session_usage["cache_read_tokens"],
                    session_usage["unknown_pricing_samples"],
                ) != (3 * expected, 2 * expected, expected, expected):
                    raise RuntimeError(
                        "yoke usage totals differ from the independently counted seed"
                    )
            # Exercise the real worker synchronously, without starting another process pool.
            db.execute(
                "UPDATE decisions SET outcome='would_deny' WHERE session_id='seed' AND id%10=0"
            )
            db.execute("UPDATE kv SET value='0' WHERE key='notifications.decision_id'")
            db.execute(
                "INSERT INTO jobs(kind,dedupe_key,project,payload,state,max_attempts,run_after,created_at,updated_at) VALUES('notifications-drain','notifications-drain','','{}','queued',3,?,?,?)",
                (now, now, now),
            )
            db.commit()
            started = time.perf_counter()
            run(binary, home, project, real_home, ["jobs", "work"])
            drain_ms = (time.perf_counter() - started) * 1000
            job_state = db.execute(
                "SELECT state FROM jobs WHERE kind='notifications-drain'"
            ).fetchone()[0]
            if job_state != "succeeded":
                raise RuntimeError(
                    f"notification drain job is {job_state}; inspect its stored error"
                )
            observed = db.execute(
                "SELECT coalesce(sum(count),0) FROM notifications WHERE kind='observe_would_deny'"
            ).fetchone()[0]
            if observed != args.decisions // 10:
                raise RuntimeError(
                    "background notification counts differ from the seeded decisions"
                )
            size = (
                db.execute("PRAGMA page_count").fetchone()[0]
                * db.execute("PRAGMA page_size").fetchone()[0]
            )
            if size > CAP:
                raise RuntimeError("fixture exceeded its 48 MiB page limit")
            if real_test_rows(real_home, project) != before:
                raise RuntimeError(
                    "test-project decisions reached real state; stop and inspect the real database read-only"
                )
            return {
                "database_bytes": size,
                "decisions": args.decisions,
                "usage_samples": args.usage_samples,
                "samples_per_event": args.samples,
                "usage_ms": usage_ms,
                "background_drain_ms": drain_ms,
                "background_job_state": job_state,
                "hooks": rows,
                "leaked_test_rows": 0,
                "limit": "below 50 MiB; 700 MB unverified",
            }
        finally:
            db.close()


def main() -> int:
    """Print compact measurements; --ci fails only on a large process-latency regression."""
    parser = Parser(
        description=__doc__,
        epilog="Examples: benchmark_hooks.py --ci; benchmark_hooks.py --json --samples 30",
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=ROOT / "target/debug/yoke",
        help="debug binary (default: target/debug/yoke)",
    )
    parser.add_argument(
        "--samples",
        type=int,
        default=30,
        help="timed runs per event, 1..200 (default: 30)",
    )
    parser.add_argument(
        "--decisions",
        type=int,
        default=20000,
        help="seed decisions, at most 50000 (default: 20000)",
    )
    parser.add_argument(
        "--usage-samples",
        type=int,
        default=20000,
        help="seed usage samples, at most 100000 (default: 20000)",
    )
    parser.add_argument(
        "--ci",
        action="store_true",
        help="use 1000 decisions, 2000 usage samples and 8 timings; fail above 250 ms p99",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="print the complete machine-readable measurement object",
    )
    args = parser.parse_args()
    if args.ci:
        args.decisions, args.usage_samples, args.samples = 1000, 2000, 8
    try:
        result = benchmark(args)
    except (
        ValueError,
        RuntimeError,
        OSError,
        sqlite3.Error,
        subprocess.TimeoutExpired,
    ) as error:
        print(f"error: {json.dumps(str(error))}")
        if (
            isinstance(error, sqlite3.Error)
            and getattr(error, "sqlite_errorcode", None) == sqlite3.SQLITE_FULL
        ):
            print(
                "help: fixture reached its page cap or free space; retry --decisions 10000 --usage-samples 10000"
            )
        else:
            print("help: uv run --no-project python scripts/benchmark_hooks.py --help")
        return 2 if isinstance(error, ValueError) else 1
    failed = args.ci and any(row["p99_ms"] > 250 for row in result["hooks"])
    result["status"] = "fail" if failed else "pass"
    if args.json:
        print(json.dumps(result, indent=2))
    else:
        print(
            f"status: {result['status']}\ndatabase_bytes: {result['database_bytes']}\nusage_ms: {result['usage_ms']:.3f}"
        )
        print(f"hooks[{len(result['hooks'])}]{{event,p50_ms,p95_ms,p99_ms}}:")
        for row in result["hooks"]:
            print(
                f"  {row['event']},{row['p50_ms']:.3f},{row['p95_ms']:.3f},{row['p99_ms']:.3f}"
            )
        print("limit: below 50 MiB; 700 MB unverified")
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
