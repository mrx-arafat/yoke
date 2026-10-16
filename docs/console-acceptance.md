# Console acceptance run sheet

[Documentation index](README.md)

This is the kit for item 1 of the [development handoff](next-steps.md): live console acceptance on isolated state.
It describes the procedure and the evidence to collect; it records no result.
Acceptance is complete only when a human has filled the evidence table at the end for a named commit and attached it to [issue #22](https://github.com/mrx-arafat/yoke/issues/22).
Agents cannot perform this pass, because `yoke console`, `yoke trust`, `yoke plan approve` and `yoke plan cancel` refuse agent processes by design.

## Prerequisites

1. Build the executable from the commit under test with `cargo build --locked -q -p yoke-cli` and note `git rev-parse --short HEAD`.
2. Have `git` and `date` on `PATH`; `sqlite3` is needed only for the optional live database copy.
3. Use a terminal of your own that is interactive and not started by Claude Code or Codex.
4. Use a headed browser whose developer tools can resize the viewport and inspect attributes.
5. Reserve about fifteen minutes: five for the fixtures, ten for the pages.

## Step 1: generate fixtures

Run this from the repository root in any terminal:

```sh
sh scripts/acceptance/make-fixtures.sh
```

It creates a fresh review home with `mktemp -d`, sets `HOME` to it, and seeds only through supported commands and documented hook payloads.
The launch project `acme-api` sits under a path of about 220 characters so every page that prints a path exercises overflow handling.
A second project `beta-cli` exists for the Projects page and the project pickers.
Eight sessions are recorded, three for Claude Code and three for Codex in the launch project, one of each in the second project.
About a hundred observe-mode decisions include an allowed command, a self-protection `would deny` for a command that names `state.db`, an unbroken 200-character command, an edit, a Stop and some lifecycle events per session.
Each Stop imports a transcript, so Usage shows two priced Claude models, one unlisted Claude model and Codex totals with unknown cost.
Five memories cover a pinned note, Markdown, an unbroken token and plain text.
Seven plans exist as drafts, one per console state, and the draft carries requirements, tasks, a comment and a reply.
The Notifications page already shows `config_untrusted` and `observe_would_deny`.
The script prints the review home, the launch project, the hook and decision counts, and the single human step.
Expect one `release lookup failed` line in `<review-home>/.yoke/error.log` when offline, because the first `SessionStart` runs the daily release check.
Pass `--copy-live-db` to seed the review home from a read-only SQLite backup of the live database first, exactly as described under [Safe console review with a database copy](next-steps.md#safe-console-review-with-a-database-copy); the copy is migrated, the live file is not touched.

## Step 2: finish the fixtures as a human

Run the command that step 1 printed, in your own terminal:

```sh
sh scripts/acceptance/finish-fixtures.sh <review-home>
```

It asks you to type seven confirmation phrases, each shown on screen before the prompt:

| Prompt | Phrase | Why a human is needed |
| --- | --- | --- |
| 1 | `trust` | Trusting the fixture policy activates enforce mode, the deny and rewrite gates and `stop.max_blocks = 2` |
| 2 to 6 | `approve <slug>` | Approval freezes the header; the building, checking, waiting, stalled and done states all start from an approved plan |
| 7 | `cancel legacy-cleanup-cancelled` | Cancellation is human-only and records the reason |

After the last prompt the script needs no input.
It binds each approved plan to a recorded session, completes `lockout-done` with a hashed screenshot, records a failing check on `rate-limit-building`, asks a question on `auth-header-waiting`, sends three Stop events without progress to stall `pagination-stalled`, kills the owner of a running check on `retry-policy-checking`, records enforce-mode denials, a self-protection denial, a rewrite and an allow for both agents, appends a comment to the trusted `yoke.toml`, and shares a memory so `.yoke/knowledge.md` changes after trust.
It verifies every expected state and stops at the first mismatch; if that happens, delete the review home and start again from step 1.
`onboarding-draft` stays a draft on purpose so the console can perform an approval.

## Step 3: launch the console

Use the command that step 2 printed, which has the same shape as the handoff's database-copy flow:

```sh
cd <launch-project> && HOME=<review-home> ./target/debug/yoke console --no-open
```

Keep that terminal open, open the printed one-time link within five minutes, and stop the console with Ctrl-C when the pass is done.
Delete the review home afterwards with `rm -rf <review-home>`; nothing outside it was written.

## Run sheet

Every numbered step names the control to use, the visible state to expect, and the state to verify afterwards.
Record each step in the evidence table with a screenshot at 1440 px, then repeat the steps marked with an asterisk at the other widths.
Counts that depend on step 2 are derived from the fixture scripts and are marked "expected"; confirm them during the pass and record any difference.
The page list below was confirmed against the console routes: Decisions, Projects, Sessions, Plans, Plan detail, Memory, Trust & config, Usage, Notifications and Workflow settings.
There is no Jobs page; background jobs are listed only by `yoke jobs`, and the fixture keeps that list empty because digests stay disabled.

### 1. Decisions

1. Open Decisions from the rail or press `g` then `d`.
   Expect the ledger caption `50 records · newest first`, the launch project in the picker, and an `Older decisions ->` link.
   Verify the link opens an older page that offers `Latest decisions`.
2. \* Choose `Would deny` in the Outcome select and press `Apply filters`.
   Expect six records, all with the reason from the self-protection guard about `state.db`.
   Verify `Reset` restores the unfiltered ledger.
3. Type `Edit` into Tool name and apply.
   Expect six Claude records, a PreToolUse and a PostToolUse per session; with `apply_patch` expect six Codex records.
4. \* Expand a record with a click on its summary, or focus it with `j` and `k` and press Enter.
   Expect the reason, Session, Project, a `Gate results` block and a `Redacted payload` block.
   Verify the record whose command contains the 200-character token shows it inside the record without a horizontal page scroll.
5. Select `beta-cli` in the Project picker and apply.
   Expect the banner `Viewing project ... Back to launch project ... All projects` and two sessions' decisions.
6. After step 2 (expected): `Denied` shows eight records, the two force pushes, the two hard resets, the two `state.db` commands and the two Stop blocks; `Rewritten` shows two records whose payload command ends in `--quiet`; the Mode column shows `enforce` for them and `observe` for older rows.

### 2. Projects

1. Open Projects or press `g` then `o`.
   Expect `acme-api` first with the `Launch project` badge and its long path, then `beta-cli`.
   Expect Last activity, Sessions `6 recorded · 4 without an end event` for `acme-api` before step 2 and `3 without an end event` after it, Denials (24 h), Open plans and Protection now.
2. \* Read Protection now.
   Expect `Configured mode enforce · Project config: trusted` for `acme-api` after step 2 and `untrusted on disk, ignored` for `beta-cli`.
3. Follow the `Decisions` link on the `beta-cli` card.
   Verify Decisions opens scoped to `beta-cli` with the banner from step 1.5.

### 3. Sessions

1. Open Sessions or press `g` then `s`.
   Expect `Protection now`, then six panels with an agent badge each.
2. \* Read one Claude and one Codex panel.
   Expect Lifecycle with `ended` for the sessions ending in `101` and `201` (and `203` after step 2) and `no end event recorded` for the others, a Decisions count, Plan binding, Mode at last event and a Resume command such as `cd '<long path>' && claude --resume '<id>'` that stays inside the panel.
3. Follow a session's Decisions count.
   Verify Decisions shows `Session: <id>` with `Clear session filter` and only that session's rows.
4. After step 2 (expected): Plan binding links read `lockout-done (done)`, `pagination-stalled (stalled)`, `auth-header-waiting (waiting)`, `rate-limit-building (building)` and `retry-policy-checking (checking)`.

### 4. Plans list

1. Open Plans or press `g` then `p`.
   Expect seven panels, each with a slug link, a state badge and `Updated ... · revision N`.
   After step 2 (expected) the badges read draft, building, checking, waiting, stalled, done and cancelled.
2. \* Move with `j` and `k` and press Enter on `lockout-done`.
   Verify the plan detail opens and `All plans` returns to the list.

### 5. Plan detail

1. Open `onboarding-draft` and press `Review approval`.
   Expect the confirmation `Approve this exact contract?` with the highlighted header, a SHA-256 line and `Open advice and comments` listing comment #1.
   Press `Cancel` and verify nothing changed, then press `Review approval` and `Confirm approval`.
   Expect the toast `Plan approved. The reviewed header is frozen.` and the badge `approved`.
   Verify Transitions shows `draft → approved` by `human`, the `Frozen approved header` panel appears, and `yoke plan status onboarding-draft` in the terminal reports `approved`.
2. \* On the same plan read Requirements `0 of 2 satisfied`, Tasks `0 of 2 done` with `review-copy` blocked and owned by human, the feedback thread with its reply, the Markdown body with a code block, and the unbroken token under `Long unbroken text`.
3. Open `rate-limit-building`.
   Expect the badge `building`, no action button, and under Checks and review evidence a row `limiter-tests - fail` with `Blocking · exit 1` and the output tail `simulated failure`.
4. Open `retry-policy-checking`.
   Expect the badge `checking`, the explanation that recovery is refused while a live check runs, and `Recover interrupted check`.
   Press it and expect the toast `Plan resumed.` and the badge `building`.
   Verify the check list shows the interrupted run and `yoke plan status retry-policy-checking` reports `building`.
5. Open `auth-header-waiting`.
   Expect the badge `waiting`, a transition `building → waiting` whose note is the question, and `Resume work`.
   Press it and verify the badge `building` and a transition `waiting → building` by `human`.
6. Open `pagination-stalled`.
   Expect the badge `stalled`, a transition by `yoke` noting `stop limit reached without progress`, and `Resume work`.
   Press it and verify the badge `building`.
7. \* Open `lockout-done`.
   Expect the badge `done`, Requirements `1 of 1 satisfied` by check, Tasks `1 of 1 done` with the recorded evidence, and a check `lockout - pass` with `Open screenshot 1` and the badge `hash verified`.
   Press `Open screenshot 1` and expect the `Check screenshot` dialog with the image; press Escape and verify focus returns to the button.
8. Optional tamper check: append one byte to `shots/lockout.png` in the launch project, reload, and expect `Screenshot 1 changed after the check captured it`; restore the file from Git afterwards.
9. Open `legacy-cleanup-cancelled`.
   Expect the badge `cancelled`, no action button, and a transition `draft → cancelled` by `human` with the recorded reason.

### 6. Memory

1. Open Memory or press `g` then `m`, type `retry` into Search memories and press Search.
   Expect one memory with rendered bold text, a list and inline code.
2. \* Press `Pin` on it and expect the label `Pinned` and the button `Unpin`; press `Unpin` and verify it reverts.
3. Press `Forget` on the plain memory.
   Expect the inline question `Forget this memory? You can undo for 30 seconds. A shared file is not removed.` and `Confirm forget`.
   Confirm, expect a toast with `Undo forget`, press it within thirty seconds, and verify the memory is back with its previous pin state.
4. Read the memory with the unbroken token and verify it wraps inside the panel.
5. Press `Share` on a memory that is not shared.
   Expect the button to become `Shared` and disabled.
   Verify Trust & config now reports `.yoke/knowledge.md` as changed on disk.

### 7. Trust & config

1. Open Trust & config or press `g` then `t`.
   Expect Effective configuration as highlighted JSON with `"mode": "enforce"` after step 2, then Origins.
2. \* Read the three snapshot sections.
   Expect the global `config.toml` as `global / Not present`, `yoke.toml` as `project / Trusted; changed on disk (stale)` and `.yoke/knowledge.md` as `project / Trusted; changed on disk (stale)`.
   Before step 2 the project files read `Not trusted (ignored)`.
3. \* In the `yoke.toml` section find the `Unified` and `Split` buttons.
   Expect `Unified` with `aria-pressed="true"` and `Split` with `aria-pressed="false"`, and the appended comment line shown as an addition.
   Press `Split`, verify the diff switches to a side-by-side layout and the attributes swap; press `Unified` and verify they swap back.
4. Reach the same buttons with Tab and toggle them with Space or Enter.
5. \* Verify the long file path headings wrap without a horizontal page scroll.

### 8. Usage

1. Open Usage or press `g` then `u`.
   Expect the filters Project, From, To, Group by, Time zone, `Apply filters` and `Reset`, and the window `All recorded time`.
   Expect rows for today and for the first day of the month in UTC, for both agents.
2. \* Read the amounts.
   Expect a cost for `claude-sonnet-4-6` and `claude-opus-5-5`, unknown cost for `fixture-unlisted-model` and for Codex, and a total that shows the priced part plus the unknown samples.
3. Set From to today and apply; verify only today's rows remain and the window reads `From <date>`.
4. Choose `Month` under Group by and apply; verify a single row per agent and month.
5. Move focus into the table region with Tab and verify it scrolls horizontally with the arrow keys at 390 px instead of the page.

### 9. Notifications

1. Open Notifications or press `g` then `n`.
   Expect the rail badge with the unread count, the toolbar `N shown · N unread`, `Acknowledge all shown`, and the Show select with `All notifications` and `Unread only`.
   After step 2 (expected) the list includes `config_stale`, `plan_waiting`, `plan_stalled`, `check_failed` and `observe_would_deny`.
2. \* Open one item.
   Expect First seen and last seen, a link to the related page, the recovery steps, `Commands to run in your own terminal` and the documentation reference.
3. Press `Acknowledge` on one unread item.
   Expect the button to disappear and the rail count to decrease by one.
   Verify `yoke notifications` in the terminal lists it as acknowledged.
4. Choose `Unread only` and apply; verify the acknowledged item is gone.
5. Type `stale` into Search recovery help and verify the help section narrows to the stale-trust topic.

### 10. Workflow settings

1. Open Workflow settings or press `g` then `w`.
   Expect the sections Activation, Configuration trust, Decided by Yoke and Decided by your coding agent, and no buttons.
2. \* Read Activation and Configuration trust.
   Expect the badge `enforce` after step 2 and the same wording per file as Trust & config.
3. Read Decided by Yoke.
   Expect `stop.max_blocks` with value `2` and the full path of the launch project's `yoke.toml` as its source after step 2, and `default` sources before it.

### 11. Chrome, keyboard and live updates

1. Press Tab on a fresh load and expect `Skip to content` first; press Enter and verify focus lands in the main area.
2. \* Change the Color theme select to Dark and then Light; verify the page follows and the choice survives a reload.
3. Press Cmd+K or Ctrl+K, type `trust`, press Enter; verify Trust & config opens and the dialog closes.
4. Press `?`; expect the shortcuts dialog; press Escape and verify focus returns.
5. Run `yoke remember 'live update check'` in a second terminal with the same `HOME` and launch project.
   Expect the notice `New activity is available.` with `Refresh view` on Memory; press it and verify the new memory appears.

## Viewports

Run the full sheet at 1440 px, then the asterisked steps at 1024, 768 and 390 px, with the height set to 900 px or the phone preset 390 by 844.

| Width | Check at this width |
| --- | --- |
| 1440 | Rail and content side by side; filters on one row |
| 1024 | Rail still visible; Usage table stays inside its region |
| 768 | Filters wrap; plan detail tables and diff remain readable |
| 390 | Rail collapses or stacks; no horizontal page scroll on any page; long path, resume command and unbroken token stay inside their panels; dialogs and the lightbox fit the screen |

At every width record `document.documentElement.scrollWidth - window.innerWidth` from the console for Decisions, Sessions, Plan detail of `lockout-done`, Trust & config and Usage; the expected value is 0.

## Keyboard and ARIA checks

- The rail is `nav[aria-label="Workspace"]` and each link carries a `kbd` hint; `g` followed by `d`, `o`, `s`, `p`, `m`, `t`, `u`, `n` or `w` navigates.
- `#view` sets `aria-busy="true"` during navigation and clears it when the page is rendered; `main` carries `data-page` with the page name.
- The Unified and Split toggles expose `aria-pressed`, and only one of them is `true`.
- The unread badge is `data-unread-count` with a visually hidden ` unread` label.
- The command palette, shortcuts dialog and lightbox are `dialog` elements with `aria-labelledby` or `aria-label`, close on Escape and return focus.
- The connection indicator, toast and announcement regions use `role="status"`; the announcement region is `aria-live="polite"`.
- Every memory, plan and session record is reachable with `j`, `k` and Enter.
- Optional: run the axe browser extension on each page at 1440 and 390 px and record the violation count; a zero count does not establish WCAG conformance.

## Evidence table

Copy this table into the acceptance record and add one row per step and width.

| Commit | Date | Page | Viewport | Action | Screenshot | Result | Failure |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `abc1234` | 2026-10-15 | Plan detail | 1440 | Approve `onboarding-draft` from the console | `evidence/plan-approve-1440.png` | pass | |
| `abc1234` | 2026-10-15 | Trust & config | 390 | Toggle Split, check `aria-pressed` | `evidence/trust-split-390.png` | fail | Split view overflows by 24 px |

Record above the table: the commit, the build command, the executable path, the operating system and browser versions, the review home path, the number of confirmation prompts typed, and every deviation from this sheet.
Attach the completed table to issue #22; until then the console acceptance item in [next-steps.md](next-steps.md) stays open.

## Not covered by this kit

- Background jobs and the digest worker: digests stay disabled so no agent CLI is ever invoked, and `yoke jobs` lists nothing.
- Decisions with the `error` outcome and `hook_error` notifications.
- Independent review, `plan validate` and `review = true`, which need an authenticated reviewer CLI.
- Installed hooks, `yoke install` and `yoke health`, because the review home has no agent configuration directories.
- Real agent sessions; every payload is scripted.
