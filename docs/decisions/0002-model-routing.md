# 0002: Model routing and workflow settings

Status: accepted and implemented.
Date: 2026-10-14.
Issue: #24.

## Question

Can Yoke route planning, execution, review and digest work to specific models, and what should it expose as workflow settings?

## Evidence: officially documented model selection

Sources were fetched on 2026-10-14 from the official documentation.
Claude Code pages live under `https://code.claude.com/docs/en/` (the old `docs.claude.com/en/docs/claude-code/` URLs redirect there).
Codex pages live under `https://learn.chatgpt.com/docs/` (the `developers.openai.com/codex/` URLs redirect there).
Locally installed versions were Claude Code 2.1.299 and codex-cli 0.162.1; their `--help` output agrees with the rows below.
Anything not stated in a fetched page is recorded as unknown.

### Claude Code

| Mechanism | Syntax | Scope | Works for a Yoke-launched `claude -p --setting-sources ""` | Source |
|---|---|---|---|---|
| CLI flag | `--model <alias or full name>` | Launched session | Yes | `cli-reference.md`, `model-config.md` |
| Environment variable | `ANTHROPIC_MODEL` | Launched session; `--model` wins | Yes | `model-config.md`, `env-vars.md` |
| Default for new sessions | `ANTHROPIC_DEFAULT_MODEL` | Lowest priority | Yes | `model-config.md` |
| Settings key | `"model"` | Persistent | No: user, project and local settings are not loaded; managed policy still loads | `settings-reference.md`, `agent-sdk/claude-code-features.md` |
| In-session command | `/model` | Running session, from inside it | Not a launch-time control | `model-config.md` |
| Alias remapping | `ANTHROPIC_DEFAULT_OPUS_MODEL`, `..._SONNET_MODEL`, `..._HAIKU_MODEL`, `..._FABLE_MODEL` | What an alias resolves to | Yes | `model-config.md` |
| Phase alias | `opusplan`: Opus in plan mode, Sonnet otherwise | Session | Accepted; behaviour in a `-p` run is not documented | `model-config.md` |
| Opt-in fallback | `--fallback-model a,b` | Per turn | Yes, only when passed | `cli-reference.md`, `settings-reference.md` |
| Subagent model | `--agents` JSON `model`, subagent frontmatter `model`, `CLAUDE_CODE_SUBAGENT_MODEL` | Subagents | `--agents` and the environment variable: yes | `sub-agents.md` |
| Reported model | `--output-format json` result `modelUsage`; `stream-json` `system/init` `model` | Whole run | Yes; text output reports nothing except retirement or remap warnings | `headless.md`, `model-config.md`, `agent-sdk/typescript.md` |

Precedence documented in `model-config.md`: `/model`, then `--model`, then `ANTHROPIC_MODEL`, then the `model` setting, then `ANTHROPIC_DEFAULT_MODEL`.
An unknown model is not checked up front; the first request fails with "There's an issue with the selected model" (`model-config.md`).
Fallback to another model is off by default (`settings-reference.md`, `fallbackModel`).
Two substitutions happen without opt-in: an organization-restricted model is replaced with a notice, and safety-flagged requests can re-run on another model; in JSON output only `modelUsage` shows that switch (`model-config.md`, `errors.md`).

### Codex

| Mechanism | Syntax | Scope | Works for a Yoke-launched `codex exec --ignore-user-config` | Source |
|---|---|---|---|---|
| CLI flag | `-m`, `--model <MODEL>` | Run | Yes | `developer-commands?surface=cli`, `models.md` |
| Config override | `-c model="<id>"` | Run | Yes | `config-file/config-advanced.md` |
| User config | `model` in `~/.codex/config.toml` | Global | No: `--ignore-user-config` skips it | `config-file/config-basic.md`, `non-interactive-mode.md` |
| Project config, profiles | `.codex/config.toml`, `--profile` | Project, run | Unknown under `--ignore-user-config`; flags outrank both | `config-file/config-advanced.md` |
| Managed default | `requirements.toml` `[models.new_thread]` | Policy | Yes unless `--model` or `-c` overrides it | `config-file/config-reference.md` |
| Review model | `review_model` | `/review` only | Not documented for `exec` | `config-file/config-reference.md` |
| Subagent model | `-c agents.default_subagent_model=...` | Subagents | Yes | `agent-configuration/subagents.md` |
| Environment variable | none documented | - | - | `config-file/environment-variables.md` |
| Reported model | none: `exec --json` events carry no model field | - | - | `non-interactive-mode.md` |

