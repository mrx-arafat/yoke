# Release packages

The release workflow builds all four supported targets on native GitHub runners for every tag.
Each target produces `yoke-<target>`, `yoke-<target>.sha256`, a redistribution archive `yoke-<target>.tar.gz`, and `THIRD_PARTY_LICENSES-<target>`.
The archive and notice asset each also have a `.sha256` file.
The archive is reproducible: entries carry no owner, group or file time, and the gzip header carries no time or name, so equal inputs give equal bytes.
The draft release also holds `SHA256SUMS`, one list of every asset's checksum, and every asset has signed build provenance made by the `draft` job; see [docs/verifying-releases.md](../../docs/verifying-releases.md).
The checksum format is one lowercase SHA-256 digest, two spaces, and the exact asset basename.
Packaging rejects empty binaries and binaries larger than 64 MiB before creating the output directory, matching the updater's accepted executable size.

The archive contains `yoke`, `LICENSE-MIT`, `LICENSE-APACHE`, and `THIRD_PARTY_LICENSES`.
Redistribute the archive so license notices travel with the executable.
The raw executable is for the updater and embeds the generated notices at build time, so notices also travel with self-updates.
The archive and companion notice asset provide the human-readable copy.

Notices are generated from Cargo's resolved graph for the target, including build dependencies and any extra packages resolved by Cargo.
The generator reproduces license files and nested NOTICE files from the downloaded crate sources, and refuses a dependency with no license text.
It never edits generated artifacts by hand.
The full `cargo deny check` (advisories, bans, licenses, sources) must pass in the `lint` job before any package is built, and a tag builds only a commit that is in `main` and passed `ci.yml` there.
The toolchain is pinned in `release.yml` (`RUST_TOOLCHAIN`); raise it deliberately.
Before a release exists, `install.sh` builds the commit `main` points to when it runs, and `installer.yml` passes `YOKE_COMMIT` so that CI builds the commit under test; no commit is hard-coded in the script.

The workflow creates a draft release only after all four targets succeed.
Tag/version agreement is checked before release compilation.
Human review of the compliance checklist is required before publication.
The workflow starts only on `vMAJOR.MINOR.PATCH` tags; the updater itself accepts `vMAJOR.MINOR.PATCH` or `MAJOR.MINOR.PATCH`.
GitHub's latest-release endpoint does not expose drafts.
Manual workflow runs create Actions artifacts without creating a release.

For a local package, run the following with one supported target and an existing binary:

```sh
uv run --no-project python scripts/release/package.py --target <target> --binary <binary> --out <new-directory>
uv run --no-project python -m unittest discover -s scripts/release -p 'test_*.py' -q
```

The packaging command refuses an existing output directory.
No script tags, pushes, updates an installed executable, or publishes a release.
Before pushing a release tag, run the read-only preflight: `uv run --no-project python scripts/release/preflight.py --tag vMAJOR.MINOR.PATCH`.
It prints PASS, FAIL or SKIP per machine-checkable item and lists the human gates it cannot check.
The four-platform workflow and release-upload permissions require a future authorized GitHub run.
