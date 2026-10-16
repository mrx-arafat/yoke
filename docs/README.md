# Yoke documentation

Yoke runs local policy checks in Claude Code and Codex hook paths.
It can record decisions, block supported actions, maintain plan evidence, and retain project memory.
It runs as a command rather than a background daemon; the optional console runs only while its command is active.

## Start here

| Guide | Use it to |
| --- | --- |
| [Getting started](getting-started.md) | Build, install, trust a policy, and inspect your first decision |
| [Configuration](configuration.md) | Understand defaults, layered trust, gates, and event behavior |
| [CLI reference](cli.md) | Find commands, arguments, and machine-readable output |
| [Workflows](workflows.md) | Use plans, independent review, memory, presets, and the console |
| [Troubleshooting](troubleshooting.md) | Diagnose inactive gates, failed checks, and storage problems |
| [Uninstalling](uninstalling.md) | Remove the hooks, the executable, and Yoke's files in the right order |
| [Releasing](releasing.md) | Take a release candidate to a published release, and withdraw a bad one |
| [Release acceptance](release-acceptance.md) | See the gate status and the evidence for builds, checksums, notices, install and update |
| [Development handoff](next-steps.md) | Resume from the hardening checkpoint and complete outstanding acceptance |
| [Upgrade notes](upgrade-notes.md) | See every behaviour change and upgrade step before replacing a binary |
| [Trials](trials.md) | Collect and record the observe-day and enforce-week trial evidence |
| [Console acceptance](console-acceptance.md) | Generate disposable fixtures and run the human console acceptance pass with an evidence template |
| [Contributing](../CONTRIBUTING.md) | Navigate the code and validate a focused change |
| [Security policy](../SECURITY.md) | Report a vulnerability privately and see which versions receive fixes |

## Mental model

1. You edit configuration on disk.
2. A human approves its exact bytes with `yoke trust`.
3. An agent invokes Yoke with a hook event and JSON payload.
4. Yoke evaluates the approved policy and records a decision.
5. Observe mode records potential denials; enforce mode can block supported events.

Trusting a configuration authorizes its shell commands to execute with your local permissions.
Observe mode still executes check commands, so review their side effects before trusting them.
Yoke is a policy tool, not an operating-system sandbox.

## Implementation and release status

The workspace package version is 0.1.0.
Implemented development features are not a promise that a published binary includes them.
See the root [status section](../README.md#status) and [execution record](specs/2026-10-13-core-execution.md) for release acceptance limitations.
The [release acceptance checklist](release-acceptance.md) records what has and has not been verified for a release.
The [design document](specs/2026-10-13-yoke-design.md) explains intent; these guides describe the current implementation.
