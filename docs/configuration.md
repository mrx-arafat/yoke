# Configuration reference

[Documentation index](README.md)

## Discovery, precedence, and trust

Yoke combines the trusted global `~/.yoke/config.toml` snapshot with the project `yoke.toml` found while walking upward from the current directory: the nearest one a human trusted, else the nearest one.
A nearer `yoke.toml` that is not trusted never replaces the policy of a trusted project above it; `yoke gates` and `yoke config` list the ignored file.
For a hook the walk starts at the directory the agent session started in, which the first event fixes for the session (`CLAUDE_PROJECT_DIR` for Claude Code), and the directory an event reports can only select a nested `yoke.toml` that a human trusted at or below it.
An event that reports a directory outside the project, a symlink that leaves it, or a nearer file nobody trusted keeps the project's trusted policy in force, and the ignored file is listed in a notification.
In a linked git worktree the policy trusted for the repository applies until the worktree's own `yoke.toml` is trusted, and editing or deleting the worktree's copy changes nothing.
[Where Yoke looks for your project](getting-started.md#where-yoke-looks-for-your-project) lists every layout.
Project values override global values only when explicitly supplied.
Project gates replace global gates with the same name; other gates remain in effect.
Unknown configuration fields are rejected.

Use `yoke trust --global` for global policy and `yoke trust` for project policy and associated shared knowledge.
Only approved snapshots execute.
Invalid or inaccessible edited files leave the last approved snapshot active and appear as stale with diagnostics in `yoke config`.
Inputs must be regular, non-symlink files no larger than 1 MiB each.

```sh
yoke config
yoke config --json
yoke gates
yoke gates --verbose
yoke gates --json
```

`yoke gates --verbose` shows the effective mode, global environment allowances, and every gate's full settings and source.
Check gates show the resolved default shell (`/bin/sh`), timeout, and expected exit code (`0`).
`source` and `expected_exit` are inspection metadata, not configuration fields.
`--json` includes those settings plus snapshot trust and staleness metadata for scripts; it prints no human help text.
Both inspect approved snapshots without executing gates, so an edited command appears only after a human trusts it.
Use `yoke log --json` for recorded failure reasons, gate exit codes and timing, plus output from checks that opt into capture below.

When re-trusting, Yoke shows changed lines with two lines of context and `...` for omitted text.
Each displayed line is JSON-quoted, including its line ending, so `\r\n`, `\n` and a missing final newline remain distinguishable.
New files are shown in full; very large comparisons fall back to full before/after output to bound diff computation.
The displayed hash and typed `trust` confirmation still approve the captured bytes, even if the disk file changes during review.

## Settings and defaults

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `mode` | string | `"observe"` | `observe` records potential effects; `enforce` applies policy |
| `meta.name` | string | `""` | Project label |
| `update.check` | boolean | `true` | Check GitHub for a stable release at most once daily at session start; set to `false` in trusted configuration to disable automatic release checks; explicit `yoke update --check` still works |
| `usage.warn_percent` | integer, 1 to 100 | `85` | Warn when a fresh account quota window reaches this percent used. Recent burn can warn sooner if it projects exhaustion before reset. Warnings are advisory in both modes, once per reset and severity |
| `log.retention_days` | integer (days) | `90` | Retention in days for decisions, facts, check runs of done or cancelled plans, and succeeded or dead background jobs; cleanup is queued at most hourly at session start and runs in a background worker in short batches, so a session never waits for it and a large backlog drains in the background. Memories, trusted snapshots, and the check runs and evidence of unfinished plans are never removed by retention. Notifications use the same window when they are listed, except unresolved conditions |
| `log.payload_cap` | integer (bytes) | `4096` | Bytes of each redacted hook payload stored, cut at a UTF-8 character boundary; `yoke log --json` flags a cut payload with `payload_truncated`; a tool's `tool_response` is stored as its size and status flags (plus a 200-character excerpt of a shell command's error output), not as the output |
| `stop.max_blocks` | integer | `5` | Stop attempts without progress that Yoke blocks before an active plan becomes `stalled` |
| `plans.dir` | string | `".yoke/plans"` | Plan directory relative to the project root; empty, absolute and parent-traversal paths are rejected |
| `memory.digest` | boolean | `false` | Enable optional agent-generated session digests |
| `memory.digest_agent` | string | `"claude"` | Digest CLI: `claude` or `codex` |
| `memory.inject_tokens` | integer (tokens) | `1500` | Memory injection budget, estimated at 4 characters per token; inspect the selection with `yoke recall --explain` |
| `review.reviewer` | string | `"auto"` | Reviewer: `auto`, `claude`, or `codex`; review requires a different agent from the builder |
| `review.models.claude`, `review.models.codex` | string | Unset | Model Yoke passes to that reviewer CLI (`claude --model`, `codex exec --model`); unset uses the CLI's default |
| `memory.digest_models.claude`, `memory.digest_models.codex` | string | Unset | Model Yoke passes to the digest CLI; unset uses the CLI's default |

