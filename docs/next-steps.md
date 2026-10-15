# Development handoff

Recorded 2026-10-14 after hardening commits `75bfe35` and `f254d94` reached `main`.
This is the starting point for the next development session, not a release acceptance claim.
Track remaining acceptance in [issue #22](https://github.com/mrx-arafat/yoke/issues/22) and the [audit tracker #23](https://github.com/mrx-arafat/yoke/issues/23).

## Completed checkpoint

The hardening covers plan recovery and stable check ownership, installation diagnostics, extension inventory redaction and symlink safety, project-scoped notifications, transcript usage accounting, console trust and mobile layout, trial time-window validation, and package size validation.
The debug build, focused regressions and Rust formatting passed during implementation.
Those checks do not establish live console acceptance, production installation, sustained enforcement, or native execution on every supported platform.
The commits were pushed without publishing a binary release or replacing installed hooks.

## Next work, in order

1. **Finish live console acceptance on isolated state.**
   A human must start the console in their own interactive terminal; do not bypass its agent-process guard.
   Navigate Trust & config and the affected plan views in a headed browser, interact with their primary controls on disposable fixtures, and confirm the resulting state at desktop and mobile widths.
   Record the tested commit, actions, screenshots and failures in the acceptance record.
   Use [console acceptance](console-acceptance.md) for the fixture scripts and the evidence template.
2. **Coordinate the installed-binary upgrade.**
   At the last read-only inspection on 2026-10-14, active Codex hooks used an older `target/release/yoke`, the real database was schema 6, and current source expected schema 12.
   Claude hooks were absent and Codex was recording in observe mode without trusted policy.
   Recheck these facts before acting, then follow [upgrade notes](upgrade-notes.md): finish active checks, preserve a consistent database backup, replace every hook executable together, reinstall with the actual agent configuration directories, approve Codex hooks, and inspect `yoke health`.
   Do not open the real database with the new writable binary while old hooks still use it.
   Rehearsed on 2026-10-15 at commit `05310d7` with a debug build, an isolated `HOME`, and an SQLite backup copy of the real database; the real `state.db` and `state.db-wal` had identical SHA-256 digests before and after, and the real schema stayed 6.
   On the copy, migrations 007 to 012 moved schema 6 to 12 in about 0.1 s inside the first writable command or hook event, `PRAGMA integrity_check` and `PRAGMA foreign_key_check` passed, every pre-existing table kept its row count, and the five new tables started empty.
   `yoke health`, `yoke trial report`, `yoke statusline` and `yoke extensions` left the copy at schema 6, and `yoke install` did not migrate either.
   Wrong order on this machine: the old binary against a schema 12 database denies PreToolUse and blocks Stop and UserPromptSubmit even though the recorded mode is observe, because it fails closed when it cannot read state, and it appends one `error.log` line per event.
   A reinstall from a binary at a different path than the installation receipt is refused with `executable changed; uninstall before reinstalling`; run `yoke uninstall` with the new binary first, or replace the binary at the receipt's path, which made a zero-change reinstall on this machine's hook file.
   Rollback worked by restoring the pre-migration copy, which the old binary opened at schema 6; restore `state.db`, `state.db-wal` and `state.db-shm` together, because a restored main file next to the newer sidecars replayed the migration and read as schema 12 with integrity ok.
   Not rehearsed: the in-place release build, the Codex `/hooks` approval, a hook firing during the binary replacement, and the console; these remain for the live upgrade, which is still an acceptance step, not a result of this rehearsal.
3. **Collect real-work trial evidence.**
   Review and trust policy in the human terminal, complete the observe-day and enforce-week trials for both agents, and record false denials, missed denials, recovery and interruption incidents using [trials.md](trials.md).
   Investigate each incident before expanding features.
4. **Complete release acceptance.**
   Run the native release workflow for all four targets, verify assets and notices, complete clean-account install checks and compliance review, and test hosted updates when a suitable published predecessor and successor exist.
   Follow [release acceptance](release-acceptance.md); a push to `main` alone does not complete these gates.
5. **Close the acceptance issues only with evidence.**
   Attach results to #22 and reconcile #23 before choosing the next feature batch.
   Keep unresolved limitations explicit rather than describing the system as defect-free.

## Safe console review with a database copy

Build the debug executable from the intended commit with `cargo build --locked -q -p yoke-cli`.
Then run the following from the repository root in your own terminal, with SQLite installed.
It copies the live database using SQLite's backup operation and lets the new console migrate only that temporary copy.
It does not replace the installed hook executable.

```sh
set -eu
umask 077
review_home=$(mktemp -d)
mkdir "$review_home/.yoke"
if [ -f "$HOME/.yoke/state.db" ]; then
  sqlite3 -readonly "$HOME/.yoke/state.db" ".backup '$review_home/.yoke/state.db'"
fi
printf 'Review state copy: %s\n' "$review_home"
HOME="$review_home" ./target/debug/yoke console --no-open
```

Keep that terminal open and use the printed one-time launch URL within five minutes.
The review home omits live agent settings and trusted configuration files, so installation and on-disk configuration diagnostics differ from the live account.
Use disposable project fixtures for any approval or recovery action: copying the database does not isolate project files or shell checks referenced by it.
Stop the console with Ctrl-C after review.

## Known limits to carry forward

- Transcript import retains a 32 MiB total bound; a Codex cumulative counter restart starts a new accounting segment, and a restart that lands exactly on the previous total is indistinguishable from no progress.
- A session silent for 30 minutes with pending facts is digested at the next session start in its project; a live session that silent is digested early, without its transcript.
- Ambiguous legacy notification ownership cannot always be recovered; hidden history must not be silently reassigned.
- Stop-cap stalls still need real interruption-trial evidence.
- Binary artifact review has intentional limits; neither successful checks nor this handoff guarantee zero defects.
- Read-only commands such as `yoke health`, `yoke trial report` and `yoke statusline` fail with SQLite error 14 on a database copy that lacks its `-wal` and `-shm` sidecars until a writer creates them; the live database has them.
- A restored `state.db` beside newer `-wal` and `-shm` files silently replays their frames; restore or remove all three together.

Next session starts with isolated live console acceptance, then the coordinated hook upgrade.
