<p align="center">
  <img src="crates/yoke-cli/src/console/yoke.svg" width="72" height="72" alt="Yoke logo">
</p>

<h1 align="center">Yoke</h1>

<p align="center"><strong>Local, trusted quality gates for Claude Code and Codex.</strong></p>

<p align="center">
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="License: MIT OR Apache-2.0"></a>
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/Rust-edition%202021-orange" alt="Rust edition 2021"></a>
</p>

A coding agent can say a task is done when no test proved it, and a rule written in a prompt is a request, not a guarantee.
Yoke sits in the hook path of Claude Code and Codex.
On every hook event it runs the checks a human approved, records what it decided, and in enforce mode blocks the action with a reason the agent sees.
A plan is marked done only after Yoke itself has run the plan's checks on the final working tree.
It is one binary and one SQLite file on your machine, with no daemon and no telemetry.

New to Yoke? Read the [manual](MANUAL.md) first: a five-minute explanation of what it is and one full worked example.

> [!IMPORTANT]
> Yoke 0.1.0 is pre-release.
> No binary release is published yet and the real-work release trials are still open, so build from source and read [Status](#status) before relying on it.

![The Yoke console Decisions page for a demo project named acme-api, listing allow and deny decisions from Claude Code in enforce mode](docs/assets/console-decisions-light.png)

<sub>The local console showing hook decisions from a synthetic demo project.</sub>

## Why Yoke

- **Code enforces, prompts inform.** Gates are shell checks, denials or command rewrites on agent events; in enforce mode a failing gate blocks `PreToolUse`, `UserPromptSubmit` and `Stop`.
- **Only approved policy runs.** Yoke applies the exact configuration bytes a human approved with `yoke trust`, edits change nothing until they are trusted again, and `yoke trust` refuses to run from an agent's tool process.
- **Watch before you block.** New policy starts in observe mode, which runs the gates and records what would have been denied without blocking anything.
- **Proof, not promises.** Plans freeze their checks, requirements and tasks at human approval, and Yoke runs the checks itself, keeps their output and SHA-256 hashes of declared artifacts, and requires an unchanged Git working tree before it marks a plan done.
- **No early exits.** While a plan is building or checking, enforce mode blocks the agent's Stop, and after `stop.max_blocks` attempts without progress (default 5) the plan stalls for a human instead of looping.
- **A second opinion.** Independent review asks the other agent's CLI to review the change since the plan started, and blocking findings fail the plan.
- **Everything visible, locally.** A loopback-only web console, terminal notifications, a Claude Code statusline, usage estimates, portable memory and a read-only trial report all read the same local state.

## Quickstart

Yoke targets macOS and Linux, and its release workflow builds both for x86_64 and aarch64.
You need Claude Code or Codex.
While no release is published, installing builds from source, so you also need a stable Rust toolchain, which the install script offers to install.

**Install or upgrade in one line:**

```sh
curl -fsSL https://raw.githubusercontent.com/mrx-arafat/yoke/main/install.sh | bash
```

It works on macOS and on common Linux distributions, including a fresh server: run as root, it installs `yoke` into `/usr/local/bin`, and otherwise into `~/.local/bin`, offering to add that to your `PATH`.
It downloads the release executable for your platform and refuses it unless its SHA-256 checksum matches, and when it has to build from source (on Alpine and other musl systems, or when the release has no executable for your platform) it builds the newest release tag, or while no release exists the commit `main` points to when it runs (it prints the commit and builds exactly that one), offering to install a C compiler and Rust when they are missing.
To check what you downloaded rather than trust the page it came from, see [Verifying releases](docs/verifying-releases.md).
It uses `sudo` only to install a C compiler, after asking; every question defaults to yes, and `YOKE_YES=1` answers them without a terminal.
It then connects the Claude Code and Codex hooks it finds, creates or upgrades the state database once those hooks run the new binary, adds Yoke's line (mode, context, 5-hour and weekly quota) to Claude Code's status bar when you have no statusline, offers a starter global policy for you to review and trust, and runs `yoke health`.
A statusline you already have is never replaced: the installer keeps it, and wraps it only if you answer yes at a terminal (`YOKE_NO_STATUSLINE=1` skips the step).
Rerunning it upgrades in place, and if your hooks run a Yoke binary from another location, it moves them to the new one.
Read [`install.sh`](install.sh) first if you prefer; [Getting started](docs/getting-started.md#1-build-and-install-the-executable) lists its settings, such as `YOKE_INSTALL_DIR` and `YOKE_AGENTS`.
Then approve the hooks in Codex with `/hooks` and create a policy as in step 2.
The steps below do the same thing one at a time.

**1. Build and install from source**, because no binary release exists yet:

```sh
git clone https://github.com/mrx-arafat/yoke.git
cd yoke
cargo install --path crates/yoke-cli --locked
yoke --version
```

Cargo installs `yoke` into its binary directory, normally `~/.cargo/bin`, which must be on your `PATH`.

**2. Create and approve a policy** in the project you want to govern:

```sh
cd /path/to/your-project
yoke init     # writes yoke.toml in observe mode with a sample no-force-push gate
yoke trust    # shows the exact bytes and asks you to type a confirmation phrase
yoke gates    # lists the trusted gates for this directory
```

Run `yoke trust` in your own interactive terminal; it rejects piped input and agent tool processes.

**3. Connect your agents** and confirm the hooks can run:

```sh
yoke install --agent claude,codex --dry-run   # prints every file change first
yoke install --agent claude,codex
yoke health                                   # exits 1 when installed hooks cannot run
```

Pass `--agent claude` or `--agent codex` to install for one agent.
Codex runs new hooks only after you approve them with `/hooks` inside Codex.
If you start an agent with `CLAUDE_CONFIG_DIR` or `CODEX_HOME`, keep the same variable set when you run `yoke install`.
If health reports no recorded event, start or restart the agent in the project, perform a tool action, then run `yoke log --limit 10` and `yoke health` again.

**4. Watch before you block:**

```sh
printf '%s\n' '{"tool_name":"Bash","tool_input":{"command":"git push --force"}}' |
  yoke run-hook --agent claude pre_tool_use
yoke log --outcome would_deny --limit 5
```

This simulates a hook event; it does not run Git.
In observe mode Yoke prints nothing and records a `would_deny` decision.
After a period of normal work, review `yoke log --outcome would_deny`, set top-level `mode = "enforce"` in `yoke.toml`, and run `yoke trust` again.
The same simulated event then returns a denial, as shown in [Gates](#gates-observe-then-enforce).
For global policy, health names the file supplying observe mode and the matching `yoke trust --global` command; its stale-policy guidance also names the correct trust command and says to restore a missing trusted file first.

`yoke --help` shows the setup sequence; `init`, `trust`, `install` and `health` help include examples and setup constraints.
[Getting started](docs/getting-started.md) walks through every step, and [uninstalling](docs/uninstalling.md) reverses the installer.

## How it works

```mermaid
flowchart LR
  agent["Claude Code or Codex"] -- "hook event JSON on stdin" --> hook["yoke run-hook<br>runs the trusted gates"]
  hook -- "enforce: deny or rewrite" --> agent
  files["yoke.toml and<br>~/.yoke/config.toml"] -- "yoke trust (human)" --> db[("~/.yoke/state.db")]
  db -- "trusted snapshot" --> hook
  hook -- "decision, redacted" --> db
  db --> views["yoke log, notifications,<br>statusline, console"]
```

1. `yoke install` registers `yoke run-hook --agent <agent> <event>` in each agent's hook settings, and the agent sends each event's JSON on stdin.
2. Yoke evaluates only the trusted snapshots of the global and project configuration; files on disk are compared for staleness and never executed directly.
3. In observe mode Yoke prints nothing and records `would_deny`; in enforce mode a failing gate on a blocking event answers in the agent's documented hook format, and policy errors fail closed.
4. Every decision is stored with secrets redacted, and the CLI, statusline and console read the same database.

Plans move through these states, and the database, not the Markdown file, owns the state:

```mermaid
flowchart LR
  draft -- "yoke plan approve<br>(human)" --> approved -- "yoke plan start" --> building -- "yoke plan check" --> checking -- "checks, review and<br>acceptance pass" --> done
  checking -- "a blocking check fails" --> building
  building -- "yoke plan ask" --> waiting -- "yoke plan resume (human)" --> building
  building -- "stop.max_blocks Stops<br>without progress" --> stalled -- "yoke plan resume (human)" --> building
  done -- "tree changed, next Stop<br>in enforce mode" --> building
```

`yoke plan new` creates the draft, a human can cancel any unfinished plan with `yoke plan cancel`, and `yoke plan resume` also recovers a plan whose check process was killed.

## Feature tour

### Gates: observe, then enforce

```toml
mode = "enforce"

[[gate]]
name = "no-force-push"
on = "pre_tool_use"
match_tool = "Bash"
match_contains = "push --force"
action = "deny"
message = "Force push is blocked by Yoke. Push without --force."

[[gate]]
name = "format-before-stop"
on = "stop"
run = "cargo fmt --all -- --check"
timeout = 30
message = "Run cargo fmt --all, then retry."
```

A `check` gate runs a shell command and fails on a nonzero exit, `deny` blocks whatever matches, and `rewrite` edits a `PreToolUse` input such as a shell command.
Check commands receive the event, gate, tool, input, session, working directory and project as `YOKE_*` environment variables, and run in the directory of the trusted `yoke.toml`, so a relative command means the same file wherever the agent works.
With that policy trusted, a force push is denied in the agent's own response format:

```console
$ echo '{"tool_name":"Bash","tool_input":{"command":"git push --force origin main"}}' \
    | yoke run-hook --agent claude pre_tool_use
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"Force push is blocked by Yoke. Push without --force."}}

$ yoke log --outcome deny --limit 1
project: "/path/to/project"
decisions[1]{time,agent,session,event,tool,outcome,gate,detail}:
  "2026-10-14 08:55:56","claude","5f2a9c1e-0b7d-4c3a-9e55-1d6a7f0c2b48","pre_tool_use","Bash","deny","no-force-push","git push --force origin main - Force push is blocked by Yoke. Push without --force."
```

Only `pre_tool_use`, `user_prompt_submit` and `stop` can block; failures on other events are recorded and the agent continues.
The sample matcher is an example, not complete protection against every way to spell a force push.
`yoke preset add git-safety` appends readable deny gates for force pushes, hard resets, forced cleans and `git checkout --` discards, and they apply only after you run `yoke trust`.
The [configuration reference](docs/configuration.md) lists every field, event and default.

### Plans that prove they are done

A plan is a Markdown file in Git whose TOML header declares checks, requirements and tasks:

```toml
+++
title = "Rate-limit the login endpoint"
review = false

[[check]]
name = "rate-limit"
run = "sh tests/rate_limit.sh"

[[check]]
name = "lockout-screenshot"
run = "test -s shots/lockout.png"
artifacts = ["shots/lockout.png"]

[[requirement]]
id = "limit"
text = "Reject the sixth login attempt within one minute"
checks = ["rate-limit"]

[[requirement]]
id = "lockout-copy"
text = "The lockout page says when the user can try again"
checks = ["lockout-screenshot"]

[[task]]
id = "middleware"
title = "Add the rate-limit middleware"
requirements = ["limit"]

[[task]]
id = "lockout-page"
title = "Show the lockout page"
requirements = ["lockout-copy"]
depends_on = ["middleware"]
+++
```

```sh
yoke plan new login-rate-limit --title "Rate-limit the login endpoint"
yoke plan validate login-rate-limit --agent claude   # optional: the other agent reviews the plan first
yoke plan approve login-rate-limit                    # human, in your own terminal
yoke plan start login-rate-limit --session <session-id>
yoke plan task done login-rate-limit middleware --evidence "Counts failures per account"
yoke plan task done login-rate-limit lockout-page --evidence "Shows the retry time"
yoke plan check login-rate-limit --agent claude      # --agent names the agent that built it
yoke plan status login-rate-limit
```

Approval freezes the header, so changing a check, requirement or task needs a fresh approval.
`yoke plan check` runs the frozen commands, records their output and artifact hashes, and marks the plan done only when every blocking check, required review, requirement and task passes on a working tree that did not change during the run.
Set `review = true` to have the other agent's CLI review the change, and see the [workflow guide](docs/workflows.md#plans-backed-by-evidence) for human-only requirements, feedback threads and recovery.
Take session ids from the session column of `yoke log` (it lists the current project) or the console Sessions page.

An excerpt of `yoke plan status` after that run, with the temporary project path shortened to `...`:

```console
$ yoke plan status login-rate-limit
plan:
  slug: "login-rate-limit"
  state: done
  tasks: 2/2 done
  requirements: 2/2 satisfied
checks[3]{id,name,result,blocking}: newest 3 of 3
  3,"acceptance",pass,true
  2,"lockout-screenshot",pass,true
  1,"rate-limit",pass,true
evidence[1]{check,path,status}:
  2,".../acme-api/shots/lockout.png",verified
```

![The Yoke console plan page in dark theme for login-rate-limit, showing the plan done with two requirements satisfied by passing checks, two tasks done, and an open comment with an agent reply](docs/assets/console-plan-dark.png)

### Console and notifications

```sh
yoke console            # opens a one-time sign-in link in your browser
yoke console --no-open  # prints the link instead
```

The console listens on loopback only and must be started by a human in their own terminal, because it can approve and resume plans.
Its link works once, within five minutes, and signs that browser in with an `HttpOnly` session cookie; Ctrl-C stops the server.
It covers Projects, Sessions, Plans, Decisions, Memory, Trust & config, Usage, Notifications and Workflow settings, in light and dark themes with keyboard navigation.
Notifications collects hook errors, observe-mode would-deny summaries, waiting or stalled plans, failed checks and stale or untrusted configuration, each with recovery steps, and `yoke notifications` prints the same list.

![The Yoke console Notifications page listing a failed plan check with its output and a plan waiting for a human answer, each with recovery help and an Acknowledge button](docs/assets/console-notifications-light.png)

### Memory

```sh
yoke remember --pinned "Run sh tests/rate_limit.sh before asking for review"
yoke recall rate
yoke recall --explain
```

Notes belong to the current project, and recognizable secrets are redacted before they are stored: known token prefixes, `key=value`, `--key value` and `key: value` pairs for names like `password` and `api_key`, bearer headers and the password in a URL such as `postgres://user:password@host`.
In enforce mode, session start injects pinned notes, trusted shared knowledge and recent notes within `memory.inject_tokens` (default 1500), and `--explain` shows exactly what would be injected and why.
`yoke memory export` and `yoke memory import` move memory between machines with explicit project-path mapping, and optional session digests are off by default.

### Usage, trials and extensions

- Yoke's statusline adds its mode, git branch, allowed and blocked counts, plan, alert and quota line to Claude Code (the 5-hour and weekly percent used and the reset time, when Claude reports them; `--style detailed` adds a second line). The install script adds it when you have no statusline; otherwise run `yoke install --agent claude --statusline`, which wraps an existing statusline command. Without it, none of this shows inside the agent.
- `yoke usage` prints each agent's quota (percent used and left, reset times), token and estimated-cost totals for today and the last 7 days, and the newest sessions, read-only.
- The console Usage page and `yoke usage` estimate API token prices from agent transcripts; an unpriced model shows as unknown rather than a guessed amount, and the figures are not invoices.
- `yoke trial report --since <date>` summarizes recorded decisions into release-trial evidence, read-only.
- `yoke extensions` inventories Claude Code and Codex skills, agents, plugins, MCP servers, hooks and memory files read-only, and exits 1 when it finds stale or dangling definitions (shadowed names are reported without failing).

## Security model and limits

- Yoke is not a sandbox.
  Its self-protection stops an agent from casually approving its own plan, starting the console or editing Yoke state through ordinary tools, but an agent with a shell running as your OS user can deliberately work around it.
  Read [Security model and limits](docs/workflows.md#security-model-and-limits) and use OS-level isolation if you need more.
- Trusting a configuration authorizes its shell commands to run with your permissions, and observe mode still runs them.
- Human-only commands refuse agent markers and noninteractive terminals; an action recorded with the actor `human` passed that check, which does not prove a person acted.
- In enforce mode a Yoke failure on a blocking event denies the action; the explicit bypass is `yoke pause`, which only a human can run: it prints a private token to start the agent with, or pauses the hooks for a limited time, as [troubleshooting](docs/troubleshooting.md#storage-failures-block-work) explains.
- Yoke's own network use is `yoke update` and a background release check at most once a day at session start.
  Independent review and memory digests run the Claude Code or Codex CLI, which sends the selected material to that agent's service.
- Builds of the original 0.1.0 skeleton commit (`db81d08`) predate trusted snapshots and execute configuration straight from disk; use them only in trusted repositories.

## Status

Yoke 0.1.0 is a pre-release: no release is published, so `yoke update` has nothing to install, and the one-line install builds `main` from source until the first release exists.
CI passes on Ubuntu and macOS, the installer passes on nine fresh Linux images, and the release package for the build host passed a local dry run.
Still open: the release workflow has never run, so the three other builds are unverified, and the compliance ticks, the trademark search, console acceptance and update acceptance are not done.
The real-work trials are also open: only a partial Codex observe-mode day has been recorded, and neither agent has had its enforce-mode week.
The [gate status table](docs/release-acceptance.md#gate-status) lists every gate with its evidence, the [releasing runbook](docs/releasing.md) is the path to a published release, and [trials](docs/trials.md) records the real-work evidence.
Read the [upgrade notes](docs/upgrade-notes.md) before replacing an existing Yoke binary, because the first new binary to open the state database migrates it.

## Documentation

| Guide | Use it to |
| --- | --- |
| [Manual](MANUAL.md) | Get a five-minute explanation of what Yoke is and one full worked example |
| [Getting started](docs/getting-started.md) | Build, install, trust a policy, and inspect your first decision |
| [Configuration](docs/configuration.md) | Understand defaults, layered trust, gates, and event behavior |
| [CLI reference](docs/cli.md) | Find every command, argument, and machine-readable output |
| [Workflows](docs/workflows.md) | Use plans, independent review, memory, presets, and the console |
| [Troubleshooting](docs/troubleshooting.md) | Diagnose inactive gates, failed checks, and storage problems |
| [Upgrade notes](docs/upgrade-notes.md) | See every behavior change before replacing a binary |
| [Development handoff](docs/next-steps.md) | Find the current checkpoint and remaining acceptance roadmap |
| [Decision records](docs/decisions/README.md) | Read why each design choice was made and where it stops |
| [Design](docs/specs/2026-10-13-yoke-design.md) | Read the full design and the road to 1.0 |

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) for the repository map and focused validation, and [COMPLIANCE.md](COMPLIANCE.md) for the clean-room rules every contribution follows.
Yoke is an independent implementation built only from public documentation and idea-level concepts.
[CHANGELOG.md](CHANGELOG.md) is generated from the records in `docs/changes/`, so edit those records instead.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