`stop.max_blocks` limits consecutive plan-guard blocks without recorded activity, not total retries over a plan's lifetime.
The count resets when the working-tree fingerprint changes or a new plan transition or check result is recorded, including failed checks.
With the default of 5, five unchanged Stops are blocked; the next Stop stalls the plan and lets the agent stop. Observe mode reports the hypothetical block without advancing the counter; a separate gate's denial does not advance this plan counter either.
This progress-aware design is retained because a hard total ceiling could interrupt legitimate repairs that need several check cycles. Recorded activity does not prove useful progress: repeated failing checks can reset the count indefinitely. Inspect `yoke plan status <slug>` for those loops, and use a supervising runner's time or token budget when total work must be bounded.

Model values are checked when a file is parsed, so `yoke trust` refuses an invalid one: 1 to 128 letters, digits or `. _ : / @ [ ] -`, not starting with `-`, and only `claude` or `codex` keys.
Yoke cannot check offline whether a model is available to your account.
It never adds a fallback model, so an unavailable model fails the review or digest with the CLI's error and the review is recorded as failed.
Models are per agent so that `review.reviewer = "auto"` still gets the right model for whichever agent reviews.
Planning and building models stay with Claude Code and Codex; see [decision 0002](decisions/0002-model-routing.md).

```toml
[review.models]
claude = "opus"
codex = "gpt-6.1-sol"

[memory.digest_models]
claude = "haiku"
```

TOML keys following a table header belong to that table.
Place `mode` before `[meta]`, `[memory]`, or other table headers.

## Gate fields

Each `[[gate]]` defines one gate.

| Field | Default or requirement | Behavior |
| --- | --- | --- |
| `name` | Required, nonempty, unique within a layer | Gate identity and replacement key |
| `on` | Required | Event name from the table below |
| `action` | `"check"` | `check`, `deny`, or `rewrite` |
| `run` | Required for `check` | Shell command; a nonzero exit fails the check |
| `capture_output` | `false` | Check only: store redacted stdout/stderr tails with each execution's decision; see below |
| `shell` | `"/bin/sh"` | Check shell, invoked with `-c`; valid only for checks |
| `timeout` | `30` | Timeout in seconds, at least 1; `yoke trust` refuses check gates whose timeouts on one event add up to more than 530 s for `pre_tool_use` or `stop`, or 20 s for `user_prompt_submit`, so the hook can answer before the agent's own hook timeout |
| `message` | Optional | Failure reason; otherwise check stderr is used |
| `match_tool` | Optional | Regex matching the entire tool name |
| `match_contains` | Optional | Literal substring in the event's searchable text |
| `match_pattern` | Optional | Regex searched within that text |
| `rewrite_field` | `"command"` | Tool-input field to rewrite |
| `rewrite_pattern` | Required for rewrite | Replacement regex |
| `rewrite_to` | Required for rewrite | Replacement text |
| `require_command` | Optional, rewrite only | Executable basename that must exist on PATH |