Precedence documented in `config-file/config-basic.md`: CLI flags and `-c`, then project config, then profile, then user config, then managed and system defaults.
Behaviour for an unknown or unavailable model in `exec` is not documented.
No phase-specific model switch is documented; only `plan_mode_reasoning_effort` (effort, not model) and `review_model` (for `/review`).

### Changing the model of a running session from outside

Neither tool documents a way for an unrelated process to change the model of a user's interactive session.
Claude Code allows it only from the host that owns the session (Agent SDK `setModel()` in streaming-input mode, Remote Control, the Desktop app); `PreModelSwitch` hooks can only allow, deny or ask (`agent-sdk/typescript.md`, `remote-control.md`, `hooks.md`).
Codex allows it only for an app-server client through `turn/start` overrides (`app-server.md`).
Yoke installs hooks; it is not the session host, so it cannot use either path.

## Decision

1. Planning and building models stay native.
   Yoke displays where they are chosen and the models that transcripts report on the Usage page, and never claims to switch them.
   Phase routing such as `opusplan` remains a Claude Code feature the human selects in Claude Code.
2. Yoke owns only the processes it launches: the independent reviewer (`review.reviewer`) and the memory digest (`memory.digest`, `memory.digest_agent`), as `crates/yoke-core/src/config.rs` already defines.
3. The console gains a read-only Workflow settings page (`crates/yoke-cli/src/console/settings.rs`).
   It shows activation (observe or enforce), each Yoke-owned setting with its effective value and origin file, the trust state of every snapshot (trusted, stale, not trusted, not present), and the native choices.
   Editing stays a file change plus `yoke trust` in a human terminal, so executable configuration keeps its human approval.
4. Both CLIs document a model flag for the headless runs Yoke launches (`claude -p --model`, `codex exec --model`), so Yoke adds optional per-agent model fields for the reviewer and the digest.
   All three reviewer entry points use them: review inside `yoke plan check`, `yoke review`, and `yoke plan validate`; the digest uses its own field.

## Model fields

Shape, per agent so that `review.reviewer = "auto"` stays meaningful:

```toml
[review.models]
claude = "opus"
codex = "gpt-6.1-sol"

[memory.digest_models]
claude = "haiku"
```

Validation at parse time (`crates/yoke-core/src/config.rs`, `Config::validate`), and therefore before `yoke trust` can save a snapshot: a value must be 1 to 128 characters of `A-Z a-z 0-9 . _ : / @ [ ] -`, must not start with `-`, and the agent key must be `claude` or `codex`.
Model availability cannot be checked offline; Claude Code itself does not check it up front.
Execution (`crates/yoke-core/src/review.rs`, `command`) passes `--model <value>` to both CLIs, before Codex's `-` stdin marker, and never passes `--fallback-model`, so an unavailable model fails the review or digest with the native error instead of silently using another model.
An unset field keeps today's behaviour: no flag, the CLI's own default.
Changing a field changes executable configuration and therefore needs a fresh human `yoke trust`, like every other setting.

Evidence: a review check run records the reviewer agent and the requested model in its command, for example `independent review by codex (requested model: o3; reported model: unknown)` (`crates/yoke-core/src/plan/checks.rs`, `review_run`).
The model that actually ran is reported only by Claude Code JSON output (`modelUsage`); Yoke uses text output, and Codex reports no model, so the reported model stays "unknown".
Switching the Claude reviewer to `--output-format json` and reading `modelUsage` would fill it in for Claude only; that is a possible follow-up, not part of this change.

## Consequences

Users see one place that states what Yoke controls, what it does not, and why.
No unsupported model switching is promised.
A configured model is executable configuration, so it takes effect only after a human trusts the file.
A trusted snapshot that uses the new keys cannot be parsed by an older Yoke binary, so every Yoke executable that runs hooks must be updated before trusting them.
