# Releasing Yoke

[Documentation index](README.md) · [Release acceptance](release-acceptance.md)

This runbook is the one path from a release candidate to a published release.
[Release acceptance](release-acceptance.md) holds the gate status and the recorded evidence.
This page holds the commands.
Run it from a clone of `mrx-arafat/yoke` on `main`, as a maintainer who can push tags and publish releases.
You need the GitHub CLI (`gh`) signed in, `uv`, `cargo`, `cargo-deny`, and `shasum` (on Linux, `sha256sum`).
Every step names what a passing result looks like.
Stop at the first step that does not pass.

## How a release works

A push never publishes a binary.
Pushing a tag builds a draft, and a human publishes the draft.

| Workflow | Runs | What it proves |
| --- | --- | --- |
| `ci.yml` | every push to `main` and every pull request | formatting, clippy, the full test suite on Ubuntu and macOS, `cargo deny check`, the generated changelog, the release packaging tests and the Markdown link check |
| `installer.yml` | a push or pull request that changes `install.sh` or the workflow itself, and manual dispatch | `install.sh` on fresh Linux: as root in nine containers, as a user with sudo, and in a terminal |
| `release.yml` | every pushed `vMAJOR.MINOR.PATCH` tag, and manual dispatch | for a tag, that the commit is in `main` and passed CI there; the full `cargo deny check`; focused tests, notices, a build with the notices embedded on a pinned toolchain, packaging and checksums on four native runners; then `SHA256SUMS` and signed build provenance for every asset |

The release workflow runs these targets, each on its own native runner:

| Target | Runner |
| --- | --- |
| `aarch64-apple-darwin` | `macos-15` |
| `x86_64-apple-darwin` | `macos-15-intel` |
| `x86_64-unknown-linux-gnu` | `ubuntu-24.04` |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` |

Each target produces six assets:

- `yoke-<target>` is the raw executable that `yoke update` and `install.sh` download.
  It embeds the license notices.
- `yoke-<target>.tar.gz` holds `yoke`, `THIRD_PARTY_LICENSES`, `LICENSE-MIT` and `LICENSE-APACHE`.
- `THIRD_PARTY_LICENSES-<target>` is the readable notice file.
- Each of those three has a `.sha256` file: one lowercase digest, two spaces, and the file name.

A manual run of `release.yml` builds and uploads the same files as Actions artifacts and creates no release, because the job that creates the release runs only for a tag.
A tag run builds again and then creates a draft release named `Yoke <tag>` with all 25 assets: the 24 per-target files and `SHA256SUMS`, which lists the checksum of each of them.
The job that does this runs in the `release` environment, attests every asset with GitHub build provenance, and is the only job with permission to sign or publish.
In the repository settings, give the `release` environment required reviewers and protect `v*` tags, so that nobody can sign or publish alone; GitHub creates an unprotected `release` environment on its first use otherwise.
[Verifying releases](verifying-releases.md) says how a user checks the result and what that check does not prove.
A draft is invisible to the two things users run.
`yoke update` reads GitHub's latest-release endpoint, which never returns a draft or a prerelease.
`install.sh` downloads `https://github.com/mrx-arafat/yoke/releases/latest/download/yoke-<target>`, builds the newest release tag from source when that asset does not exist (the commit pinned in `install.sh` while no release is published), and always builds from source on musl systems such as Alpine, because no musl executable is released.

## Choose the tag

Use `v` plus the `yoke-cli` version, for example `v0.1.0` for version `0.1.0` in `[workspace.package]` of `Cargo.toml`.
`release.yml` starts only on `vMAJOR.MINOR.PATCH` tags, so a tag without the `v` never builds.
`scripts/release/package.py --tag` checks the tag with digits only, no leading zeros, no pre-release or build suffix, and a number equal to the `yoke-cli` version.
Checked on 2026-10-16 against version `0.1.0`: `v0.1.0` passes, and `v0.1.1`, `v0.1.0-rc1`, `V0.1.0` and `v01.1.0` exit 2 with `Release tag must be a stable version matching yoke-cli`.
Push no tag of the form `vMAJOR.MINOR.PATCH` that is not a release.
A wrong version runs the gate, the dependency check and all four package jobs, fails in the notices step after the focused tests, and creates no draft.
A tag on a commit that is not in `main`, or that has no successful CI run on `main`, stops at the gate before anything is built.

