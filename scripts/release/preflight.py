#!/usr/bin/env python3
"""Read-only release preflight: prints PASS, FAIL or SKIP per check before a release tag is pushed."""

import argparse
import json
import os
import re
import subprocess
from collections.abc import Callable, Sequence
from pathlib import Path
from typing import Any, NamedTuple

ROOT = Path(__file__).resolve().parents[2]
PASS, FAIL, SKIP = "PASS", "FAIL", "SKIP"
NOT_FOUND, TIMED_OUT, GH_AUTH_REQUIRED = 127, 124, 4
TIMEOUT_SECONDS = 120
# The exact rule scripts/release/package.py applies to --tag.
TAG_PATTERN = re.compile(r"v?(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
IGNORED_UNTRACKED = {"?? .codegraph/", "?? .lavish/"}
REQUIRED_WORKFLOWS = ("CI", "Installer")
DISPATCH_COMMAND = "gh workflow run release.yml --ref main"
HUMAN_GATES = (
    "COMPLIANCE.md is fully ticked",
    "Trademark and crates.io name checks are done",
    "Workflow assets for all four targets are downloaded and verified (checksums, archive contents)",
    "Install acceptance passes on each platform; update acceptance passes after publication",
    "Trials are complete and every incident is resolved or accepted (docs/trials.md)",
    "Console acceptance is complete (docs/console-acceptance.md)",
)


class Cmd(NamedTuple):
    code: int
    out: str
    err: str


class Check(NamedTuple):
    name: str
    status: str
    detail: str


Runner = Callable[[Sequence[str]], Cmd]


def subprocess_runner(root: Path) -> Runner:
    """Run argument lists only, from the repository root, never prompting for credentials."""
    env = {**os.environ, "GIT_TERMINAL_PROMPT": "0"}

    def run(args: Sequence[str]) -> Cmd:
        try:
            done = subprocess.run(
                list(args),
                cwd=root,
                env=env,
                capture_output=True,
                text=True,
                errors="replace",
                timeout=TIMEOUT_SECONDS,
            )
        except FileNotFoundError:
            return Cmd(NOT_FOUND, "", f"{args[0]}: command not found")
        except subprocess.TimeoutExpired:
            return Cmd(TIMED_OUT, "", f"{args[0]} timed out after {TIMEOUT_SECONDS}s")
        return Cmd(done.returncode, done.stdout, done.stderr)

    return run


def first_line(text: str) -> str:
    lines = text.strip().splitlines()
    return lines[0] if lines else "no output"


def toml_section_value(text: str, section: str, key: str) -> str | None:
    """Read one `key = "value"` from a TOML section (tomllib needs Python 3.11)."""
    in_section = False
    for line in text.splitlines():
        header = re.fullmatch(r"\s*\[([^\]]+)\]\s*", line)
        if header:
            in_section = header.group(1) == section
        elif in_section:
            match = re.fullmatch(rf'\s*{re.escape(key)}\s*=\s*"([^"]+)"\s*(#.*)?', line)
            if match:
                return match.group(1)
    return None


def cli_version(root: Path) -> str:
    """The yoke-cli package version, which must come from [workspace.package]."""
    package = (root / "crates/yoke-cli/Cargo.toml").read_text(encoding="utf-8")
    if not re.search(r"^version\.workspace\s*=\s*true\s*$", package, re.MULTILINE):
        raise ValueError(
            "crates/yoke-cli/Cargo.toml no longer inherits version.workspace"
        )
    version = toml_section_value(
        (root / "Cargo.toml").read_text(encoding="utf-8"),
        "workspace.package",
        "version",
    )
    if version is None:
        raise ValueError("Cargo.toml has no [workspace.package] version")
    return version


def check_tag(tag: str, version: str) -> Check:
    name = "tag format"
    if not TAG_PATTERN.fullmatch(tag):
        return Check(
            name,
            FAIL,
            f"{tag!r} is not a stable vMAJOR.MINOR.PATCH or MAJOR.MINOR.PATCH tag, which release.yml rejects",
        )
    if tag.removeprefix("v") != version:
        return Check(name, FAIL, f"{tag} does not match the yoke-cli version {version}")
    return Check(name, PASS, f"{tag} matches the yoke-cli version {version}")


def check_clean_tree(run: Runner) -> Check:
    name = "clean tree"
    done = run(["git", "status", "--porcelain"])
    if done.code != 0:
        return Check(name, FAIL, f"git status failed: {first_line(done.err)}")
    dirty = [
        line.strip()
        for line in done.out.splitlines()
        if line.strip() and line not in IGNORED_UNTRACKED
    ]
    if dirty:
        shown = "; ".join(dirty[:5]) + ("; ..." if len(dirty) > 5 else "")
        return Check(name, FAIL, f"{len(dirty)} uncommitted path(s): {shown}")
    return Check(
        name, PASS, "no uncommitted changes (.codegraph/ and .lavish/ ignored)"
    )


def check_pushed(run: Runner, head: str) -> Check:
    name = "HEAD is pushed"
    upstream = run(["git", "rev-parse", "@{u}"])
    if upstream.code != 0:
        return Check(name, FAIL, f"no upstream to compare: {first_line(upstream.err)}")
    pushed = upstream.out.strip()
    if pushed != head:
        return Check(
            name,
            FAIL,
            f"HEAD {head[:7]} differs from upstream {pushed[:7]}; push or pull first",
        )
    return Check(name, PASS, f"HEAD {head[:7]} equals its upstream")


def check_changelog(run: Runner, root: Path, version: str | None) -> Check:
    name = "changelog"
    done = run(
        ["uv", "run", "--no-project", "python", "scripts/changelog.py", "--check"]
    )
    if done.code != 0:
        return Check(
            name,
            FAIL,
            f"scripts/changelog.py --check exit {done.code}: {first_line(done.err or done.out)}",
        )
    if version is None:
        return Check(
            name,
            FAIL,
            "yoke-cli version is unreadable, so the change record is unknown",
        )
    record = f"docs/changes/{version}.json"
    try:
        status = json.loads((root / record).read_text(encoding="utf-8")).get("status")
    except (OSError, ValueError, AttributeError) as error:
        return Check(name, FAIL, f"cannot read {record}: {error}")
    if not isinstance(status, str) or status == "unreleased":
        return Check(
            name,
            FAIL,
            f"{record} status is {status!r}; set it to the release date, then regenerate CHANGELOG.md",
        )
    return Check(name, PASS, f"CHANGELOG.md is current and {record} is dated {status}")


def gh_runs(
    run: Runner, args: list[str], fields: str
) -> tuple[list[dict[str, Any]] | None, str]:
    """Return gh's runs, or None plus the reason they could not be queried."""
    done = run(["gh", "run", "list", *args, "--json", fields])
    if done.code == NOT_FOUND:
        return None, "gh is not installed"
    if done.code == GH_AUTH_REQUIRED or "gh auth login" in done.err:
        return None, "gh is not authenticated (run gh auth login)"
    if done.code != 0:
        return None, f"gh run list failed (exit {done.code}): {first_line(done.err)}"
    try:
        runs = json.loads(done.out)
    except ValueError:
        return None, "gh run list returned invalid JSON"
    return runs, ""


def check_ci(run: Runner, head: str) -> Check:
    name = "CI green for HEAD"
    runs, reason = gh_runs(run, ["--commit", head], "name,conclusion,status")
    if runs is None:
        return Check(name, SKIP, reason)
    latest: dict[str, dict[str, Any]] = {}
    for item in runs:
        latest.setdefault(str(item.get("name")), item)  # gh lists the newest run first
    problems = []
    for workflow in REQUIRED_WORKFLOWS:
        item = latest.get(workflow)
        if item is None:
            problems.append(f"{workflow}: no run for {head[:7]}")
        elif item.get("conclusion") != "success":
            problems.append(
                f"{workflow}: {item.get('conclusion') or item.get('status')}"
            )
    if problems:
        return Check(name, FAIL, "; ".join(problems))
    return Check(
        name, PASS, f"{' and '.join(REQUIRED_WORKFLOWS)} succeeded for {head[:7]}"
    )


def check_release_run(run: Runner, head: str) -> Check:
    name = "release workflow evidence"
    runs, reason = gh_runs(
        run,
        ["--workflow", "release.yml", "--commit", head],
        "conclusion,event,status,url",
    )
    if runs is None:
        return Check(name, SKIP, reason)
    for item in runs:
        if item.get("conclusion") == "success":
            return Check(
                name, PASS, f"{item.get('event')} run succeeded: {item.get('url')}"
            )
    seen = (
        f" (latest run: {runs[0].get('conclusion') or runs[0].get('status')})"
        if runs
        else ""
    )
    return Check(
        name,
        FAIL,
        f"no successful release.yml run for {head[:7]}{seen}; run: {DISPATCH_COMMAND}",
    )


def check_tag_absent(run: Runner, tag: str) -> Check:
    name = "tag not yet used"
    ref = f"refs/tags/{tag}"
    local = run(["git", "rev-parse", "-q", "--verify", ref])
    if local.code == 0:
        return Check(name, FAIL, f"{tag} already exists locally")
    if local.code != 1:
        return Check(name, FAIL, f"git rev-parse failed: {first_line(local.err)}")
    remote = run(["git", "ls-remote", "--tags", "origin", ref])
    if remote.code != 0:
        return Check(name, SKIP, f"could not query origin: {first_line(remote.err)}")
    if remote.out.strip():
        return Check(name, FAIL, f"{tag} already exists on origin")
    return Check(name, PASS, f"{tag} exists neither locally nor on origin")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--tag", required=True, help="the release tag to check, e.g. v0.1.0"
    )
    args = parser.parse_args(argv)
    run = subprocess_runner(ROOT)

    head_done = run(["git", "rev-parse", "HEAD"])
    if head_done.code != 0:
        print(f"[FAIL] cannot resolve HEAD: {first_line(head_done.err)}")
        return 1
    head = head_done.out.strip()

    try:
        version: str | None = cli_version(ROOT)
        tag_check = check_tag(args.tag, version)
    except (OSError, ValueError) as error:
        version = None
        tag_check = Check(
            "tag format", FAIL, f"cannot read the yoke-cli version: {error}"
        )
    checks = [
        tag_check,
        check_clean_tree(run),
        check_pushed(run, head),
        check_changelog(run, ROOT, version),
        check_ci(run, head),
        check_release_run(run, head),
        check_tag_absent(run, args.tag),
    ]
    for number, check in enumerate(checks, start=1):
        print(f"[{check.status}] {number}. {check.name}: {check.detail}")

    counts = {
        status: sum(c.status == status for c in checks) for status in (PASS, FAIL, SKIP)
    }
    tally = f"{counts[PASS]} passed, {counts[FAIL]} failed, {counts[SKIP]} skipped"
    if counts[FAIL]:
        print(f"\nSUMMARY: FAIL - {tally}. Do not push the tag.")
    else:
        skipped = (
            " Skipped checks are unverified: resolve them first."
            if counts[SKIP]
            else ""
        )
        print(
            f"\nSUMMARY: no failures - {tally}.{skipped} The human gates below are still open."
        )
    print("\nHuman gates this script cannot check (docs/release-acceptance.md):")
    for gate in HUMAN_GATES:
        print(f"  - {gate}")
    return 1 if counts[FAIL] else 0


if __name__ == "__main__":
    raise SystemExit(main())
