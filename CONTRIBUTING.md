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
| `scripts/` | Changelog generation and release packaging |

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
cargo test -p yoke-core config::tests -- --quiet
```

Inspect nearby test names before selecting filters for another module.
Add the fewest behavioral regressions necessary and keep external agent credentials out of ordinary automated tests.
Hook changes need real payload/output checks; UI changes need browser interaction and a subsequent snapshot of the resulting state.
Unit checks alone do not establish agent integration or rendered UI correctness.

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
