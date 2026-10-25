# Getting started

[Documentation index](README.md)

## Prerequisites

Use a recent stable Rust toolchain with Cargo and a local checkout of this repository.
Agent integration requires Claude Code or Codex installed separately.
Independent review and memory digests additionally require an authenticated agent CLI.
SQLite is bundled through the Rust dependency, so a separate database server is unnecessary.

## 1. Build and install the executable

To install or upgrade without a checkout, run the install script:

```sh
curl -fsSL https://raw.githubusercontent.com/mrx-arafat/yoke/main/install.sh | bash
```

It supports macOS and Linux on x86_64 and aarch64, including a fresh server over SSH as root.
On Alpine, use `| sh` instead of `| bash`, because Alpine ships without bash.
It uses `sudo` only to install a C compiler, after asking.
Every question defaults to yes and is read from your terminal, so it works through the pipe.
It downloads `yoke-<target>` and `yoke-<target>.sha256` from the latest GitHub release, the same assets `yoke update` uses, and refuses to install unless the SHA-256 checksum matches.
When the release has no executable for your platform, including while no release is published, it builds from source with `cargo install --locked --git https://github.com/mrx-arafat/yoke yoke-cli`, always at one exact revision: `YOKE_COMMIT` or `YOKE_VERSION` if you set one, otherwise the newest release tag, and while no release exists the commit `main` points to when the installer runs, which it looks up, prints and builds as that one hash.
It stops instead of guessing when it cannot look up the newest release or that commit, and its message gives the `YOKE_COMMIT=` command to run with a hash you choose.
Downloads use TLS 1.2 or newer, time out after ten minutes and stop at 64 MiB.
The SHA-256 file comes from the same place as the executable, so it catches a corrupt download but not a replaced release; [Verifying releases](verifying-releases.md) shows how to check the signed build provenance.
Release executables need glibc, so on Alpine and other musl systems it always builds from source.
Before building, it asks to install whatever is missing, picking the package manager by its command:

| Platform | Package manager | What the installer adds when missing |
| --- | --- | --- |
| macOS | none | Opens the Command Line Tools installer with `xcode-select --install`, then stops so you can rerun it when they finish |
| Ubuntu, Debian | `apt-get` | `build-essential`, then Rust with rustup |
| Fedora, Rocky Linux, Amazon Linux 2023 | `dnf` | `gcc`, then Rust with rustup |
| Older yum-based systems | `yum` | `gcc`, then Rust with rustup |
| openSUSE | `zypper` | `gcc`, then Rust with rustup |
| Arch Linux | `pacman` | `gcc`, then Rust with rustup |
| Alpine | `apk` | `build-base`, then Rust with rustup |

