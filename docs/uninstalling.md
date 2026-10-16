# Uninstalling Yoke

[Documentation index](README.md)

This page reverses what `install.sh` and `yoke install` did.
Run every command in your own terminal.
The order matters: remove the hooks first and the executable second, because the hooks name the executable by absolute path.

## What Yoke put on your machine

| What | Where | Removed in |
| --- | --- | --- |
| Hook entries | Claude `settings.json`, Codex `hooks.json`, and `hooks = true` under `[features]` in Codex `config.toml` | [Step 1](#1-remove-the-hooks) |
| Ownership receipts | `~/.yoke/install-claude.json` and `~/.yoke/install-codex.json` | [Step 1](#1-remove-the-hooks) |
| The executable | `~/.local/bin/yoke`, `/usr/local/bin/yoke` when the installer ran as root, `$YOKE_INSTALL_DIR/yoke`, or `~/.cargo/bin/yoke` for a Cargo install | [Step 2](#2-remove-the-executable) |
| A `PATH` line | `~/.bashrc`, and `~/.zshrc` when your `SHELL` was zsh, only if you let the installer add it | [Step 3](#3-remove-the-path-line) |
| State | `~/.yoke`: the state database `state.db` (decision log, memory, trust approvals), `quota-claude.json` (the last Claude quota reading), `backups/`, and `config.toml` when you accepted the starter global policy | [Step 4](#4-delete-the-state-optional) |
| Project files | `yoke.toml` and `.yoke/` in each project where you used Yoke | [Step 4](#4-delete-the-state-optional) |

## Before you start: keep your data (optional)

Step 4 deletes your decision history and memory, and steps 1 and 2 remove the only program that can read them.
To keep them, export first, while `yoke` still works:

```sh
yoke memory export ~/yoke-memory.json --all
(umask 077; yoke log --limit 1000 --json > ~/yoke-decisions.json)
```

`yoke memory export` writes a JSON file with your memories and the facts waiting to become memories, for every project with `--all`.
It refuses to replace an existing file unless you add `--overwrite`.
`yoke log` saves only the newest 1000 decisions, which is the largest `--limit` it accepts.
Both files are plain text, and the decision log includes what your agents ran, so the `umask 077` keeps the second one readable by you alone (the memory export already is).
Delete both when you no longer need them.
Plans are files under `.yoke/plans` in each project, so they are not part of either export.

## 1. Remove the hooks

Inspect the planned changes, then apply them:

```sh
yoke uninstall --agent claude,codex --dry-run
yoke uninstall --agent claude,codex
```

The real run prints `changes: 3` and the three files it changed.
The dry run prints the same list plus each file's contents before and after, and changes nothing.

What it does:

- It removes only the entries Yoke added, as recorded in the receipts.
- When you have not touched the files since installing, each one is restored byte for byte.
- When you have edited them since, your edits stay and only Yoke's entries go.
- A file that did not exist before the install is deleted again.
  The `.claude` and `.codex` directories stay behind, empty.
- If a Yoke entry itself was changed or duplicated, it stops with an error naming the hook and leaves the file untouched.
  Fix what the message names and run it again.
- Before changing anything it copies the files, as they are at that moment, into `~/.yoke/backups/<timestamp>-<id>/` as `0.original`, `1.original` and so on, with an `index.json`.
  The backup from your install holds your files from before Yoke existed.
  Keep `~/.yoke/backups` until you are sure nothing is missing.
- Running it again is harmless and prints `changes: 0`.

Check the result:

```sh
yoke health
```

It now exits 1 with `Yoke hooks are not installed for any agent.`, which is the expected state here and not a fault.

### If you set `CLAUDE_CONFIG_DIR` or `CODEX_HOME`

Yoke installs into Claude settings under `CLAUDE_CONFIG_DIR` and Codex files under `CODEX_HOME` when those variables are set, which must then be absolute paths.
Otherwise it uses `~/.claude` and `~/.codex`.
Run `yoke uninstall` with the same values you installed with.
With different values it stops with `settings path differs from installation receipt` and changes nothing; the message names the directory the hooks were installed into and says which variable to set or unset.

### If you already deleted the executable

The hooks still name the missing path, and `yoke uninstall` needs a working executable.
Any working `yoke` can run the uninstall, even one at a different path, because it works from the receipts.
To get one back without reconnecting any hooks, run the installer again with `YOKE_AGENTS=none`, then do step 1:

```sh
curl -fsSL https://raw.githubusercontent.com/mrx-arafat/yoke/main/install.sh | YOKE_AGENTS=none bash
```

It may offer to add the `PATH` line again.
Answer `n`, or set `YOKE_NO_MODIFY_PATH=1`.

## 2. Remove the executable

Find the copy your shell runs:

```sh
command -v yoke
```

Then remove the one that matches how you installed:

```sh
rm -f ~/.local/bin/yoke
```

- The installer put it there when you ran as an ordinary user.
- When the installer ran as root it used `/usr/local/bin/yoke`, so run `sudo rm -f /usr/local/bin/yoke`.
- When you set `YOKE_INSTALL_DIR`, run `rm -f "$YOKE_INSTALL_DIR/yoke"` with the same value.
- When you ran `cargo install` yourself, run `cargo uninstall yoke-cli`.
  The package is `yoke-cli` and its executable is `yoke`.
  The installer's source build installs into a temporary directory and copies the executable out, so it never leaves a Cargo copy.

- When you reran the installer with a different `YOKE_INSTALL_DIR`, it moved the hooks but left the earlier copy on disk, so remove that one as well.

Leave `~/.local/bin` itself in place, because other tools may use it.
Your shell remembers where it found `yoke`, so right after `rm` the check below can still print the path you just deleted.
Make the shell forget it first, with `hash -r` in bash or `rehash` in zsh, then check again:

```sh
hash -r 2>/dev/null || rehash
command -v yoke
```

If `command -v yoke` still prints a path after that, another copy is on your `PATH`; remove that one too.

## 3. Remove the PATH line

Skip this step if the installer never added a line.
It only does so when you answered yes, and each line it writes ends with `# added by yoke install.sh`.
See whether you have any:

```sh
grep -n 'added by yoke install.sh' ~/.bashrc ~/.zshrc
```

A `No such file or directory` message for a file you do not use is normal.
To delete exactly the lines that end with that marker and nothing else:

```sh
for f in ~/.bashrc ~/.zshrc; do
    if [ -f "$f" ]; then perl -i -ne 'print unless /# added by yoke install\.sh$/' "$f"; fi
done
```

- A line that merely mentions the marker in the middle is kept.
- The blank line the installer wrote above its line stays, and a file that had no final newline gains one.
  Both are harmless.
- A startup file the installer created, such as `~/.zshrc` for a zsh user who had none, is left behind holding only blank lines.
  Delete it yourself if you never wrote anything in it; `cat ~/.zshrc` shows whether it is empty.
- `perl -i` replaces a symlink with a regular file and leaves the file it pointed to unchanged.
  If `ls -l ~/.zshrc` or `ls -l ~/.bashrc` shows `->`, run the `perl` command on the file the link points to instead.
- A `PATH` line you wrote yourself has no marker and stays.
  Remove it by hand.

Open a new terminal so the change takes effect.

## 4. Delete the state (optional)

`~/.yoke` holds your decision history, memory, trust approvals, and the backups of your agent files.
Deleting it cannot be undone.
Skip this step if you might reinstall, because a new install picks up the existing history.
Export first if you want a copy, as described above.
If you want to keep the backups, copy `~/.yoke/backups` somewhere else first.

```sh
rm -rf ~/.yoke
```

Run this in a plain terminal, not through an agent.
While Yoke's hooks are installed, an agent's shell command that removes `~/.yoke` is denied in enforce mode and recorded as a would-deny in observe mode.

Uninstalling and deleting `~/.yoke` leave your project files alone.
In each project where you used Yoke, `yoke.toml` and the `.yoke` directory hold the policy, the plans in `.yoke/plans`, and shared notes in `.yoke/knowledge.md`.
They are ordinary files.
Delete them project by project if you want them gone, after checking whether your repository tracks them.

## 5. Clean up in Codex

Codex records which hooks you approved in `config.toml`, in `~/.codex` or in `CODEX_HOME`.
Each approval is a table named after the `hooks.json` path, the event and the position of the hook, with a `trusted_hash`, for example `[hooks.state."/home/you/.codex/hooks.json:pre_tool_use:0:0"]`.
Codex writes these, not Yoke, and `yoke uninstall` leaves them in place.
They are keyed by position and can cover other tools' hooks in the same file, so do not delete them by guess.
They do not run anything.

Open `/hooks` in Codex and confirm that no Yoke hook is listed.
Close any agent session that started before step 1, in case it loaded the hooks at start.

## Verification status

The following was run with the development build, `target/debug/yoke`, on macOS arm64 in temporary home directories, never against a real home:

- The installer against a local server, with `YOKE_AGENTS=none`, adding the `PATH` lines.
- `yoke install` and `yoke uninstall` with and without existing user content in the three agent files, with SHA-256 comparison.
- Edits made after installing, `CLAUDE_CONFIG_DIR` and `CODEX_HOME` mismatch, and uninstalling with an executable at another path.
- `yoke memory export`, `yoke log --json`, `yoke share`, `rm` of the executable and of `~/.yoke`, and the `perl` command above on the files the installer wrote.

It was not run: the root install location, Linux, `cargo uninstall` against a real Cargo install, the `/hooks` screen in Codex itself, or the installer from its public URL.
The Codex approval format above was read from a Codex 0.163.0 configuration file and was not exercised by Codex.
