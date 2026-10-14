# 0003 Extension inventory: read-only report now, native distribution first

Status: accepted on 2026-10-14 for issue #17.

## Context

Yoke installs managed hooks with ownership receipts (`crates/yoke-cli/src/install.rs:156`, `crates/yoke-cli/src/install.rs:211`) and appends bundled presets marked `# Yoke preset: <name>` (`crates/yoke-core/src/preset.rs`).
Nothing showed which skills, rules, custom agents, plugins, MCP servers, hooks or commands Claude Code and Codex would load, which definition wins when two scopes use the same name, or whether Yoke's own managed definitions still match what Yoke would install.
Issue #17 asks for that inventory and a read-only drift report before any team distribution write path exists.

## Decision

`yoke extensions [--json]` reads the documented locations below, prints one row per definition with agent, kind, name, scope, Yoke ownership and origin path, then a drift table.
It exits 0 when no drift is found and 1 when any drift is found, after printing the full report.
It never writes extension files, never executes extension code, and never starts MCP servers.
The only state it touches is the usual `~/.yoke/state.db` open that every project command performs to resolve the project root.

### Safety rules

Configuration files are read only when they are regular files of at most 4 MiB; Markdown files are read only up to their first 64 KiB to find the frontmatter `name`.
Symlinks are reported when their target leaves every scanned root, and symlinked directories are listed but never walked.
Directory walks stop at 8 levels and 2000 files per location, with a note.
Shape-based redaction cannot recognise every secret (webhook paths, `curl -u` credentials, cookies, unknown key formats), so values are never displayed.
Hook and MCP commands are reported as the program name and an argument count, MCP URLs as scheme and `host[:port]` only, and MCP environment variables and headers as key names only.
Every displayed name and summary still passes through `yoke_core::redact::redact`.
Hook and MCP commands are kept unredacted only in memory for the existence check and ownership match; they are never serialized.

### Locations scanned

Claude Code (`CLAUDE_CONFIG_DIR` replaces `~/.claude`, and `.claude.json` is then read from inside it):

| Kind | Scope and path | Source |
| --- | --- | --- |
| Hooks, plugins | managed `/Library/Application Support/ClaudeCode/managed-settings.json` (macOS) or `/etc/claude-code/managed-settings.json`, plus `managed-settings.d/*.json` | https://code.claude.com/docs/en/managed-settings.md |
| Hooks, plugins | user `~/.claude/settings.json`, project `.claude/settings.json`, local `.claude/settings.local.json` | https://code.claude.com/docs/en/settings.md |
| Skills | managed `<managed dir>/.claude/skills/<name>/SKILL.md`, user `~/.claude/skills/`, project `.claude/skills/` | https://code.claude.com/docs/en/skills.md |
| Commands | user `~/.claude/commands/**/*.md`, project `.claude/commands/**/*.md` | https://code.claude.com/docs/en/skills.md |
| Subagents | managed `<managed dir>/.claude/agents/`, user `~/.claude/agents/`, project `.claude/agents/`, recursive, identity from frontmatter `name` | https://code.claude.com/docs/en/sub-agents.md |
| MCP servers | managed `<managed dir>/managed-mcp.json`, user `~/.claude.json` `mcpServers`, local `~/.claude.json` `projects.<project path>.mcpServers`, project `.mcp.json` | https://code.claude.com/docs/en/mcp.md |
| Memory | managed `<managed dir>/CLAUDE.md`, user `~/.claude/CLAUDE.md`, project `CLAUDE.md` and `.claude/CLAUDE.md`, local `CLAUDE.local.md` | https://code.claude.com/docs/en/memory.md |
| Rules | user `~/.claude/rules/**/*.md`, project `.claude/rules/**/*.md` | https://code.claude.com/docs/en/memory.md |

Codex (`CODEX_HOME` replaces `~/.codex`):

| Kind | Scope and path | Source |
| --- | --- | --- |
| MCP servers, inline hooks, plugins | system `/etc/codex/config.toml`, user `~/.codex/config.toml`, project `.codex/config.toml` | https://developers.openai.com/codex/config-basic and https://developers.openai.com/codex/extend/mcp |
| Hooks | user `~/.codex/hooks.json`, project `.codex/hooks.json` | https://developers.openai.com/codex/hooks |
| Skills | admin `/etc/codex/skills`, user `$HOME/.agents/skills`, project `.agents/skills` | https://developers.openai.com/codex/skills |
| Custom agents | user `~/.codex/agents/*.toml`, project `.codex/agents/*.toml`, identity from the `name` field | https://developers.openai.com/codex/agent-configuration/subagents |
| Custom prompts (deprecated) | user `~/.codex/prompts/*.md` | https://developers.openai.com/codex/custom-prompts |
| Memory | `AGENTS.override.md` and `AGENTS.md` in `~/.codex` and the project root | https://developers.openai.com/codex/agent-configuration/agents-md |
| Rules | user `~/.codex/rules/*.rules`, project `.codex/rules/*.rules` | https://developers.openai.com/codex/agent-configuration/rules |