On Linux it installs the compiler directly as root and otherwise with `sudo`.
Without a terminal, each of these steps stops with the exact command to run, unless `YOKE_YES=1` answers yes.
It installs into `/usr/local/bin` as root and into `~/.local/bin` otherwise, replacing the executable atomically.
An executable in `/usr/local/bin` serves every user, but hooks and state are per user, so each user who runs an agent runs the installer once.
It runs `yoke install` for the agents it finds, creates or migrates the state database with `yoke log` once the hooks run the new executable, and reports `yoke health`.
Once Claude's hooks are connected it also looks at Claude Code's `statusLine` setting.
With none, it asks (default yes) and adds Yoke's line, which shows the mode, context and your 5-hour and weekly quota from Claude Code's next session; restart sessions that are already running.
If you already have a statusline command, the installer keeps it and tells you the command that wraps it; at a terminal it asks first, defaults to no, and `YOKE_YES=1` never answers yes to it.
If the line is already Yoke's, it does nothing.
Codex has no statusline hook, so `yoke usage` is where Codex quota shows, when Codex records it.
With `YOKE_AGENTS=none`, or when it finds no agent, it leaves the database alone, so hooks that still run an older executable keep working; rerun it after installing an agent to connect that agent's hooks.
Before it replaces the executable it checks that the state database is not newer than the new executable, because hooks running an older `yoke` would deny every tool call; if it is, the script stops with nothing changed and says how to recover.
If your hooks run a Yoke executable from another location, it runs `yoke uninstall` with the new executable and then installs again, and tells you where the earlier copy is still on disk so you can remove it.
Once the hooks are connected, Yoke records hook events from every Claude Code and Codex session of that user, in every project, including unrelated ones; observe mode, the default, never blocks anything, and no gate runs until you trust a policy (step 2).
When the install directory is not on your `PATH`, it asks to add one line marked `# added by yoke install.sh` to `~/.bashrc`, and to `~/.zshrc` when your shell is zsh; open a new login shell or run `exec $SHELL -l` afterwards.
Shells that skip that file do not find `yoke`: cron, scripts and, on Ubuntu, `ssh host command`, where `~/.bashrc` returns early for non-interactive shells.
Use the full path there, for example `ssh host '~/.local/bin/yoke health'`; the hooks always use the absolute path.
Without a terminal and without `YOKE_YES=1`, the installer does not edit the file: it prints the exact line and runs its follow-up commands by full path.
That is deliberate.
A piped install cannot ask, a shell startup file is yours rather than Yoke's, `yoke uninstall` does not remove the line again, and nothing needs it: the hooks use absolute paths and non-interactive shells skip the file.
`YOKE_YES=1` is the explicit opt-in for cloud-init and CI.
When no trusted global policy exists and you are at a terminal, it offers to add the `git-safety` preset to `~/.yoke/config.toml` in observe mode and runs `yoke trust --global`, which shows the exact bytes and waits for you to type the confirmation phrase.
`YOKE_YES=1` never answers that confirmation; without a terminal the script prints `yoke preset add git-safety --global` and `yoke trust --global` for you to run.
These environment variables change what it does:

| Variable | Effect |
| --- | --- |
| `YOKE_INSTALL_DIR` | Absolute install directory, default `/usr/local/bin` for root and `~/.local/bin` otherwise. It is written into your shell startup file, so it must not contain a double quote, dollar sign, backtick, backslash or newline. |
| `YOKE_VERSION` | Release tag to download, or to build with `--tag`, instead of the latest. |
| `YOKE_COMMIT` | Full 40-character commit to build from source, for example to test one commit. It cannot be combined with `YOKE_VERSION`. |
| `YOKE_DOWNLOAD_URL` | Base `https://` URL of a mirror that serves the release assets. Plain `http://` is accepted only for `localhost`, `127.0.0.1` and `[::1]`. |
| `YOKE_FROM_SOURCE=1` | Build from source even when a release exists. |
| `YOKE_YES=1` | Answer yes to every question, also without a terminal, for cloud-init or CI; trusting a policy still needs a human. |
| `YOKE_INSTALL_RUST=1` | Install Rust without asking when Cargo is missing. |
| `YOKE_NO_MODIFY_PATH=1` | Never add the install directory to shell startup files. |
| `YOKE_AGENTS` | Agents to connect, such as `claude` or `claude,codex`, or `none`; by default each agent found on `PATH` or by its configuration directory. |
| `YOKE_NO_STATUSLINE=1` | Never touch Claude Code's `statusLine` setting. |

To build from a local checkout instead, from the repository root:

```sh
cargo build --release
./target/release/yoke --version
cargo install --path crates/yoke-cli --locked
yoke --help
```

To build straight from GitHub without the script: `cargo install --locked --git https://github.com/mrx-arafat/yoke yoke-cli`.
Cargo installs the executable into its configured binary directory, normally `~/.cargo/bin`.
Ensure that directory is on your PATH.
The remaining commands run from the project you want Yoke to govern, which need not be Yoke's own repository.
`yoke --help` shows the setup sequence; `yoke init --help`, `yoke trust --help`, `yoke install --help` and `yoke health --help` include examples without reading or creating state.

## 2. Create and approve a project policy

```sh
yoke init
```

