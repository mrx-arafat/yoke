# Release acceptance

[Documentation index](README.md) · [Releasing](releasing.md)

This page is the checklist a human completes before publishing a Yoke release, and the record of what has been verified.
The commands that take a release from ready to published are in [Releasing](releasing.md).
Local focused checks are not release acceptance, and nothing on this page is complete until its evidence is recorded here.

A Git push never publishes a binary.
Pushing a tag runs the release workflow, which builds every target on its own native runner and creates a draft release.
A draft is invisible to `yoke update` until a human reviews it against [COMPLIANCE.md](../COMPLIANCE.md) and publishes it.
A manual `workflow_dispatch` run produces Actions artifacts only and creates no release.

## Gate status

Checked on 2026-10-16 at commit `e2745ce`.
A row reads `done` only when its evidence exists today, `pending` when a command or a change is still needed, and `human` when only a person can complete it.
No row reads `done` on a claim alone, and the release preflight in [Releasing](releasing.md#step-4-run-the-preflight) checks the machine-checkable rows again for the release commit.

| Gate | Status | Evidence | Owner |
| --- | --- | --- | --- |
| CI green | done | [CI run 38921922828](https://github.com/mrx-arafat/yoke/actions/runs/38921922828) passed for `e2745ce`: formatting, clippy, the full test suite on Ubuntu and macOS, `cargo deny check`, and the docs and scripts checks | CI |
| Installer matrix, root | done | [Installer run 38921922813](https://github.com/mrx-arafat/yoke/actions/runs/38921922813) passed the nine Linux images for `e2745ce`; [run 38729099812](https://github.com/mrx-arafat/yoke/actions/runs/38729099812) passed the same nine for `808aca8` | CI |
| Non-root installer and interactive trust | done | The same run passed `ubuntu:24.04 as a user with sudo` and `ubuntu:24.04 in a terminal`, where a terminal driver answers the questions and types the trust phrase; Linux only, because no workflow runs the installer on macOS | CI |
| Release workflow on all four targets | pending | No `release.yml` run exists: `gh run list --workflow release.yml` printed `[]` on 2026-10-16; start one with `gh workflow run release.yml --ref main` | maintainer |
| Packaging and license notices | done | The host target was packaged and verified at `ff20d07` ([dry run](#local-packaging-dry-run)), `cargo deny check licenses sources` passed ([license check](#local-license-check)), and the notices generate for all four targets; the three other builds belong to the row above | maintainer |
| Crates packageable | done | `yoke-core` packages without warnings and `yoke-cli` lists its licenses and readme; `yoke-cli` can be packaged only after `yoke-core` is on crates.io ([packaging evidence](#packaging)) | maintainer |
| Compliance ticks | human | 0 of 21 boxes in [COMPLIANCE.md](../COMPLIANCE.md) are ticked; [Compliance evidence](#compliance-evidence) is what the person ticking can rely on | maintainer |
| Trademark search | human | Not done: USPTO and EUIPO, classes 9 and 42, as COMPLIANCE.md section 3 requires | maintainer |
| Observe-day trial, Codex | human | Partial: 0.8 of one working day, recorded for 2026-10-13 to 2026-10-14 in [trials](trials.md#evidence-record) | maintainer |
| Observe-day trial, Claude Code | human | Not started: [trials](trials.md#evidence-record) records no Claude Code decision | maintainer |
| Enforce-week trial, Codex | human | Not started | maintainer |
| Enforce-week trial, Claude Code | human | Not started | maintainer |
| Console acceptance | human | The [procedure and fixtures](console-acceptance.md) exist and record no result | maintainer |
| Install acceptance | human | Steps 1, 2, 3, 5 part A and 6 passed for a packaged executable at `ff20d07`; steps 4 and 5 part B need `yoke trust` in a human terminal ([details](#install-acceptance)) | maintainer |
| Update acceptance | pending | No release is published: `gh release list` printed nothing on 2026-10-16; the replacement path needs a second release to update from ([details](#update-acceptance)) | maintainer |

## Supported targets

The packaging dry run and the install acceptance below were run at commit `ff20d07` on `main`, whose CI run [38690301997](https://github.com/mrx-arafat/yoke/actions/runs/38690301997) passed on 2026-10-15.
The release review repeats the checks for the release commit.

| Target | Release workflow runner | Status for `ff20d07` on 2026-10-15 |
| --- | --- | --- |
| `aarch64-apple-darwin` | `macos-15` | Packaging dry run and install acceptance passed on the host (see below) |
| `x86_64-apple-darwin` | `macos-15-intel` | Pending; no release workflow run exists yet |
| `x86_64-unknown-linux-gnu` | `ubuntu-24.04` | Pending; no release workflow run exists yet |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` | Pending; no release workflow run exists yet |

The three non-native targets are built only by the release workflow on native runners, as [scripts/release/README.md](../scripts/release/README.md) documents.
No local cross-compilation path is documented, so none was attempted: the development host has only the `aarch64-apple-darwin` standard library installed and no Linux linker.

### Non-native target evidence

On 2026-10-15, `gh run list --workflow release.yml --limit 5 --json databaseId,event,headSha,status,conclusion` returned an empty list and `gh release list` printed nothing, so no workflow artifacts exist for any commit.
A human starts the first run with `gh workflow run release.yml --ref main`; it produces Actions artifacts only and creates no release.
When that run completes for `ff20d07`, download its artifacts with `gh run download <run-id> -D <dir>` and record here, for each of the four targets, that every `.sha256` verifies with `shasum -a 256 -c`, the archive listing, the first notice line and the digests.
Run the `x86_64-apple-darwin` executable with `arch -x86_64 ./yoke --version` on an Apple silicon host with Rosetta, and record `file` output for the two Linux executables, which cannot run on the host.

## Local packaging dry run

The host target was packaged with exactly the workflow's steps, from a detached worktree of the release commit and into a temporary directory rather than `target/release`:

```sh
W=$(mktemp -d)
git worktree add --detach "$W/src" ff20d07
cd "$W/src"
uv run --no-project python scripts/release/package.py --target aarch64-apple-darwin --notices-only --out "$W/THIRD_PARTY_LICENSES"
CARGO_TARGET_DIR="$W/target" RUSTFLAGS='--cfg yoke_release_notices --check-cfg=cfg(yoke_release_notices)' \
  YOKE_RELEASE_NOTICES="$W/THIRD_PARTY_LICENSES" \
  cargo build --locked --release -p yoke-cli --target aarch64-apple-darwin
uv run --no-project python scripts/release/package.py --target aarch64-apple-darwin \
  --binary "$W/target/aarch64-apple-darwin/release/yoke" --out "$W/dist"
(cd "$W/dist" && shasum -a 256 -c ./*.sha256)
git worktree remove "$W/src"
```

On 2026-10-15 it built commit `ff20d07` with rustc 1.95.0, cargo 1.95.0 and uv 0.10.3 from a worktree with no local changes.
These digests identify a local dry run of the host target only; a release is accepted against the workflow's own artifacts, never against these.

| Asset | Size | SHA-256 |
| --- | --- | --- |
| `yoke-aarch64-apple-darwin` | 9688608 bytes | `4192fe6ae3773565edf2af9e620396f7c60f286605364ec9e4c372236542c7f9` |
| `yoke-aarch64-apple-darwin.tar.gz` | 3715876 bytes | `d64f7ae2958e2ffe8791d83f5ea8f032205bcb8174d6ef6b6aede8a3cbbd4113` |
| `THIRD_PARTY_LICENSES-aarch64-apple-darwin` | 764728 bytes | `e829055d7c0ff0a7eb201f055bedd4cd7a347e8414e3298ded6089a3c35654c3` |

Every `.sha256` file verified with `shasum -a 256 -c`.
The archive contains `yoke`, `THIRD_PARTY_LICENSES`, `LICENSE-MIT` and `LICENSE-APACHE`, and each extracted file is byte-identical to its source: the raw executable asset, the notice asset and the two repository license files.
The notice asset begins with `Yoke third-party licenses` and covers 72 third-party packages from the resolved Cargo graph, and the built executable embeds it once.
The packaged executable reported version `0.1.0`.

The archive digest is not reproducible between packagings of the same inputs: `scripts/release/package.py` writes the archive with Python's `tarfile` in `w:gz` mode, which records the packaging time in the gzip header and each entry's modification time.
Compare archives by their extracted contents, not by the archive digest.

The packaged executable differs from a plain `cargo build --release` of the same commit, which on this host is 8895632 bytes with SHA-256 `999db10f434e148ca20dbab584695a9b54570b5982f87b5fd800c83d25ff64a5`.
The release workflow compiles with `--cfg yoke_release_notices`, under which `crates/yoke-cli/src/update.rs` embeds the generated notices with `include_bytes!`, so the packaged executable is 792976 bytes larger and contains the notice heading once where the plain build contains it zero times.
Only an executable built with the notices is a release candidate.

From the same worktree, `cargo test --locked -p yoke-cli update:: --quiet` passed 6 tests and `uv run --no-project python -m unittest discover -s scripts/release -p 'test_*.py' -q` passed 3 tests.

## Notices and licenses

Notices come only from the existing generator, `scripts/release/package.py`, which copies license and nested NOTICE files from the downloaded crate sources and refuses a dependency without license text.
Never edit a generated notice file by hand.
`cargo deny check licenses sources` must pass on the release commit; the result of the local run is recorded under [Local license check](#local-license-check).

### Local license check

On 2026-10-15, `cargo deny check licenses sources` with cargo-deny 0.20.2, installed as [COMPLIANCE.md](../COMPLIANCE.md) describes, reported `licenses ok, sources ok` from the `ff20d07` worktree used for the packaging dry run.
Its only warnings named three allowed licenses that no current dependency uses: `BSD-3-Clause`, `ISC` and `Zlib`.

## Compliance evidence

Collected on 2026-10-16 at commit `e2745ce`.
Every check only reads, and nothing was published.
Ticking [COMPLIANCE.md](../COMPLIANCE.md) stays a human act, and this is the evidence the person ticking can rely on.

### crates.io names

| Name | crates.io API | Result |
| --- | --- | --- |
| `yoke-cli` | HTTP 404 | Free |
| `yoke-core` | HTTP 404 | Free |
| `yoke` | HTTP 200 | Taken by an unrelated crate, version 0.8.3 from `unicode-org/icu4x`, as COMPLIANCE.md says |

`cargo search yoke-cli` and `cargo search yoke-core` print no result.
A name can be taken at any time, so repeat the check immediately before the first publish.

### Other products' names

This search runs over the tracked files with the names of other agent tools and editors, written with one letter in brackets so that this page does not match itself:

```sh
git grep -n -i -I -E 'p[i]lot|w[i]ndsurf|c[u]rsor|a[i]der|c[l]ine|o[p]encode|r[o]o ?code|k[i]ro|g[e]mini|a[n]tigravity|m[o]mentic' -- . ':!Cargo.lock' ':!docs/assets'
```

At `e2745ce` it matches 32 lines in 10 files.
Three lines are in COMPLIANCE.md, which names another product on purpose in its clean-room rules.
The other 29 are ordinary words that merely contain one of the names, in stylesheets, pagination and error text.
No tracked path contains another product's name.
Commit `689821a` replaced the one test fixture that used another product's name as a rejected key with a neutral placeholder, so the search no longer finds a product name outside COMPLIANCE.md.

### Packaging

`[workspace.package]` in `Cargo.toml` carries `repository`, `homepage`, `readme`, `keywords`, `categories` and `rust-version = "1.95"`, and both crates inherit them.
Each crate directory holds `LICENSE-MIT` and `LICENSE-APACHE` as symlinks to the files at the repository root, and the root `README.md` is the packaged readme.
The following ran from `git archive` output of `e2745ce` into a scratch target directory:

```sh
cargo package -p yoke-core --no-verify
cargo package -p yoke-cli --no-verify
cargo package -p yoke-core --no-verify --list
cargo package -p yoke-cli --no-verify --list
```

- `yoke-core` printed `Packaged 72 files, 707.6KiB (178.1KiB compressed)` with no metadata warning, and its listing includes `LICENSE-APACHE`, `LICENSE-MIT` and `README.md`.
- The `yoke-cli` listing has 94 files and includes the same three.
- Packaging `yoke-cli` fails with `no matching package named yoke-core found`, searched in the crates.io index, because `yoke-cli` depends on `yoke-core` by path and version and `yoke-core` is not published.
  That is the order of publication, not a defect, so the first publish of `yoke-core` is also the first full check of `yoke-cli`.
- At `808aca8`, before the change, the same command failed with `all dependencies must have a version requirement specified when packaging`, and neither listing contained a license or readme file.

## Checklist for each release

Record the date, the tag, the workflow run URL and the person who checked each item.

1. The tag is a stable `vMAJOR.MINOR.PATCH` matching the `yoke-cli` package version; the workflow starts on no other tag and rejects a mismatch before building.
2. The workflow passed for all four targets, and the draft lists, per target, the executable, the archive, the notice file and a `.sha256` for each, plus one `SHA256SUMS` for all of them.
   `gh attestation verify` succeeds for the executables and `SHA256SUMS` as [Verifying releases](verifying-releases.md) shows; this has not been run, because no tagged run exists yet.
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
5. Part A: repeat steps 2, 3 and 6 with `CLAUDE_CONFIG_DIR` and `CODEX_HOME` set to absolute directories, and confirm the hooks land only there.
   Part B: repeat step 4 there, reading the hook commands from those directories.
6. `yoke uninstall --agent claude,codex` restores the settings files, and `yoke health` then exits 1 because no agent is installed.

Steps 1, 2, 3, 5 part A and 6 can be run by an agent; step 4 and step 5 part B require a human terminal, because `yoke trust` refuses agent processes by design.

On 2026-10-15, steps 1, 2, 3, 5 part A and 6 passed for the `ff20d07` dry-run executable of `aarch64-apple-darwin` in a temporary home.
The dry run printed `changes: 3` with the three planned files and created nothing, the apply wrote them, the repeat printed `changes: 0`, and `yoke health --json` exited 0 with both agents installed and empty `problems` lists; its status was `attention` only because no hook event or trusted policy existed yet.
With `CLAUDE_CONFIG_DIR` and `CODEX_HOME` set to absolute temporary directories, the same three files landed only under those directories, and the `.claude` and `.codex` directories under the home stayed empty.
After `yoke uninstall --agent claude,codex` the three files were gone and `yoke health` exited 1 with `Yoke hooks are not installed for any agent.`
Step 4 and step 5 part B were not run for the packaged executable, because `yoke trust` requires a human terminal.
For the development build, `crates/yoke-cli/tests/core_workflow.rs` covers steps 2 and 6 in `install_dry_run_roundtrip_and_agent_specific_events_are_isolated`, steps 3 to 6 in `install_follows_agent_config_dirs_and_health_stays_read_only`, and the enforce denial with the silent allow in `untrusted_and_observe_are_silent_then_enforce_uses_snapshot_and_redacted_log`.
Those tests commit the trust snapshot through the library, and `off_exits_before_parse_or_state_and_trust_has_no_agent_bypass` pins that `yoke trust` refuses agents and that `--yes` is not a bypass.
To complete step 4 by hand, run the following in your own terminal after step 2, from a project directory inside the temporary home:

```sh
printf "mode='enforce'\n[[gate]]\nname='review'\non='PreToolUse'\naction='deny'\nmatch_tool='Bash'\nmessage='review required'\n" > yoke.toml
yoke trust
HOOK=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["hooks"]["PreToolUse"][0]["hooks"][0]["command"])' "$HOME/.claude/settings.json")
printf '{"session_id":"accept","tool_name":"Bash","tool_input":{"command":"echo test"}}' | sh -c "$HOOK"
printf '{"session_id":"accept","tool_name":"Read","tool_input":{"file_path":"README.md"}}' | sh -c "$HOOK"
```

The first pipe prints a `hookSpecificOutput` whose `permissionDecision` is `deny`, the second prints nothing, and repeating both with the `PreToolUse` command from `$HOME/.codex/hooks.json` gives the same results.
For step 5 part B, run the same commands after step 5 part A's install, reading the hook command from `$CLAUDE_CONFIG_DIR/settings.json` and then from `$CODEX_HOME/hooks.json` instead.
These commands mirror the development-build test and have not been run by an agent.

## Update acceptance

Packaging requires a regular, non-symlink executable between 1 byte and 64 MiB, matching the updater's executable size limit.
Focused synthetic fixtures exercise asset naming, checksum records, archive contents, and updater replacement without publishing or replacing the installed Yoke executable.
These fixtures do not establish native execution on another platform, hosted update acceptance, or real-work trial completion.

`yoke update` reads GitHub's latest-release endpoint, which never returns drafts.
On 2026-10-15 the repository still had no published release: `/releases/latest` returned HTTP 404.
`yoke update --check` from the `ff20d07` dry-run executable in a temporary home exited 1 with `release check failed (exit status: 56); check connectivity and release availability`, where 56 is the status curl 8.7.1 on the host returns for that 404 over HTTP/2, so update acceptance is not yet possible.
On 2026-10-16 `gh release list` still printed nothing.

The first release cannot complete update acceptance, because replacing an executable needs an older release to update from.
After the first publication, [Releasing](releasing.md#step-8-check-the-published-release) checks that `yoke update --check` prints `latest_version: v0.1.0` and that `yoke update` prints `update: already current (0.1.0)`.
After the second publication, update acceptance is:

1. Install the previous release in a temporary home and run `yoke update --check`; it names the new version.
2. Run `yoke update`; it downloads the executable and its `.sha256`, refuses a mismatch, and replaces the executable atomically.
3. `yoke --version` prints the new version, and `yoke health` still reports the installed hooks, because they point at the same executable path.

## Trials

The full-day observe trial and the week-long enforce trials for Claude Code and Codex are run and recorded as described in [trials.md](trials.md).
They need real calendar time and real work, so no automated test replaces them.