Searchable text consists of the tool name, prompt when present, and serialized tool input.
It is not simply the raw shell command.
`deny` and `rewrite` must not specify `run` or `shell`, and a `shell` that is set must not be empty.
Rewrite-specific fields are rejected on other actions.
A `rewrite` gate needs `on = "pre_tool_use"`, `rewrite_pattern`, `rewrite_to`, and a nonempty `rewrite_field`.
`require_command` must be a bare executable name of letters, digits, `_`, `.` or `-`.
`match_tool`, `match_pattern` and `rewrite_pattern` must be valid regular expressions.
All of these are checked when the file is parsed, so `yoke trust` refuses an invalid file and keeps the previous snapshot.

### Example: a check before stopping

For this Rust repository, a formatting gate can be written as:

```toml
mode = "observe"

[[gate]]
name = "format-before-stop"
on = "stop"
run = "cargo fmt --all -- --check"
timeout = 30
message = "Run cargo fmt --all, then retry."
```

Choose commands suitable for the governed repository.
Observe mode executes `run` even though it does not block.
If a check exits with `126` or `127`, Yoke keeps the shell's stderr even when `message` is set and adds recovery hints.
Failure reasons are redacted before their final 4 KiB limit is applied.
If the raw stderr buffer was truncated, Yoke omits its leading partial line because it may have lost a credential label.
These statuses usually indicate an unexecutable command or a missing command: inspect `yoke gates --verbose`, check permissions or `PATH`, and run `yoke trust` after editing the policy.

### Retain check diagnostics

Enable capture only for checks whose output you want retained, then run `yoke trust` in your own terminal:

```toml
[[gate]]
name = "format-before-stop"
on = "stop"
run = "cargo fmt --all -- --check"
capture_output = true
message = "Fix formatting, then retry."
```

`yoke log --json --limit 20` includes an `output` object on each captured gate result, with `stdout`, `stderr`, `stdout_truncated` and `stderr_truncated`.
The console's existing Gate results detail shows the same data.
Capture applies to passing, failing and timed-out checks in both modes; it does not change the agent's decision or failure message.
Each stream retains at most 4 KiB after recognized secrets are redacted and invalid UTF-8 is replaced.
If a raw stream exceeded its buffer, the leading partial line is omitted before redaction because it may have lost a credential label; a long single line can leave an empty tail with its truncation flag set.
The streams are separate tails, not an interleaved transcript.

Captured output follows decision-log retention (`log.retention_days`); disabling capture affects future runs, not existing records.
Capture is off by default and is not permitted on deny or rewrite gates.
Redaction recognizes supported credential patterns, not arbitrary confidential data; choose check output accordingly.

## Events and decisions

Both snake_case names and their PascalCase equivalents are accepted.

| Event | Agent spelling | Can block in enforce mode? |
| --- | --- | --- |
| `pre_tool_use` | `PreToolUse` | Yes |
| `user_prompt_submit` | `UserPromptSubmit` | Yes |
| `stop` | `Stop` | Yes |
| `post_tool_use` | `PostToolUse` | No |
| `post_tool_use_failure` | `PostToolUseFailure` | No; Claude only |
| `session_start` | `SessionStart` | No |
| `session_end` | `SessionEnd` | No |
| `pre_compact` | `PreCompact` | No |
| `notification` | `Notification` | No; Claude only |

Yoke records a `command_failure` memory fact only from Claude's `PostToolUseFailure` event for `Bash` and `PowerShell`, using its documented `error` text and skipping interrupts.
Codex documents no exit status or failure flag in its `PostToolUse` payload, so Codex command failures are not recorded as facts.
Installs made before this event existed gain it when you rerun `yoke install --agent claude`.

Successful ordinary hook evaluation is silent.
Observe mode logs potential denials and does not rewrite input.
In enforce mode, checks and denials must pass before the first matching rewrite can apply.
Rewrites are supported only for `pre_tool_use`.
Codex rewrites use an allow decision that bypasses its normal permission prompt for that invocation; keep matchers narrow.
For Codex, a matching rewrite gate must set both a nonempty `match_tool` and a nonempty `match_contains`.
Otherwise enforce mode refuses calls for that gate's event with a message to narrow those matchers and trust the updated policy in your own terminal.
Observe mode records the fault and continues without rewriting.
Claude rewrites preserve normal permission evaluation.

## Gate environment