This creates `yoke.toml` at the project root without overwriting an existing file.
Run from a subdirectory of a repository, it writes the repository's root file and prints that path, so one project never splits in two.
The starter uses observe mode, names the project after its root directory, and contains one `no-force-push` gate using the git-safety preset's force-push matcher across `Bash`, `exec_command`, `shell_command` and `shell`.
A commented `[memory]` block shows where to turn on session digests.
It recognizes common force flags and refspecs, including `-f`, `--force-with-lease` and `+main`; aliases, expansions and other programs still need operating-system isolation.
Inspect and edit the file for your agent's actual payloads before enabling enforcement.

From your own interactive terminal:

```sh
yoke trust
yoke config
yoke gates
```

Review the displayed snapshot and type the requested confirmation phrase.
Trust cannot be approved noninteractively or from a detected agent tool process.
Subsequent edits remain inactive until you trust them again.

### Where Yoke looks for your project

Trust, gates, memory, plans and the decision log all belong to a project.
Yoke finds it from the directory you run in, after resolving symlinks and letter case, so one directory never becomes two projects.
For a hook, the project is fixed where the session starts: the directory Claude Code names in `CLAUDE_PROJECT_DIR`, else the directory the first event of the session reports, remembered for the rest of the session.
A directory the agent reports later never changes the project, and never turns its policy off, whether it is outside the project, a symlink that leaves it, or holds a `yoke.toml` nobody trusted.
It only selects a `yoke.toml` that a human trusted, at or below the directory the session started in.
`yoke config` prints the project it resolved.

Walking upward, the project is the first of these that applies:

1. The nearest directory with a `yoke.toml` that a human trusted.
2. The nearest directory with a `yoke.toml` that is not yet trusted.
3. The root of the git repository.
4. The directory itself.

| Where you run Yoke | Project |
| --- | --- |
| A directory outside any git repository | That directory; a subdirectory of it is a different project |
| A repository with no commits yet | The repository root; plans need a first commit |
| A subdirectory of a repository | The repository root, unless a `yoke.toml` sits closer |
| A monorepo package with its own `yoke.toml` | That package once a human trusts its file; until then its file is ignored, `yoke gates` says so, and the repository's trusted policy applies. An agent session started at the repository root applies the package's trusted policy while it works in the package, but its memory, plans and Stop guard stay with the repository |
| A linked git worktree | The worktree directory, with its own memory, plans and log; the policy trusted for the repository applies there until you trust the worktree's own `yoke.toml` |
| A submodule | The submodule's root, unless a `yoke.toml` above it covers it |
| A bare repository | The bare repository directory |
| A directory you renamed or moved | A new project; trust, memory, plans and log stay under the old path |

Moving a project means trusting its `yoke.toml` again with `yoke trust`.
To keep the project's memory across the move, see [the memory import notes](troubleshooting.md#memory-import-fails-or-recall-looks-empty).

## 3. Exercise the starter gate safely

```sh
printf '%s\n' '{"tool_name":"Bash","tool_input":{"command":"git push --force"}}' |
  yoke run-hook --agent claude pre_tool_use
yoke log --outcome would_deny --limit 5
```

This sends a simulated payload; it does not execute Git.
Observe mode produces no hook output, and the decision log should contain `would_deny` when the trusted starter gate matches.
If it does not, inspect [troubleshooting](troubleshooting.md).
`yoke log` lists the project you run it in; add `--all` to see every project.
An empty `would_deny` listing says when the project has no trusted gate, so no gate has run.

## 4. Connect an agent

```sh
yoke install --agent claude,codex --dry-run
yoke install --agent claude,codex
```

Select only `claude` or `codex` if you use one agent.
Installation preserves unrelated hooks and creates backups and ownership receipts.
It is idempotent, and every hook it installs carries a label such as `Yoke: PreToolUse` in both agents.
After updating Yoke, rerun `yoke install --agent claude,codex` to label existing Yoke-owned hooks without adding duplicates.
Hooks you registered manually or changed yourself remain untouched.
For Codex, open Codex itself, run `/hooks` and approve the Yoke hooks: Codex skips hooks you have not approved without any warning, including in `codex exec`, and `yoke health` reports how many of the seven Yoke hooks have no approval record.
If you run Claude Code with `CLAUDE_CONFIG_DIR` or Codex with `CODEX_HOME`, keep the same variable set when you run `yoke install`, so the hooks land in the files the agent reads.
Restart or reload your agent's hook configuration as appropriate, perform a normal tool action, and inspect `yoke log` for the resulting decision.
Run `yoke health` to confirm that each agent reports installed, enabled hooks and a recorded event; it exits 1 when a check fails.
If an installed agent has no recorded event, health names its launch command and the `yoke log --limit 10` and `yoke health` checks to run after a tool action.

