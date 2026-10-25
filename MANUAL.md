# Yoke manual

A five-minute read: what Yoke is, why it exists, and one full example of using it.
For the complete reference, see [docs/user-guide.md](docs/user-guide.md) (the long path), [docs/cli.md](docs/cli.md) (every command) and [docs/configuration.md](docs/configuration.md) (every setting). This page never repeats their detail; it just gets you to your first real decision.

## What Yoke is, in one paragraph

Yoke is a small program that watches what your AI coding agent (Claude Code or Codex) is about to do, checks it against rules you wrote and approved, and records what happened. In **observe mode** it only watches and logs. In **enforce mode** it can actually block an action and tell the agent why. It has no server, no account, and no persistent daemon: it's one executable and one SQLite file on your machine, invoked by the agent's own hook system each time something happens. (A few commands, like `update` and `job`, launch a short-lived bounded worker; nothing runs continuously in the background.)

## Why it exists

An AI agent can tell you a task is "done" without anything having actually checked that. A rule written into a prompt ("never force-push", "always run the tests first") is a request the agent might forget, misread, or override under pressure, not a guarantee. Yoke turns rules like that into code: a shell command that actually runs, actually decides, and actually blocks (or doesn't), independent of whatever the agent currently believes about its own task.

## The three ideas you need

| Idea | What it means |
|---|---|
| **Gate** | One rule: "when X happens, run this check / match this pattern, then allow, deny, or warn." You write these in a `yoke.toml` file. |
| **Trust** | A gate does nothing until a human runs `yoke trust` and approves its exact bytes. An agent cannot do this step: Yoke refuses to run `trust` from an agent process. This is deliberate, an AI can't approve its own policy. |
| **Mode** | `observe` (default) runs every gate and records what *would* happen, but blocks nothing. `enforce` actually blocks. Always start in observe. |

## Install (condensed)

```sh
curl -fsSL https://raw.githubusercontent.com/mrx-arafat/yoke/main/install.sh | bash
```

This builds and installs `yoke`, wires up hooks for Claude Code and/or Codex, and offers you a starter policy to review. Full detail, flags, and manual build steps: [docs/getting-started.md](docs/getting-started.md).

Already have `yoke` installed from source? Just make sure it's on your `PATH` and move on.

## One full example: stop an agent from force-pushing

This is the entire loop, start to finish, in a real project.

**1. Create a policy.**

```sh
cd your-project
yoke init
```

This writes `yoke.toml` with `mode = "observe"` and one sample gate already in it:

```toml
[[gate]]
name = "no-force-push"
on = "pre_tool_use"
match_tool = "Bash|exec_command|shell_command|shell"
match_pattern = '\bgit(?:\s+(?:-C|-c|--git-dir|--work-tree)\s+[^\s;]+)*\s+push\b[^;\n]*\s(?:--force(?:-with-lease)?(?:[=\s\x22]|$)|-[A-Za-z]*f[A-Za-z]*(?:[\s\x22]|$)|\+[^\s\x22]+)'
action = "deny"
message = "Force push is blocked by Yoke. Push without --force."
```

It uses the git-safety preset's force-push matcher, covering common force flags and refspecs on both agents' shell tools.
This is a heuristic, not a shell parser; aliases and expansions can evade it.
Read it. This file does nothing yet; it's just text on disk.

**2. Approve it (you, not the agent).**

```sh
yoke trust
```

Yoke shows you the exact bytes and asks you to type a confirmation phrase. Type it. Now the policy is active, in observe mode.

**3. Wire it into your agent.**

```sh
yoke install --agent claude,codex
```

This adds hook entries to Claude Code's and Codex's own settings, pointing them at your `yoke` binary. For Codex, you still need to approve the new hooks once inside a Codex session with `/hooks`.

**4. Watch it work, in observe mode first.**

Send a simulated event instead of actually force-pushing. Observe mode doesn't block anything, so a real `git push --force` would genuinely push:

```sh
printf '%s\n' '{"tool_name":"Bash","tool_input":{"command":"git push --force"}}' |
  yoke run-hook --agent claude pre_tool_use
yoke log --limit 5
```

```
decisions: 1
  TIME                 AGENT   SESSION  EVENT         TOOL  OUTCOME     GATE           DETAIL
  2026-10-24 09:12:03  claude  s1       pre_tool_use  Bash  would_deny  no-force-push  Force push is blocked by Yoke. P…
  … shortened; full records: --json
```

An ellipsis marks a shortened preview. Use `yoke log --json` for complete commands, reasons and session ids; `COLUMNS=160 yoke log` gives wider human previews.

`would_deny` means Yoke correctly identified that the gate should have fired, but observe mode let it through so you could verify the rule is right before it starts blocking anything.

**5. Turn on enforcement.**

```toml
mode = "enforce"   # in yoke.toml
```

```sh
yoke trust          # re-approve the changed file
```

Now the same action gets a real `deny`, and the agent sees the message: *"Force push is blocked by Yoke. Push without --force."* Check `yoke log` again and you'll see `deny` instead of `would_deny`.

That's the whole loop: write a gate → trust it → watch it in observe → flip to enforce once you believe it. Everything else in Yoke (plans, review, memory, usage tracking) builds on this same pattern.

## Seeing what else is happening

```sh
yoke health         # is everything wired up correctly? what's missing?
yoke gates          # what gates are trusted and will actually run?
yoke usage          # token/cost usage and quota (5-hour / weekly / 7-day) with burn-rate estimates
yoke notifications  # anything waiting for your attention
yoke console         # a local web page showing every decision, live (your terminal only)
```

If you've wired up Codex, Yoke also surfaces your Codex quota directly inside the Codex CLI itself as a `systemMessage`. You'll see a line like `Yoke Codex quota: 7d 22% (resets ...)` show up in the session without having to run any command.

## Things that are deliberately not automatable

`yoke trust`, `yoke console`, and the real `yoke install` all refuse to run from inside an agent's own tool process. If an AI agent tries to run them on your behalf, Yoke detects that and refuses, every time. This isn't a bug to work around; it's the whole point. The same goes for Codex's own `/hooks` approval. If you ever see an agent (including this one) try to script past that, don't let it.

## Turning it off

```sh
yoke uninstall --agent claude,codex   # remove the hooks (keeps your yoke.toml and history)
yoke pause                            # temporary bypass, human-only, see docs/troubleshooting.md
```

Full removal steps, including deleting `~/.yoke`: [docs/uninstalling.md](docs/uninstalling.md).

## Where to go from here

- [docs/user-guide.md](docs/user-guide.md): the long path, memory, plans, independent review, every mode in detail
- [docs/configuration.md](docs/configuration.md): every `yoke.toml` field
- [docs/cli.md](docs/cli.md): every command and flag
- [docs/troubleshooting.md](docs/troubleshooting.md): gates not firing, storage problems, recovery