Set the tag once.
Every later block assumes it is set in the same shell.

```sh
cd /path/to/your/clone/of/yoke
TAG=v0.1.0
```

## Verify a set of assets

Steps 3 and 6 run this block on a directory `D` that holds the downloaded assets of one run or one draft.
Set `REF` to the commit the assets were built from: `REF=$(git rev-parse HEAD)` for a rehearsal, and `REF=$TAG` for the draft.

```sh
ls "$D" | wc -l
find "$D" -name '*.sha256' -execdir shasum -a 256 -c {} \;
for t in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu; do
  echo "== $t"
  X=$(mktemp -d)
  tar -tzf "$D/yoke-$t.tar.gz"
  tar -xzf "$D/yoke-$t.tar.gz" -C "$X"
  cmp "$X/yoke" "$D/yoke-$t" && echo "archive executable matches the raw asset"
  cmp "$X/THIRD_PARTY_LICENSES" "$D/THIRD_PARTY_LICENSES-$t" && echo "archive notices match the notice asset"
  git show "$REF:LICENSE-MIT" | cmp - "$X/LICENSE-MIT" && echo "LICENSE-MIT matches the commit"
  git show "$REF:LICENSE-APACHE" | cmp - "$X/LICENSE-APACHE" && echo "LICENSE-APACHE matches the commit"
  head -1 "$D/THIRD_PARTY_LICENSES-$t"
  echo "notice packages: $(grep -c '^=== ' "$D/THIRD_PARTY_LICENSES-$t")"
  echo "notice heading inside the executable: $(grep -a -c 'Yoke third-party licenses' "$D/yoke-$t")"
  rm -rf "$X"
done
```

It passes when all of these hold:

- The directory holds 24 files.
- Twelve checksum lines print and every one ends in `OK`.
  The `.sha256` files name files without a directory, so the checksums must run beside the files, which `-execdir` does.
- Every archive lists exactly `yoke`, `THIRD_PARTY_LICENSES`, `LICENSE-MIT` and `LICENSE-APACHE`, and the four `matches` lines print for every target.
- Every notice file begins with `Yoke third-party licenses`, and the executable embeds that heading once.
  A plain `cargo build --release` embeds it zero times and is not a release executable.
- The package count is 72 for the two Apple targets and 73 for the two Linux targets.
  That is what `package.py --notices-only` produced locally on 2026-10-16 for the current dependency graph.
  A different count means the graph changed, so confirm `cargo deny check licenses sources` still passes and record the new numbers.

Then run what the host can run, with a throwaway home so no real Yoke state is touched.
Set `HOST` to the target of the machine you are on.

```sh
chmod +x "$D/yoke-$HOST"
HOME=$(mktemp -d) "$D/yoke-$HOST" --version
```

It prints `0.1.0` for version 0.1.0.
On Apple silicon with Rosetta, run `arch -x86_64 "$D/yoke-x86_64-apple-darwin" --version` for the Intel executable.
For the two Linux executables, `file "$D/yoke-x86_64-unknown-linux-gnu"` and `file "$D/yoke-aarch64-unknown-linux-gnu"` report ELF executables for x86-64 and for aarch64.
Run them on matching Linux machines when you have them.

## Step 1. Confirm the gates