Claude's statusline is the way to see Yoke inside the agent: it puts the mode (and whether any trusted policy applies), your git branch and changed-file count, this session's allowed and blocked actions, the plan state, your 5-hour and weekly quota, the session's cost and context use, and alerts such as `! 2 unread` in Claude Code's prompt bar, dropping the least important parts when the terminal is narrow.
Add `--style detailed` to either command below for a second line, and see [Statusline](cli.md#statusline) for every segment, `NO_COLOR` and `YOKE_ASCII=1`.
The install script adds it when you have no statusline, and `yoke install --agent claude,codex` does not; add it with:

```sh
yoke install --agent claude --statusline-if-absent
yoke install --agent claude --statusline
```

The first adds Yoke's line only when you have none, reports any other statusline it left alone, and does nothing when the line is already Yoke's.
The second wraps an existing statusline command while preserving its input and settings: Yoke's line comes first and that command's output follows, but its colors are removed, its lines are joined, and the whole line is dropped when the command takes over 250 ms or fails.
Uninstall restores the prior statusline when Yoke still owns it and preserves your subsequent changes.
With the statusline installed, Claude's quota is also recorded for `yoke usage`, which prints quota, usage and recent sessions without the console.

### Register hooks manually

If you register hooks yourself instead of using `yoke install`, include the agent name in each invocation.
For example, Claude Code settings can include:

```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "*", "hooks": [{ "type": "command", "command": "yoke run-hook --agent claude PreToolUse", "statusMessage": "Yoke: PreToolUse" }] }
    ],
    "Stop": [
      { "hooks": [{ "type": "command", "command": "yoke run-hook --agent claude Stop", "statusMessage": "Yoke: Stop" }] }
    ]
  }
}
```

## 5. Enable enforcement when ready

Set the top-level `mode` in `yoke.toml` to `"enforce"`, then run `yoke trust` again from your own terminal.
Re-trust shows the change against the approved snapshot with surrounding context; review the `-` and `+` lines before typing `trust`.
If you use global policy instead, `yoke health` names the file supplying the mode and the matching `yoke trust --global` command.
For stale policy, follow the scope-specific trust command in health; restore a missing trusted file before approving new bytes.
Repeat the simulated payload above to inspect the agent-specific denial response.
Only `pre_tool_use`, `user_prompt_submit`, and `stop` can block; other events report failures without vetoing the action.

To remove managed hooks, inspect and apply:

```sh
yoke uninstall --agent claude,codex --dry-run
yoke uninstall --agent claude,codex
```

Uninstall removes Yoke-owned integration entries rather than erasing project policy or memory.
For the full sequence, including the executable, the PATH line and `~/.yoke`, see [uninstalling](uninstalling.md).
On a remote server, open the console through an SSH tunnel as [workflows](workflows.md#console-on-a-remote-server) describes.
Continue with the [configuration reference](configuration.md) to define your own gates.

## 6. Add project memory (optional)

```sh
yoke remember --pinned 'Run the focused tests before asking for review'
yoke recall
yoke recall --explain
```

Notes belong to the current project, and `yoke recall` prints the project it searched.
Session start injects them only in enforce mode, so finish step 5 first; in observe mode `yoke recall --explain` prints `injection: inactive`.
Automatic digests stay off until you add `digest = true` under `[memory]` in `~/.yoke/config.toml` or `yoke.toml` and run `yoke trust` in your own terminal.
See [workflows](workflows.md#project-memory) for sharing, forgetting and transferring notes.

