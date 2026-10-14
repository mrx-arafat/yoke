# Release acceptance

This page is the checklist a human completes before publishing a Yoke release.
Local focused checks are not release acceptance, and nothing on this page is complete until its evidence is recorded here.

A Git push never publishes a binary.
Pushing a tag runs the release workflow, which builds every target on its own native runner and creates a draft release.
A draft is invisible to `yoke update` until a human reviews it against [COMPLIANCE.md](../COMPLIANCE.md) and publishes it.
A manual `workflow_dispatch` run produces Actions artifacts only and creates no release.

## Supported targets

| Target | Release workflow runner | Local status on 2026-10-14 |
| --- | --- | --- |
| `aarch64-apple-darwin` | `macos-15` | Packaging dry run passed on the host (see below) |
| `x86_64-apple-darwin` | `macos-15-intel` | Not built; needs the workflow run |
| `x86_64-unknown-linux-gnu` | `ubuntu-24.04` | Not built; needs the workflow run |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` | Not built; needs the workflow run |

The three non-native targets are built only by the release workflow on native runners, as [scripts/release/README.md](../scripts/release/README.md) documents.
No local cross-compilation path is documented, so none was attempted: the development host has only the `aarch64-apple-darwin` standard library installed and no Linux linker.

## Local packaging dry run

The host target was packaged with exactly the workflow's steps, into a temporary directory rather than `target/release`:

```sh
W=$(mktemp -d)
uv run --no-project python scripts/release/package.py --target aarch64-apple-darwin --notices-only --out "$W/THIRD_PARTY_LICENSES"
CARGO_TARGET_DIR="$W/target" RUSTFLAGS='--cfg yoke_release_notices --check-cfg=cfg(yoke_release_notices)' \
  YOKE_RELEASE_NOTICES="$W/THIRD_PARTY_LICENSES" \
  cargo build --locked --release -p yoke-cli --target aarch64-apple-darwin
uv run --no-project python scripts/release/package.py --target aarch64-apple-darwin \
  --binary "$W/target/aarch64-apple-darwin/release/yoke" --out "$W/dist"
(cd "$W/dist" && shasum -a 256 -c ./*.sha256)
```

It built uncommitted work on top of commit `e8b8ee0`, so its checksums identify that dry run only and must never be published or compared with a release.

| Asset | Size | SHA-256 |
| --- | --- | --- |
| `yoke-aarch64-apple-darwin` | 9.0 MiB | `27dea3e49afdb5d88a5a41fefcf56ec54eb925655c63d6cce88d8f3626990820` |
| `yoke-aarch64-apple-darwin.tar.gz` | 3.5 MiB | `5dec0fdee6cb5ffb6ae25b06901865f83459148de8bc9c43ecb902f8debb918a` |
| `THIRD_PARTY_LICENSES-aarch64-apple-darwin` | 746.8 KiB | `e829055d7c0ff0a7eb201f055bedd4cd7a347e8414e3298ded6089a3c35654c3` |

Every `.sha256` file verified with `shasum -a 256 -c`.
The archive contains `yoke`, `THIRD_PARTY_LICENSES`, `LICENSE-MIT` and `LICENSE-APACHE`.
The generated notices cover 72 third-party packages from the resolved Cargo graph, and the built executable embeds them.
The packaged executable reported version `0.1.0`.
`uv run --no-project python -m unittest discover -s scripts/release -p 'test_*.py' -q` passed 2 tests, and `cargo test --locked -p yoke-cli update::` passed 6 tests.

## Notices and licenses

Notices come only from the existing generator, `scripts/release/package.py`, which copies license and nested NOTICE files from the downloaded crate sources and refuses a dependency without license text.
Never edit a generated notice file by hand.
`cargo deny check licenses sources` must pass on the release commit; the result of the local run is recorded under [Local license check](#local-license-check).

### Local license check

On 2026-10-14, `cargo deny check licenses sources` with cargo-deny 0.20.2, installed as [COMPLIANCE.md](../COMPLIANCE.md) describes, reported `licenses ok, sources ok` for the working tree.
Its only warnings named allowed licenses that no current dependency uses.

## Checklist for each release

Record the date, the tag, the workflow run URL and the person who checked each item.

1. The tag is a stable `vMAJOR.MINOR.PATCH` or `MAJOR.MINOR.PATCH` matching the `yoke-cli` package version; the workflow rejects anything else before building.
2. The workflow passed for all four targets, and the draft lists, per target, the executable, the archive, the notice file and a `.sha256` for each.
3. Each downloaded asset verifies with `shasum -a 256 -c <asset>.sha256`, and the digests are copied into the release notes.
4. Each archive contains `yoke`, `THIRD_PARTY_LICENSES`, `LICENSE-MIT` and `LICENSE-APACHE`, and `THIRD_PARTY_LICENSES` begins with `Yoke third-party licenses`.
5. Every item in [COMPLIANCE.md](../COMPLIANCE.md) is ticked, including the trademark and crates.io name checks.
6. Each platform passes the install acceptance below on a clean machine or account.
7. After publication, the update acceptance below passes from the previous release.
8. The trials in [trials.md](trials.md) are complete, with every incident resolved or accepted.

## Install acceptance

Run on each platform with the archive's executable and an empty home directory, so no real agent settings change:

1. `export HOME=$(mktemp -d)` and unset `CLAUDE_CONFIG_DIR` and `CODEX_HOME`, then run `./yoke --version`.
2. `yoke install --agent claude,codex --dry-run` lists three planned files; `yoke install --agent claude,codex` applies them, and repeating it reports `changes: 0`.
3. `yoke health --json` reports both agents installed with no problems and exits 0.
4. With `mode = "enforce"` and a `Bash` deny gate trusted through `yoke trust`, piping a `Bash` `PreToolUse` event into the installed hook command prints a denial, and a `Read` event prints nothing.
5. Repeat steps 2 and 3 with `CLAUDE_CONFIG_DIR` and `CODEX_HOME` set to absolute directories, and confirm the hooks land only there.
6. `yoke uninstall --agent claude,codex` restores the settings files, and `yoke health` then exits 1 because no agent is installed.

On 2026-10-14, steps 1 to 3 and 6 passed for the `aarch64-apple-darwin` dry-run executable in a temporary home, and `crates/yoke-cli/tests/core_workflow.rs` covers steps 2 to 6 for the development build.

## Update acceptance

Packaging requires a regular, non-symlink executable between 1 byte and 64 MiB, matching the updater's executable size limit.
Focused synthetic fixtures exercise asset naming, checksum records, archive contents, and updater replacement without publishing or replacing the installed Yoke executable.
These fixtures do not establish native execution on another platform, hosted update acceptance, or real-work trial completion.

`yoke update` reads GitHub's latest-release endpoint, which never returns drafts.
On 2026-10-14 the repository had no published release (`/releases/latest` returned 404), and `yoke update --check` failed with a download error, so update acceptance is not yet possible.
After the first publication:

1. Install the previous release in a temporary home and run `yoke update --check`; it names the new version.
2. Run `yoke update`; it downloads the executable and its `.sha256`, refuses a mismatch, and replaces the executable atomically.
3. `yoke --version` prints the new version, and `yoke health` still reports the installed hooks, because they point at the same executable path.

## Trials

The full-day observe trial and the week-long enforce trials for Claude Code and Codex are run and recorded as described in [trials.md](trials.md).
They need real calendar time and real work, so no automated test replaces them.