Open the [gate status table](release-acceptance.md#gate-status).
Every row must read `done` before you tag, and every `human` row must carry the name and date of the person who completed it.
Two rows are completed by this runbook instead of before it: the release workflow on all four targets (step 3) and update acceptance (step 8, and only fully by the next release).
Do not continue while a `pending` row has no owner.

## Step 2. Prepare the release commit

The tagged commit, the rehearsed commit and the commit CI passed on must be the same commit, so make every repository change first.

1. Set the status of the change record to the planned release date.
   In `docs/changes/0.1.0.json`, change `"status": "unreleased"` to an ISO date such as `"2026-10-20"`.
   Keep exactly the four fields `version`, `status`, `added` and `pending_acceptance`.
2. Rewrite `pending_acceptance` in the same file so that every entry is still true after the release.
   Delete the entries the release completes, such as "Version 0.1.0 remains unreleased".
   The list may be empty.
3. Check that `docs/release-notes-0.1.0.md` is ready, because step 6 turns it into the release body.
4. Regenerate the changelog with the script.
   Never edit `CHANGELOG.md` by hand.

   ```sh
   uv run --no-project python scripts/changelog.py
   uv run --no-project python scripts/changelog.py --check
   ```

   The second command prints `changelog is current`.
5. Commit the two files with a sign-off and push.

   ```sh
   git add docs/changes/0.1.0.json CHANGELOG.md
   git commit -s -m "release: date 0.1.0"
   git push origin main
   ```

6. Wait for CI on the pushed commit.
   Run `gh run list --commit "$(git rev-parse HEAD)"` until `CI` reads `completed` and `success`.

If the release slips past the date you wrote, change the date and repeat this step.

## Step 3. Rehearse on all four targets

The rehearsal runs the real workflow at the release commit, creates no release, and proves the three non-native targets build.
The Installer workflow is limited to changes of `install.sh` and itself, so a release commit usually has no Installer run, and the preflight in step 4 requires one.
Wait for any running Installer to finish first, because a dispatch cancels an in-progress run on the same branch.

```sh
gh workflow run installer.yml --ref main
gh workflow run release.yml --ref main
gh run list --commit "$(git rev-parse HEAD)" --json databaseId,workflowName,event,status,conclusion
```

Dispatch runs at the tip of `main`, so push nothing between step 2 and the end of step 5.
Wait for both runs with their ids.

```sh
gh run watch <installer-run-id> --exit-status
gh run watch <release-run-id> --exit-status
```

Both must exit 0.
Then download the release run's artifacts and check them.
`gh run download` places each artifact in a directory of its own, so copy the files into one flat directory first.

```sh
R=$(mktemp -d)
D=$(mktemp -d)
gh run download <release-run-id> -D "$R"
find "$R" -type f -exec cp {} "$D"/ \;
REF=$(git rev-parse HEAD)
```

Run [Verify a set of assets](#verify-a-set-of-assets) on `D`.
Record the run URLs and the results in [release acceptance](release-acceptance.md).

## Step 4. Run the preflight

The preflight is read-only.
It runs `git` and `gh` with argument lists and timeouts, writes nothing, and creates no tag.

```sh
uv run --no-project python scripts/release/preflight.py --tag "$TAG"
```

It prints `PASS`, `FAIL` or `SKIP` for eight checks and exits 1 when any check fails.

| Check | Passes when |
| --- | --- |
| 1. tag format | the tag follows the rule in [Choose the tag](#choose-the-tag) and equals the `yoke-cli` version |
| 2. clean tree | `git status` shows no change, ignoring untracked `.codegraph/` and `.lavish/` |
| 3. HEAD is pushed | `HEAD` equals its upstream branch |
| 4. install.sh pinned commit | `PINNED_COMMIT` in `install.sh` is a commit in `HEAD`'s history |
| 5. changelog | `scripts/changelog.py --check` passes and the change record is dated |
| 6. CI green for HEAD | the latest `CI` and `Installer` runs for `HEAD` both concluded `success` |
| 7. release workflow evidence | a successful `release.yml` run exists for `HEAD` |
| 8. tag not yet used | the tag exists neither locally nor on `origin` |

Continue only when the summary reports no failure and no skip.
A `SKIP` means a check could not run, for example `gh` is missing or not signed in, and is not a pass.
The script ends by listing the human gates it cannot check, which are the same rows of the gate table that read `human`.
Its usage line is `preflight.py --tag TAG`, described in [scripts/release/README.md](../scripts/release/README.md).

## Step 5. Tag and push

Pushing the tag starts the workflow and is the last step that is cheap to undo.
Publishing in step 7 is the first step that reaches users.

```sh
git tag -a "$TAG" -m "Yoke $TAG"
git push origin "$TAG"
```

Find the run and wait for it.

```sh
gh run list --workflow release.yml --event push --limit 1 --json databaseId,headSha,status,conclusion
gh run watch <run-id> --exit-status
gh release view "$TAG" --json isDraft,assets --jq '[.isDraft, (.assets | length)]'
```

The run's `headSha` equals the commit you rehearsed, and the last command prints `[true,25]`.
The draft job needs all four package jobs, so no draft exists unless every target built.
If anything differs, go to [When the workflow fails](#when-the-workflow-fails).

## Step 6. Review the draft

Download the draft and check it as you checked the rehearsal.

```sh
D=$(mktemp -d)
gh release download "$TAG" -D "$D"
REF=$TAG
```

1. Run [Verify a set of assets](#verify-a-set-of-assets) on `D`, then verify the build provenance of the executables and `SHA256SUMS` as [Verifying releases](verifying-releases.md) describes.
   Provenance cannot be checked before the first tagged run, so this is the first time it is exercised.
2. Confirm that every box in [COMPLIANCE.md](../COMPLIANCE.md) is ticked at the tag.
   The command prints `0`.

   ```sh
   git show "$TAG:COMPLIANCE.md" | grep -c -- '- \[ \]'
   ```

3. Run the [install acceptance](release-acceptance.md#install-acceptance) with the downloaded executable of each platform you have.
   Complete its two human steps, which need `yoke trust` in your own terminal.
4. Turn `docs/release-notes-0.1.0.md` into the release body.
   Copy it to a scratch file and edit the copy:
   - Remove the draft banner and `(Draft)` from the title.
   - Rewrite every sentence that stops being true at publication, such as "While no release is published".
   - Drop the pending items this release completes.
   - Replace relative links such as `upgrade-notes.md` with `https://github.com/mrx-arafat/yoke/blob/<tag>/docs/upgrade-notes.md`, because relative links do not resolve on a release page.
   - Replace the closing sentence of its Checksums section with the digests printed by `(cd "$D" && cat ./*.sha256)`.
5. Save it as the release body.
   The workflow's placeholder body is a note to the reviewer and must not ship.

   ```sh
   gh release edit "$TAG" --notes-file <edited-notes-file>
   ```

6. Record the date, the tag, the run URL and your name for each item in [release acceptance](release-acceptance.md).

## Step 7. Publish

Publish only when every check in step 6 passed.

```sh
gh release edit "$TAG" --draft=false
```

## Step 8. Check the published release

```sh
gh release view "$TAG" --json isDraft,isPrerelease,tagName --jq '[.tagName, .isDraft, .isPrerelease]'
gh api repos/mrx-arafat/yoke/releases/latest --jq .tag_name
```

The first prints `["v0.1.0",false,false]` and the second prints `v0.1.0`.

Check the updater with a throwaway home, using the downloaded executable of your platform.

```sh
T=$(mktemp -d)
env -u CLAUDE_CONFIG_DIR -u CODEX_HOME HOME="$T" "$D/yoke-$HOST" update --check
env -u CLAUDE_CONFIG_DIR -u CODEX_HOME HOME="$T" "$D/yoke-$HOST" update
```

The first prints `latest_version: v0.1.0`.
The second prints `update: already current (0.1.0)`, because `yoke update` installs only a strictly newer version.
For the first release this is all the updater check that can run, since replacement needs an older release to start from.
[Later releases](#later-releases) complete it.

Check that the one-line installer now downloads instead of building.
This run installs into a throwaway directory, connects no agent, and edits no shell startup file.

```sh
T=$(mktemp -d)
(cd "$T" && env -u CLAUDE_CONFIG_DIR -u CODEX_HOME HOME="$T" YOKE_INSTALL_DIR="$T/bin" YOKE_AGENTS=none YOKE_NO_MODIFY_PATH=1 YOKE_YES=1 \
  sh -c 'curl -fsSL https://raw.githubusercontent.com/mrx-arafat/yoke/main/install.sh | bash')
HOME="$T" "$T/bin/yoke" --version
```

The output contains `downloading https://github.com/mrx-arafat/yoke/releases/latest/download/yoke-<target>`, then `verified SHA-256 checksum`, then `installed Yoke 0.1.0`.
It must not contain `no release executable found` or `building Yoke from source`.
The installer ends with `yoke health` reporting that no hooks are installed, and it still exits 0.
That is expected, because this run connected no agent.
The only file it leaves under the throwaway home is `bin/yoke`.
On a musl system it builds from source by design, so run this check on glibc Linux or macOS.

## Step 9. Update what the release makes stale

Several pages say that no release exists.
List them and fix each one that is not a dated record.

```sh
git grep -n -i -E 'no (binary )?release|none have been published|while no release|has nothing to install|not (yet )?(been )?published' -- '*.md' ':!docs/releasing.md' ':!CHANGELOG.md'
```

At the time of writing the live statements are in `README.md` (the notice at the top, the Quickstart, and the Status section), `docs/getting-started.md`, `docs/upgrade-notes.md`, `SECURITY.md`, and `docs/release-acceptance.md`.
Dated records, such as the trial evidence and the specs, stay as they are.
Then update the [gate table](release-acceptance.md#gate-status) with the release run URL and the checks above, and commit the documentation.
Documentation commits after the tag are fine, because they change no binary.

## Withdraw a bad release

Decide first whether anything was published.

**Still a draft.**
Nothing outside GitHub has seen it, so delete the draft and its tag and start again.

```sh
gh release delete "$TAG" --cleanup-tag --yes
git tag -d "$TAG"
```

**Already published.**
Stop distribution first, then fix forward.

1. Return the release to a draft.

   ```sh
   gh release edit "$TAG" --draft
   ```

   `/releases/latest` then names the previous published release, or returns 404 when there is none.
   `yoke update` stops offering the bad version, and `install.sh` stops downloading it.
   With no published release left, `install.sh` builds the commit pinned in it from source, so check that `PINNED_COMMIT` is still a commit you stand behind before telling anyone to reinstall.
   If GitHub refuses the edit, the release is immutable (`gh release view "$TAG" --json isImmutable`), and you can only supersede it.
2. Never move or reuse a published tag, and never replace assets under the same version.
   Ship the fix as the next patch version through this whole runbook.
   `yoke update` installs only a strictly newer version and otherwise prints `update: already current`, so a same-version fix never reaches the people who already installed the bad one.
3. To send a person back to an earlier release, they run the installer with `YOKE_VERSION=<earlier tag>` set.
   Read the [upgrade notes](upgrade-notes.md) first, because a newer binary migrates the state database and there is no downgrade command.
4. Say what happened in the `added` or `pending_acceptance` entries of the next change record.

## When the workflow fails

A tag run that fails creates no draft, because the draft job needs all four package jobs.

```sh
gh run view <run-id> --log-failed
```

- A wrong tag name fails in the notices step.
  Delete the remote tag with `git push origin --delete "$TAG"` and the local tag with `git tag -d "$TAG"`, then push the right one.
- A build or test failure needs a fix on `main`.
  Delete the tag the same way, repeat steps 2 to 5 for the new commit, and reuse the name.
  Reusing the name is safe only because nothing was published.
- A failure in the draft job can leave a partial draft.
  Check `gh release list` and delete any partial draft as above.
  Then re-run the failed job with `gh run rerun <run-id> --failed`.

## Later releases

1. Change `version` in `[workspace.package]` of `Cargo.toml`, run `cargo check` so `Cargo.lock` records it, and commit both files.
2. Add `docs/changes/<version>.json` with the four fields, `status` set to `unreleased`, and the file name equal to the version.
   Add `docs/release-notes-<version>.md` as well.
3. Follow steps 1 to 9, using `docs/release-notes-<version>.md` in step 6.
4. In step 8, also complete update acceptance from the previous release.
   Download only its executable, update it in a throwaway home, and confirm the new version.

   ```sh
   P=$(mktemp -d)
   T=$(mktemp -d)
   gh release download <previous-tag> -p "yoke-$HOST" -D "$P"
   chmod +x "$P/yoke-$HOST"
   env -u CLAUDE_CONFIG_DIR -u CODEX_HOME HOME="$T" "$P/yoke-$HOST" update
   env -u CLAUDE_CONFIG_DIR -u CODEX_HOME HOME="$T" "$P/yoke-$HOST" --version
   ```

   `update` prints `updated_version: <new tag>` and replaces the executable in place.
   The version command then prints the new version.

## Publishing the crates

Publishing to crates.io is separate from the GitHub release and is not part of the steps above.
It cannot be undone: a published version can be yanked but never replaced or deleted.
Do it only when the crates gate in the [gate table](release-acceptance.md#gate-status) is `done` and the names are still free (`cargo search yoke-cli` and `cargo search yoke-core` print nothing).
`yoke-cli` depends on `yoke-core` by version, so publish `yoke-core` first, wait for it to appear in the index, and then publish `yoke-cli`.

```sh
cargo publish -p yoke-core
cargo publish -p yoke-cli
```

Until `yoke-core` is on crates.io, `cargo package -p yoke-cli` fails with `no matching package named yoke-core found`, so the first publish is also the first full check of `yoke-cli`.