Yoke: preset blocks in `~/.yoke/config.toml` and the project `yoke.toml`, and receipted hooks in `~/.claude/settings.json` and `~/.codex/hooks.json`.

### Drift findings

`shadowed`: the same name is defined more than once in one namespace.
The report states the winner only where the agent documents it.
Claude skills: managed over personal over project, and a skill beats a `.claude/commands/` file of the same name (https://code.claude.com/docs/en/skills.md, "Resolve skills that share a name").
Claude subagents: managed, then project, then user (https://code.claude.com/docs/en/sub-agents.md, "Choose the subagent scope").
Claude MCP servers: local, then project, then user, and "fields are not merged across scopes" (https://code.claude.com/docs/en/mcp.md, "Scope hierarchy and precedence"); managed servers are ranked first.
Claude plugins follow settings precedence, managed, local, project, user (https://code.claude.com/docs/en/settings.md); plugins enabled identically at several scopes are not reported.
Codex MCP servers and plugins follow config layer precedence, project over user over system (https://developers.openai.com/codex/config-basic); whether a server's fields merge across layers is not documented, and the report says so.
Codex skills with the same `name` both load ("Codex doesn't merge them", https://developers.openai.com/codex/skills).
Claude user versus project commands and Codex user versus project custom agents have no documented winner, and the report says "not documented".

`stale`: a Yoke-managed definition differs from what this executable would install now.
Receipted hooks are compared with the groups `install.rs` would write, reusing its receipt reader through the child module `crates/yoke-cli/src/install/drift.rs`.
A receipt naming another executable, an unlabeled legacy hook, a missing or edited managed hook, and an unreadable receipt are each reported.
A preset block that no longer contains the bundled text for its name is stale; presets that need options (`web-redirect`, `token-saver`) cannot be re-rendered and are not judged.

`dangling`: a hook command or MCP stdio command whose program is not an executable file or not on `PATH`.
Hook commands are shell lines, so only the first word is resolved, `$CLAUDE_PROJECT_DIR`, `$HOME` and a leading `~/` are expanded, relative paths resolve against the project root, and anything needing further shell expansion is left unjudged.

### Known limits

Project files are listed even when the agent would ignore them until the project is trusted or `.mcp.json` servers are approved; trust and approval state is not read.
Only the project root is scanned, not parent or nested directories between the working directory and the root.
Plugin contents, `--agents` and `--add-dir` sessions, Codex profiles, `requirements.toml` and `managed_config.toml` are not inventoried.
`yoke install` always writes `~/.claude/settings.json` and `~/.codex/hooks.json`, so with `CLAUDE_CONFIG_DIR` or `CODEX_HOME` set, receipted hooks live in a file the agent does not read and are not marked managed.

## Team distribution and conflict handling (design only, no write path)

Native mechanisms already distribute team configuration: checked-in `.claude/settings.json`, `.claude/skills/`, `.claude/agents/`, `.mcp.json`, `enabledPlugins` and `extraKnownMarketplaces` for Claude Code (https://code.claude.com/docs/en/settings-reference.md), and `.agents/skills`, `.codex/config.toml` and repository plugin marketplaces for Codex (https://developers.openai.com/codex/plugins).
Yoke should rely on those and contribute review and drift detection, not a parallel package manager.
If a Yoke write path is later approved, it must follow this contract.

1. Bundle: a checked-in `yoke-extensions.toml` lists entries with agent, kind, name, target path and the SHA-256 of a plain source file kept beside it, so the whole bundle is reviewable in an ordinary code review.
2. Plan: every apply starts as a dry run that classifies each entry as `create` (target absent), `update` (target equals the last applied hash in Yoke's receipt), `conflict` (target differs from the receipt or was never applied by Yoke), or `remove` (entry dropped and target still equals the receipt), and prints before and after bytes the way `yoke install --dry-run` does.
3. Approval: applying requires a human terminal outside an agent process (`crates/yoke-cli/src/commands.rs:35`, `crates/yoke-core/src/guard.rs:146`) and a typed confirmation; executable definitions (hooks, MCP servers, statusline commands) additionally need the same exact-byte trust display as `yoke trust`.
4. Ownership: Yoke writes a receipt before touching any file, as `install.rs` already does (`crates/yoke-cli/src/install.rs:531`), and owns only bytes or JSON keys it recorded.
5. Preservation: files and keys without a matching receipt are user-owned and never overwritten, merged or deleted; conflicts are reported with a diff and resolved only by the human, per entry.
6. Scope: only project-scope targets named by the bundle are written; user and managed scopes are never written by a bundle.
7. Locality: bundles come from the local checkout; Yoke fetches nothing over the network and sends no extension content anywhere.
8. Recovery: writes are atomic with backups and a lock, and an interrupted apply can be retried or rolled back from its receipt.
