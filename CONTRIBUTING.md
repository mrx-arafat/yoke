# Contributing to Yoke

Read [COMPLIANCE.md](COMPLIANCE.md) before contributing.
Yoke is a clean-room implementation based on public documentation and idea-level concepts.

## Repository map

| Path | Responsibility |
| --- | --- |
| `crates/yoke-cli/src/main.rs` | CLI command definitions and dispatch entry point |
| `crates/yoke-cli/src/commands.rs` | Policy inspection, trust, and integration command dispatch |
| `crates/yoke-cli/src/hook.rs` | Agent hook orchestration |
| `crates/yoke-cli/src/install.rs` | Managed agent installation and removal |
| `crates/yoke-cli/src/console/` | Console HTTP, rendering, actions, and browser assets |
| `crates/yoke-core/src/config.rs` and `trust.rs` | Configuration validation and trusted snapshots |
| `crates/yoke-core/src/engine.rs` and `wire.rs` | Gate evaluation and agent output contracts |
| `crates/yoke-core/src/plan/` | Approved checks, fingerprints, and evidence |
| `crates/yoke-core/src/memory/` | Memory processing and portable archives |
| `crates/yoke-core/src/store/` | SQLite migrations and read models |
| `docs/` | User documentation, design records, and release records |
| `scripts/` | Changelog generation, release packaging and preflight, and the Markdown link and dash checker |

The CLI handles process integration and presentation; core owns policy, persistence, and workflow behavior.
Executable policy comes from human-trusted snapshots rather than unapproved disk changes.
Preserve that boundary when adding commands or hook behavior.

## Build and focused validation

```sh
cargo build --workspace --locked
cargo fmt --all -- --check
```

Run the smallest existing tests covering your change.
For example, configuration changes can be checked with:

```sh
sh scripts/test-isolated-home.sh cargo test -p yoke-core config::tests -- --quiet
```

Inspect nearby test names before selecting filters for another module.
Add the fewest behavioral regressions necessary and keep external agent credentials out of ordinary automated tests.
Hook changes need real payload/output checks; UI changes need browser interaction and a subsequent snapshot of the resulting state.
Unit checks alone do not establish agent integration or rendered UI correctness.
To fake GitHub in a test, point `YOKE_CURL` at a stub script: `yoke update` and the daily check run curl from a fixed system path, so a stub on `PATH` is not used, and only debug and `honor-home-env` builds honour `YOKE_CURL`.

## Testing against real state

A hook uses the account's own home directory, not `HOME`, whenever the two differ and the account home already holds Yoke state.
Only a debug build or a build made with `--features honor-home-env` honours `HOME`.
So never run an optimized (`--release`) binary on a machine that has a real `~/.yoke` and expect an isolated `HOME` to protect it: the hook writes to the real database.
For an optimized run under an isolated home, build into a throwaway target directory and delete it afterwards:

```sh
CARGO_TARGET_DIR="$(mktemp -d)" cargo build --release -p yoke-cli --features honor-home-env
```

Never build into the `target/release` that installed hooks run, and never enable the feature in `release.yml` or the installer.
CI and containers have no real state, so `cargo test` and release builds there need no feature.
`cargo test --release` needs `--features honor-home-env`, because the integration tests rely on `HOME`.
Before any run that could touch a real `~/.yoke`, set `HOME` to a fresh directory with `T=$(mktemp -d) && export HOME="${T:?}"`.

## Continuous integration

`.github/workflows/ci.yml` runs on every push to `main` and on every pull request.
It checks formatting, runs clippy with warnings denied, runs the full `cargo test --workspace --locked` suite on Ubuntu and macOS through `scripts/test-isolated-home.sh`, enforces `deny.toml` with `cargo deny check`, validates `CHANGELOG.md`, runs the release packaging unit tests, and checks Markdown links, anchors and dashes with `scripts/check_docs.py`.
The test wrapper uses a disposable `HOME`, removes inherited agent configuration directories, and supplies an offline curl stub while preserving `CARGO_HOME` and `RUSTUP_HOME` for the installed toolchain.
Its selftest checks toolchain resolution and cleanup on success, failure and termination.
The test job also runs `uv run --no-project python scripts/benchmark_hooks.py --ci` against the debug binary, using disposable state capped at 48 MiB and a 250 ms per-event p99 ceiling.
Tests marked `#[ignore]` (the synthetic soak trial and the real-reviewer checks) stay out of CI and run only when invoked explicitly with `--ignored`.
Local runs stay focused on the smallest tests covering a change; CI is where the full suite runs.

## Documentation and generated files

Update user guides when commands, configuration, or documented behavior change.
Keep defaults aligned with `config.rs` and command syntax aligned with installed `--help` output.
Separate implementation evidence from release or production acceptance claims.

Do not edit `CHANGELOG.md` directly.
Edit the reviewed records under `docs/changes/`, then regenerate and validate:

```sh
uv run --no-project python scripts/changelog.py
uv run --no-project python scripts/changelog.py --check
```

See [release packaging](scripts/release/README.md) for archive contents, licenses, checksums, and publication requirements.
Publishing a release is separate from building or pushing code.
