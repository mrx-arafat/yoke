# Security policy

Yoke is pre-release software.
The 0.1.x line is the current development line and is not a stable release: no binary release has been published yet, and the real-work release trials are still open.
Only the `0.1.x` line receives security fixes at this time.
There is no support window for older releases, because none have been published.
Do not rely on Yoke 0.1.x for security-sensitive work until a stable release is published and its support window is stated here.

## Reporting a vulnerability

Please report security issues privately so that a fix can ship before the details become public.

Use GitHub private vulnerability reporting on this repository:

1. Open the repository's **Security** tab and choose **Report a vulnerability**.
2. Alternatively, open https://github.com/mrx-arafat/yoke/security/advisories/new directly.
3. Fill in the advisory form with the affected version, the steps to reproduce, and the impact.

Do not open a public issue, pull request, discussion, or commit for a security issue.
Do not email the maintainers; this repository does not publish an email address for security reports.

## Response expectations

The maintainer aims to acknowledge a report promptly, investigate it, and either publish a fix or explain why the behavior is expected.
This is a volunteer-maintained pre-release project with no staffed security team, so no specific response time is promised.
If a report is accepted, the maintainer will ask for a coordinated disclosure date and will credit reporters who want credit.
If a report is declined, the maintainer will say why.
Expect status updates only when there is something to report.

## In scope

The following are security issues we want reported:

- **Hook bypass of the self-protection guard.** A way for an agent to run a human-only command, write inside `~/.yoke`, or touch `state.db` while `enforce` mode should deny it, through a form the guard documents as recognized, such as quoting, wrappers, redirections or nested shells.
  The residual bypasses listed under Out of scope are known limits, not new reports.
- **Console loopback authentication.** A way to read or act on a console session from another process or browser tab, including the one-time code, the session secret, or the random per-run path that scopes the cookie.
- **Installer download verification.** A way to make `install.sh` install a tampered binary, or to bypass the SHA-256 checksum comparison that it performs on the downloaded release asset.
- **Redaction leaks.** A way to make a secret or private path appear in stored payloads, logs, notifications, diagnostics, or console output after redaction should have removed it.

## Out of scope

The following are documented, accepted limits rather than vulnerabilities.
Read [docs/workflows.md](docs/workflows.md), section **Security model and limits**, for the full statement.

Yoke's self-protection is explicitly not a sandbox.
As that section states: "Yoke's self-protection keeps agents from accidentally or casually unblocking themselves; it is not a sandbox."

Concretely, the following are out of scope:

- An agent that has a shell running as your OS user and deliberately works around the guard.
  It can hide a command behind a shell alias, a variable, a copied or renamed `yoke` binary, an encoded payload, or string concatenation inside an interpreter; remove the agent markers and fake a terminal; read a console link from a shared terminal or process arguments; open the database with its own tools; or replace the `yoke` binary or the hook settings.
- A same-user agent reading your browser profile files, including the console session cookie.
- Deliberate edits to evidence-comparison files that keep their size and restore their exact modification time, because files above 64 MiB are compared only by size and modification time to the nanosecond.
- An actor recorded as `human`, which means the action passed the human-terminal check, not that a person was proven to act.

If you need protection against a determined agent, use operating-system isolation as described in [docs/workflows.md](docs/workflows.md): run agents as a separate OS user, in a container, or in a virtual machine without access to `~/.yoke`, your browser profile, terminal multiplexer sockets, the `yoke` binary, or the agent hook settings, and prefer your agent's sandbox modes.

## Supported platforms

Yoke targets macOS and Linux on `x86_64` and `aarch64`.
Reports about other platforms are welcome, but a fix may wait until those platforms are officially supported.
