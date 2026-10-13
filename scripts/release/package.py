"""Generate license notices and package one of Yoke's four release targets."""

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import tarfile
from pathlib import Path

TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
)
LICENSE_PREFIXES = ("LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE")


def generate_notices(metadata: dict) -> str:
    """Bundle actual license and NOTICE text from the resolved Cargo graph."""
    resolved = {node["id"] for node in metadata["resolve"]["nodes"]}
    workspace = set(metadata["workspace_members"])
    parts = ["Yoke third-party licenses\nGenerated from cargo metadata; do not edit.\n"]
    for package in sorted(
        metadata["packages"], key=lambda item: (item["name"], item["version"])
    ):
        if package["id"] not in resolved or package["id"] in workspace:
            continue
        root = Path(package["manifest_path"]).parent.resolve()
        files = {
            path.resolve()
            for path in root.rglob("*")
            if path.is_file() and path.name.upper().startswith(LICENSE_PREFIXES)
        }
        if package.get("license_file"):
            files.add((root / package["license_file"]).resolve())
        if not any(not path.name.upper().startswith("NOTICE") for path in files):
            raise ValueError(
                f"No license text shipped by {package['name']} {package['version']}"
            )
        parts.append(f"\n=== {package['name']} {package['version']} ===\n")
        parts.append(f"Declared license: {package.get('license') or 'license_file'}\n")
        for path in sorted(files):
            if not path.is_relative_to(root):
                raise ValueError(f"License path escapes package: {path}")
            parts.append(f"\n--- {path.relative_to(root)} ---\n")
            parts.append(path.read_text(encoding="utf-8"))
            parts.append("\n")
    return "".join(parts)


def checksum(path: Path) -> None:
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    path.with_name(f"{path.name}.sha256").write_text(
        f"{digest}  {path.name}\n", encoding="utf-8"
    )


def package_release(
    root: Path, binary: Path, output: Path, target: str, notices: str
) -> None:
    """Write raw updater assets and the redistribution bundle without overwriting output."""
    if target not in TARGETS:
        raise ValueError("Unsupported target")
    if not binary.is_file() or binary.is_symlink():
        raise ValueError("Binary must be a regular file")
    output.mkdir(parents=True, exist_ok=False)
    name = f"yoke-{target}"
    executable = output / name
    shutil.copyfile(binary, executable)
    executable.chmod(0o755)
    licenses = output / f"THIRD_PARTY_LICENSES-{target}"
    licenses.write_text(notices, encoding="utf-8")
    archive = output / f"{name}.tar.gz"
    with tarfile.open(archive, "w:gz") as bundle:
        bundle.add(executable, arcname="yoke")
        bundle.add(licenses, arcname="THIRD_PARTY_LICENSES")
        for filename in ("LICENSE-MIT", "LICENSE-APACHE"):
            bundle.add(root / filename, arcname=filename)
    for path in (executable, licenses, archive):
        checksum(path)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--binary", type=Path)
    parser.add_argument(
        "--notices-only",
        action="store_true",
        help="Write generated notices before the release build",
    )
    parser.add_argument(
        "--tag", help="Require a stable tag matching the CLI package version"
    )
    parser.add_argument(
        "--out",
        type=Path,
        required=True,
        help="New output directory; existing paths are refused",
    )
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    metadata = subprocess.run(
        [
            "cargo",
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--filter-platform",
            args.target,
        ],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
        timeout=120,
    )
    graph = json.loads(metadata.stdout)
    if args.tag:
        cli = next(
            package for package in graph["packages"] if package["name"] == "yoke-cli"
        )
        if (
            not re.fullmatch(
                r"v?(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", args.tag
            )
            or args.tag.removeprefix("v") != cli["version"]
        ):
            parser.error("Release tag must be a stable version matching yoke-cli")
    notices = generate_notices(graph)
    if args.notices_only:
        with args.out.open("x", encoding="utf-8") as output:
            for filename in ("LICENSE-MIT", "LICENSE-APACHE"):
                output.write(f"Yoke: {filename}\n")
                output.write((root / filename).read_text(encoding="utf-8"))
                output.write("\n")
            output.write(notices)
    else:
        if args.binary is None:
            parser.error("--binary is required unless --notices-only is set")
        package_release(root, args.binary, args.out, args.target, notices)


if __name__ == "__main__":
    main()
