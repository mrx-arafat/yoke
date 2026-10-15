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
When the release has no executable for your platform, including while no release is published, it builds from source with `cargo install --locked --git https://github.com/mrx-arafat/yoke yoke-cli`.
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
With `YOKE_AGENTS=none`, or when it finds no agent, it leaves the database alone, so hooks that still run an older executable keep working; rerun it after installing an agent to connect that agent's hooks.
If your hooks run a Yoke executable from another location, it runs `yoke uninstall` with the new executable and then installs again.
When the install directory is not on your `PATH`, it asks to add one line marked `# added by yoke install.sh` to `~/.bashrc`, and to `~/.zshrc` when your shell is zsh; open a new login shell or run `exec $SHELL -l` afterwards.
When no trusted global policy exists and you are at a terminal, it offers to add the `git-safety` preset to `~/.yoke/config.toml` in observe mode and runs `yoke trust --global`, which shows the exact bytes and waits for you to type the confirmation phrase.
`YOKE_YES=1` never answers that confirmation; without a terminal the script prints `yoke preset add git-safety --global` and `yoke trust --global` for you to run.
These environment variables change what it does:

| Variable | Effect |
| --- | --- |
| `YOKE_INSTALL_DIR` | Absolute install directory, default `/usr/local/bin` for root and `~/.local/bin` otherwise. |
| `YOKE_VERSION` | Release tag to download, or to build with `--tag`, instead of the latest. |
| `YOKE_DOWNLOAD_URL` | Base URL of a mirror that serves the release assets. |
| `YOKE_FROM_SOURCE=1` | Build from source even when a release exists. |
| `YOKE_YES=1` | Answer yes to every question, also without a terminal, for cloud-init or CI; trusting a policy still needs a human. |
| `YOKE_INSTALL_RUST=1` | Install Rust without asking when Cargo is missing. |
| `YOKE_NO_MODIFY_PATH=1` | Never add the install directory to shell startup files. |
| `YOKE_AGENTS` | Agents to connect, such as `claude` or `claude,codex`, or `none`; by default each agent found on `PATH` or by its configuration directory. |

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

## 2. Create and approve a project policy

```sh
yoke init
```

This creates `yoke.toml` without overwriting an existing file.
The starter uses observe mode and contains a sample `no-force-push` gate matching the `Bash` tool and the literal text `push --force`.
It is an example matcher, not comprehensive protection against every force-push spelling or tool name.
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

## 3. Exercise the starter gate safely

```sh
printf '%s\n' '{"tool_name":"Bash","tool_input":{"command":"git push --force"}}' |
  yoke run-hook --agent claude pre_tool_use
yoke log --outcome would_deny --limit 5
```

This sends a simulated payload; it does not execute Git.
Observe mode produces no hook output, and the decision log should contain `would_deny` when the trusted starter gate matches.
If it does not, inspect [troubleshooting](troubleshooting.md).

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
For Codex, review new hooks using `/hooks` in Codex itself.
If you run Claude Code with `CLAUDE_CONFIG_DIR` or Codex with `CODEX_HOME`, keep the same variable set when you run `yoke install`, so the hooks land in the files the agent reads.
Restart or reload your agent's hook configuration as appropriate, perform a normal tool action, and inspect `yoke log` for the resulting decision.
Run `yoke health` to confirm that each agent reports installed, enabled hooks and a recorded event; it exits 1 when a check fails.

For Claude's optional statusline:

```sh
yoke install --agent claude --statusline
```

This wraps an existing statusline command while preserving its input and settings.
Uninstall restores the prior statusline when Yoke still owns it and preserves your subsequent changes.

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
Repeat the simulated payload above to inspect the agent-specific denial response.
Only `pre_tool_use`, `user_prompt_submit`, and `stop` can block; other events report failures without vetoing the action.

To remove managed hooks, inspect and apply:

```sh
yoke uninstall --agent claude,codex --dry-run
yoke uninstall --agent claude,codex
```

Uninstall removes Yoke-owned integration entries rather than erasing project policy or memory.
Continue with the [configuration reference](configuration.md) to define your own gates.