| Variable | Meaning |
| --- | --- |
| `YOKE_EVENT` | Normalized hook event |
| `YOKE_GATE` | Current gate name |
| `YOKE_TOOL_NAME` | Event tool name |
| `YOKE_TOOL_INPUT` | JSON tool input up to 32 KiB. Larger values end with `[YOKE TOOL INPUT TRUNCATED]` and are no longer valid JSON. Matchers, `deny` and `rewrite` gates still see the whole original input |
| `YOKE_TOOL_INPUT_TRUNCATED` | `1` when tool input was shortened, otherwise `0`; checks that need the complete input should refuse when this is `1` |
| `YOKE_SESSION` | Agent session identifier |
| `YOKE_CWD` | Directory the agent reported for the event; the agent chooses it, so treat it as data |
| `YOKE_PROJECT` | Resolved project root |

A check gate runs in the directory of the trusted `yoke.toml` it came from, the project root unless a nested trusted policy applies, and not in `YOKE_CWD`.
So `run = "./check.sh"` or `run = "make lint"` means the same file whichever directory the agent works in.
Relative `PATH` entries and `require_command` wrappers resolve against that directory as well.

Check gates receive the variables above, `HOME` from the hook's chosen home, and these inherited variables when set:

| Purpose | Default inherited names |
| --- | --- |
| Shell, terminal and locale | `PATH`, `TMPDIR`, `TMP`, `TEMP`, `LANG`, `LC_ALL`, `LC_CTYPE`, `LC_MESSAGES`, `TZ`, `USER`, `LOGNAME`, `SHELL`, `TERM`, `COLORTERM`, `NO_COLOR`, `CI` |
| User directories | `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`, `XDG_RUNTIME_DIR` |
| Rust and Go | `CARGO_HOME`, `RUSTUP_HOME`, `RUSTUP_TOOLCHAIN`, `GOPATH`, `GOROOT`, `GOCACHE`, `GOMODCACHE`, `GOFLAGS` |
| Node and package managers | `NVM_DIR`, `NODE_OPTIONS`, `VOLTA_HOME`, `PNPM_HOME`, `BUN_INSTALL` |
| Python | `PYENV_ROOT`, `VIRTUAL_ENV`, `CONDA_PREFIX`, `UV_CACHE_DIR` |
| JVM, Android and .NET | `JAVA_HOME`, `GRADLE_USER_HOME`, `MAVEN_OPTS`, `ANDROID_HOME`, `DOTNET_ROOT` |
| Certificates and native packages | `SSL_CERT_FILE`, `SSL_CERT_DIR`, `NODE_EXTRA_CA_CERTS`, `REQUESTS_CA_BUNDLE`, `CURL_CA_BUNDLE`, `PKG_CONFIG_PATH` |
| Proxy exclusions | `NO_PROXY`, `no_proxy` |

Other inherited variables, including credentials, proxy URLs that may contain passwords and shell startup overrides, are removed by default.
Pass additional names explicitly in the trusted configuration, globally or for one gate:

```toml
[gates]
pass_env = ["DATABASE_URL"]

[[gate]]
name = "integration"
on = "stop"
run = "./scripts/integration.sh"
pass_env = ["SERVICE_TOKEN", "HTTPS_PROXY"]
```

Per-gate names add to the global list.
Each list accepts at most 64 exact variable names, up to 128 ASCII letters, digits and underscores, starting with a letter or underscore; globs and values are not accepted.
The variables must already exist in the hook's environment, and the hook's `HOME` and documented `YOKE_*` values always win.
Review the names and run `yoke trust` in your own terminal after changing them.
Only pass credentials to check code you trust to receive them.
On failure, the gate reason lists withheld names (never their values) with a `pass_env` hint; a long list is shortened and the message stays within 4 KiB.

Child processes have core dumps disabled.
File-size, CPU and descriptor limits stay inherited so normal build and test tools retain their existing resources.
The wall-clock timeout kills the gate's process group, including its children and grandchildren.
These controls do not restrict filesystem access or network access and are not a sandbox.

Treat payload-derived environment values as untrusted input in your own scripts.
For storage failures and the explicit `yoke pause` recovery mechanism, see [troubleshooting](troubleshooting.md).
