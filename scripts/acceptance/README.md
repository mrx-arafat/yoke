# Console acceptance fixtures

These scripts build the disposable state that the [console acceptance run sheet](../../docs/console-acceptance.md) walks through.
They never read or write the real `~/.yoke`, never run `yoke console`, and never bypass a human-only guard.

| Script | Who runs it | What it does |
| --- | --- | --- |
| `make-fixtures.sh` | Anyone, including an agent | Creates a temporary review home with two throwaway Git projects, eight recorded sessions, about a hundred observe-mode decisions, usage samples, memories, two notifications and seven draft plans, then prints the one human step |
| `finish-fixtures.sh` | A human in their own terminal | Asks for seven typed confirmations (`trust`, five `approve <slug>`, one `cancel <slug>`), then binds plans to sessions and produces the building, checking, waiting, stalled, done and cancelled states, enforce-mode decisions for both agents, and an edited trusted file |
| `lib.sh` | Sourced by both | HOME guard, `yoke` wrapper, hook payload builder and plan state reader |

## Usage

```sh
cargo build --locked -q -p yoke-cli
sh scripts/acceptance/make-fixtures.sh
sh scripts/acceptance/finish-fixtures.sh <review-home-printed-above>
cd <launch-project-printed-above> && HOME=<review-home> ./target/debug/yoke console --no-open
```

`make-fixtures.sh --copy-live-db` first copies the live database into the review home with SQLite's read-only backup operation, exactly as [docs/next-steps.md](../../docs/next-steps.md) describes, so real history renders next to the fixtures.
`--bin <path>` or `YOKE_BIN` selects another debug executable; a path under `target/release/` is refused because live hooks may execute it.

## Guarantees and limits

- Every `yoke` invocation asserts that `HOME` is a non-empty, existing directory other than the invoking user's home.
- Only supported commands and documented hook payloads seed state; nothing writes to the SQLite file directly.
- `make-fixtures.sh` starts no background process and leaves only the review home behind; delete it with `rm -rf <review-home>`.
- The first `SessionStart` performs Yoke's daily release check, so an offline machine logs one `release check failed` line in `<review-home>/.yoke/error.log`; nothing else depends on the network.
- `finish-fixtures.sh` refuses to continue when a confirmation is declined or a state is not reached, and the fix is to delete the review home and start again.
- Memory digests stay disabled, so `yoke jobs` lists nothing and no agent CLI is ever invoked.
