# Verifying releases

This page says how to check that a Yoke executable you downloaded is the one the release workflow built, and what each check does and does not prove.

**Status.**
No release has been published yet, so none of this has been run against a real release.
The workflow steps that create the checksum list and the build provenance exist in [`release.yml`](../.github/workflows/release.yml) but have not run.
Treat the commands below as the procedure to confirm on the first release, not as an observed result.

## What a release carries

Every release asset is covered by three things:

| What | Where | What it proves |
| --- | --- | --- |
| `<asset>.sha256` | next to the asset | The file is the one that was uploaded. It comes from the same place as the file, so it catches a damaged or truncated download, not a replaced release. |
| `SHA256SUMS` | a release asset | The same digests for every asset in one list, also useful for checking an executable that is already installed. It has the same limit as the `.sha256` files. |
| Build provenance | GitHub's attestation store, not a file in the release | A signed statement, made by the `release.yml` job in this repository, that names each asset by digest, the commit and the tag it was built from. Someone who replaces an asset on the release page cannot produce it. |

The provenance is what ties a file to a workflow run.
It does not say the code is safe, and it does not help if the repository, the workflow or the release reviewer was already compromised when the run happened.
Restricting who can publish is what the `release` environment is for (see [Maintainer setup](#maintainer-setup)).

## Check an executable

You need the [GitHub CLI](https://cli.github.com/) signed in with `gh auth login`.
Replace the tag and the target with yours.

```sh
TAG=v0.1.0
TARGET=aarch64-apple-darwin
cd "$(mktemp -d)"
gh release download "$TAG" --repo mrx-arafat/yoke --pattern "yoke-$TARGET" --pattern SHA256SUMS
gh attestation verify "yoke-$TARGET" --repo mrx-arafat/yoke \
  --signer-workflow mrx-arafat/yoke/.github/workflows/release.yml \
  --source-ref "refs/tags/$TAG"
grep " yoke-$TARGET\$" SHA256SUMS | shasum -a 256 -c -
```

The `gh attestation verify` command prints `Verification succeeded` and the workflow, repository and commit that built the file.
Stop on any failure: do not run the file.
On Linux, use `sha256sum -c -` instead of `shasum -a 256 -c -`.

To check the executable that `install.sh` or `yoke update` already put in place, compare its digest with the line for your target in `SHA256SUMS`.
Both put the raw `yoke-<target>` asset in place unchanged.

```sh
shasum -a 256 "$(command -v yoke)"
grep " yoke-$TARGET\$" SHA256SUMS
```

The two digests are equal for an installed release executable.
They differ for an executable you built yourself, which is expected.

## What `install.sh` and `yoke update` check themselves

Both download `yoke-<target>` and `yoke-<target>.sha256` from the same release and refuse a mismatch.
`yoke update` also accepts only the asset URLs that GitHub's release metadata names, only over HTTPS, only a newer version, and runs the downloaded executable once with `--version` before it replaces the installed one.
Neither checks the build provenance, because that needs a verifier that is not built in.
If you want that guarantee, verify the file as above before you run the installer's output or `yoke update`, or install by hand from a verified download.

## The installer script itself

The one-line command in the README fetches `install.sh` from the `main` branch, so it runs whatever `main` holds at that moment.
Once a release exists, fetch the script from the release tag instead, so it is the version you reviewed:

```sh
curl -fsSL https://raw.githubusercontent.com/mrx-arafat/yoke/v0.1.0/install.sh | bash
```

If it needs Rust and you let it, the installer runs rustup's own installer without a checksum, after asking; install Rust yourself first if you want to verify that download.

## Building from source

When `install.sh` builds from source it uses one exact revision, never a branch that moves: the commit in `YOKE_COMMIT`, the tag in `YOKE_VERSION`, the newest release tag, or, while no release exists, the commit pinned in `install.sh`.
To see which one it used, read the line it prints before `cargo install`.
A tag can in principle be moved by someone who can write tags, so protect `v*` tags (see below) or build a full commit hash with `YOKE_COMMIT`.

## Maintainer setup

These settings live in the repository on GitHub, not in the code, so they cannot be checked from a clone.
Confirm each one before the first release tag:

1. **Environment `release`** (Settings, Environments): add required reviewers.
   The job that attests and drafts the release runs in this environment, so a tag alone does not sign or publish anything.
2. **Tag protection** (Settings, Rules): a ruleset for `v*` tags that restricts who can create, move and delete them.
3. **Branch protection on `main`**: require the `CI` checks.
   The release gate also refuses a tagged commit that is not in `main` or has no successful CI run there, but it relies on `main` being protected from direct force-pushes.
4. **Two-factor authentication** for everyone with write access.

Until these are set, the build provenance still names the workflow and commit, but anyone with write access could produce it.
