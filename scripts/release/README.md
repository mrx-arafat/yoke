# Release packages

The release workflow builds all four supported targets on native GitHub runners for every tag.
Each target produces `yoke-<target>`, `yoke-<target>.sha256`, a redistribution archive `yoke-<target>.tar.gz`, and `THIRD_PARTY_LICENSES-<target>`.
The archive and notice asset each also have a `.sha256` file.
The checksum format is one lowercase SHA-256 digest, two spaces, and the exact asset basename.

The archive contains `yoke`, `LICENSE-MIT`, `LICENSE-APACHE`, and `THIRD_PARTY_LICENSES`.
Redistribute the archive so license notices travel with the executable.
The raw executable is for the updater and embeds the generated notices at build time, so notices also travel with self-updates.
The archive and companion notice asset provide the human-readable copy.

Notices are generated from Cargo's resolved graph for the target, including build dependencies and any extra packages resolved by Cargo.
The generator reproduces license files and nested NOTICE files from the downloaded crate sources, and refuses a dependency with no license text.
It never edits generated artifacts by hand.
`cargo deny check licenses sources` must pass before any package is built.

The workflow creates a draft release only after all four targets succeed.
Tag/version agreement is checked before release compilation.
Human review of the compliance checklist is required before publication.
Only stable tags of the form `vMAJOR.MINOR.PATCH` or `MAJOR.MINOR.PATCH` are accepted by the updater.
GitHub's latest-release endpoint does not expose drafts.
Manual workflow runs create Actions artifacts without creating a release.

For a local package, run the following with one supported target and an existing binary:

```sh
uv run --no-project python scripts/release/package.py --target <target> --binary <binary> --out <new-directory>
uv run --no-project python -m unittest discover -s scripts/release -p 'test_*.py' -q
```

The packaging command refuses an existing output directory.
No script tags, pushes, updates an installed executable, or publishes a release.
The four-platform workflow and release-upload permissions require a future authorized GitHub run.
