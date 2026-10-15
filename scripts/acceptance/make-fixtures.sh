#!/bin/sh
# Build disposable console acceptance fixtures in a fresh review home.
#
# Creates a temporary HOME with two throwaway Git projects, recorded Claude Code and Codex
# sessions, observe-mode decisions, usage samples, memories, notifications and seven draft plans,
# using only supported commands and hook payloads. It never touches the real home, never runs
# `yoke trust`, `yoke console` or any other human-only command, and never starts a background
# process. The human-only completion is `finish-fixtures.sh`, which this script names at the end.
#
# Usage: sh scripts/acceptance/make-fixtures.sh [--copy-live-db] [--bin <path-to-yoke>]
#   --copy-live-db  seed the review home from a read-only SQLite backup of the live database,
#                   exactly as docs/next-steps.md describes; the copy is migrated, the live file is not
#   --bin PATH      yoke executable to use (default: target/debug/yoke; YOKE_BIN also works)
set -eu
umask 077

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd -P)
REPO=$(cd "$SCRIPT_DIR/../.." && pwd -P)
COPY_LIVE=0
YOKE_BIN=${YOKE_BIN:-$REPO/target/debug/yoke}
while [ $# -gt 0 ]; do
  case $1 in
    --copy-live-db) COPY_LIVE=1 ;;
    --bin) shift; YOKE_BIN=${1:?--bin needs a path} ;;
    -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; exit 2 ;;
  esac
  shift
done

REAL_HOME=${HOME:-}
if [ -z "$REAL_HOME" ]; then
  printf 'HOME is empty; refusing to guess a home directory\n' >&2
  exit 1
fi
case $YOKE_BIN in
  */target/release/*) printf 'refusing to use a release build that live hooks may execute: %s\n' "$YOKE_BIN" >&2; exit 1 ;;
esac
if [ ! -x "$YOKE_BIN" ]; then
  printf 'yoke executable not found: %s\nBuild it first: cargo build --locked -q -p yoke-cli\n' "$YOKE_BIN" >&2
  exit 1
fi
for tool in git date; do
  command -v "$tool" >/dev/null 2>&1 || { printf 'missing required tool: %s\n' "$tool" >&2; exit 1; }
done
ASSET="$REPO/docs/assets/console-plan-dark.png"
if [ ! -f "$ASSET" ]; then
  printf 'missing screenshot asset used as plan evidence: %s\n' "$ASSET" >&2
  exit 1
fi

# Isolated review home. Every later command asserts it before touching anything.
REVIEW_HOME=$(mktemp -d)
export HOME="${REVIEW_HOME:?}"
unset CLAUDE_CONFIG_DIR CODEX_HOME YOKE_OFF XDG_CONFIG_HOME GIT_DIR GIT_WORK_TREE
export GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=yoke-fixture GIT_AUTHOR_EMAIL=fixture@localhost
export GIT_COMMITTER_NAME=yoke-fixture GIT_COMMITTER_EMAIL=fixture@localhost
export REAL_HOME YOKE_BIN
. "$SCRIPT_DIR/lib.sh"
guard
mkdir -p "$HOME/.yoke" "$HOME/transcripts" "$HOME/logs"
HOOK_LOG="$HOME/logs/hooks.log"
SETUP_LOG="$HOME/logs/setup.log"
export HOOK_LOG
: >"$HOOK_LOG"
: >"$SETUP_LOG"
printf 'Review home: %s\n' "$HOME"

if [ "$COPY_LIVE" = 1 ]; then
  command -v sqlite3 >/dev/null 2>&1 || { printf 'sqlite3 is required for --copy-live-db\n' >&2; exit 1; }
  if [ -f "$REAL_HOME/.yoke/state.db" ]; then
    sqlite3 -readonly "$REAL_HOME/.yoke/state.db" ".backup '$HOME/.yoke/state.db'"
    printf 'Copied the live database read-only into the review home; the copy will be migrated, the live file stays untouched.\n'
  else
    printf 'No live database at %s/.yoke/state.db; continuing with an empty review home.\n' "$REAL_HOME"
  fi
fi

# Two projects: the launch project sits under a deliberately long path (about 220 characters)
# so every page that prints the project path exercises overflow handling.
LONG1=long-directory-name-for-overflow-checks-00000001
LONG2=long-directory-name-for-overflow-checks-00000002
LONG3=long-directory-name-for-overflow-checks-00000003
mkdir -p "$HOME/projects/$LONG1/$LONG2/$LONG3/acme-api" "$HOME/projects/beta-cli"
ACME=$(cd "$HOME/projects/$LONG1/$LONG2/$LONG3/acme-api" && pwd -P)
BETA=$(cd "$HOME/projects/beta-cli" && pwd -P)
export ACME BETA

init_project() {
  (
    cd "$1"
    git -c init.defaultBranch=main init -q .
    printf '# %s\n\nDisposable console acceptance fixture. Delete the whole review home after the pass.\n' "$2" >README.md
    mkdir -p src
    printf 'pub fn fixture() -> &%sstatic str {\n    "%s"\n}\n' "'" "$2" >src/lib.rs
    git add -A
    git commit -q -m "fixture: initial $2"
  )
}
init_project "$ACME" acme-api
init_project "$BETA" beta-cli

# Launch project policy: enforce mode once a human trusts it, two Stop blocks before a stall,
# two deny gates and one rewrite gate. Nothing here executes a shell command.
cat >"$ACME/yoke.toml" <<'EOF'
# Console acceptance fixture: disposable policy for a throwaway project.
# Review it with `yoke trust` in your own terminal before it runs.
mode = "enforce"

[meta]
name = "acme-api"

[stop]
max_blocks = 2

[[gate]]
name = "fixture-no-force-push"
on = "pre_tool_use"
action = "deny"
match_tool = "Bash|exec_command|shell_command|shell"
match_contains = "push --force"
message = "Fixture policy: force pushes are denied. Push without --force."

[[gate]]
name = "fixture-no-hard-reset"
on = "pre_tool_use"
action = "deny"
match_tool = "Bash|exec_command|shell_command|shell"
match_contains = "reset --hard"
message = "Fixture policy: hard resets are denied."

[[gate]]
name = "fixture-quiet-tests"
on = "pre_tool_use"
action = "rewrite"
match_tool = "Bash|exec_command|shell_command|shell"
rewrite_field = "command"
rewrite_pattern = '^cargo test(?:[ \t].*)?$'
rewrite_to = "$0 --quiet"
message = "Fixture policy: cargo test runs with --quiet."
EOF
# The second project keeps the generated starter file, which stays untrusted and in observe mode.
(cd "$BETA" && y init >>"$SETUP_LOG")

# Seven plans, one per console state. All start as drafts; finish-fixtures.sh moves five of them.
new_plan() {
  (cd "$ACME" && y plan new "$1" --title "$2" >>"$SETUP_LOG")
}
new_plan onboarding-draft 'Onboarding flow with requirements, tasks and open feedback'
new_plan rate-limit-building 'Rate limiter with a failing check'
new_plan retry-policy-checking 'Retry policy with an interrupted check'
new_plan auth-header-waiting 'Auth header plan that asked a question'
new_plan pagination-stalled 'Pagination plan that stalled on Stop'
new_plan lockout-done 'Lockout screen completed with screenshot evidence'
new_plan legacy-cleanup-cancelled 'Legacy cleanup cancelled before approval'

PLANS="$ACME/.yoke/plans"
LONG_TOKEN=unbroken-identifier-for-wrap-checks-aaaaaaaaaabbbbbbbbbbccccccccccddddddddddeeeeeeeeeeffffffffffgggggggggghhhhhhhhhhiiiiiiiiiijjjjjjjjjjkkkkkkkkkkllllllllllmmmmmmmmmmnnnnnnnnnnoooooooooopppppppppp

cat >"$PLANS/onboarding-draft.md" <<'EOF'
+++
title = "Onboarding flow with requirements, tasks and open feedback"
review = false

[[check]]
name = "lint"
run = "true"
blocking = true
timeout = 30

[[requirement]]
id = "lint-clean"
text = "The onboarding module passes lint"
checks = ["lint"]

[[requirement]]
id = "copy-reviewed"
text = "A human has read the onboarding copy"
human = true

[[task]]
id = "write-form"
title = "Write the onboarding form"
requirements = ["lint-clean"]

[[task]]
id = "review-copy"
title = "Review the onboarding copy"
requirements = ["copy-reviewed"]
depends_on = ["write-form"]
owner = "human"
+++

## Goal

Pending approval: this plan stays a draft so the console can show **Review approval**.

## Scope

- One form with three fields
- One human-reviewed copy deck
- `onboarding::submit` returns a typed error

```rust
pub fn submit(form: Form) -> Result<Account, OnboardingError> {
    form.validate()?;
    Ok(Account::new(form))
}
```
EOF

cat >"$PLANS/rate-limit-building.md" <<'EOF'
+++
title = "Rate limiter with a failing check"
review = false

[[check]]
name = "limiter-tests"
run = "printf 'simulated failure: the fixture limiter returned 429 for an allowed tenant\n' >&2; exit 1"
blocking = true
timeout = 30
+++

## Goal

Building: the only check fails on purpose so the console shows a failed run and its output tail.
EOF

cat >"$PLANS/retry-policy-checking.md" <<'EOF'
+++
title = "Retry policy with an interrupted check"
review = false

[[check]]
name = "slow-integration"
run = 'printf %s "$$" > "$HOME/fixture-check.pid"; exec sleep 25'
blocking = true
timeout = 60
+++

## Goal

Checking: finish-fixtures.sh kills the check owner while this runs, so the plan stays `checking`
and the console offers **Recover interrupted check**.
EOF

cat >"$PLANS/auth-header-waiting.md" <<'EOF'
+++
title = "Auth header plan that asked a question"
review = false

[[check]]
name = "unit"
run = "true"
blocking = true
timeout = 30
+++

## Goal

Waiting: the agent asked which header the retry should keep, so the console offers **Resume work**.
EOF

cat >"$PLANS/pagination-stalled.md" <<'EOF'
+++
title = "Pagination plan that stalled on Stop"
review = false

[[check]]
name = "unit"
run = "true"
blocking = true
timeout = 30
+++

## Goal

Stalled: three Stop events without progress exceed `stop.max_blocks = 2`, so the console offers **Resume work**.
EOF

cat >"$PLANS/lockout-done.md" <<'EOF'
+++
title = "Lockout screen completed with screenshot evidence"
review = false

[[check]]
name = "lockout"
run = "test -f shots/lockout.png"
blocking = true
timeout = 30
artifacts = ["shots/lockout.png"]

[[requirement]]
id = "lockout-verified"
text = "The lockout screenshot exists and is captured as evidence"
checks = ["lockout"]

[[task]]
id = "capture"
title = "Capture the lockout screenshot"
requirements = ["lockout-verified"]
+++

## Goal

Done: the check captures `shots/lockout.png`, so the console shows a verified screenshot in the lightbox.
EOF

cat >"$PLANS/legacy-cleanup-cancelled.md" <<'EOF'
+++
title = "Legacy cleanup cancelled before approval"
review = false

[[check]]
name = "unit"
run = "true"
blocking = true
timeout = 30
+++

## Goal

Cancelled: a human cancels this draft with a reason, so the console shows the cancelled badge and transition.
EOF
printf '\n## Long unbroken text\n\n%s\n' "$LONG_TOKEN" >>"$PLANS/onboarding-draft.md"

# A plan comment and reply on the draft, both visible under feedback and during approval.
(
  cd "$ACME"
  comment=$(y plan comment onboarding-draft --anchor requirement:copy-reviewed --text 'Who reviews the copy before the first release?')
  printf '%s\n' "$comment" >>"$SETUP_LOG"
  comment_id=$(printf '%s' "$comment" | sed -n 's/^{"id":\([0-9]*\).*/\1/p')
  y plan reply onboarding-draft "${comment_id:?comment id missing}" --text 'The product owner; recorded here so approval shows open feedback.' >>"$SETUP_LOG"
)

# Screenshot evidence for the done plan, taken from the documentation assets.
mkdir -p "$ACME/shots"
cp "$ASSET" "$ACME/shots/lockout.png"
(cd "$ACME" && git add -A && git commit -q -m 'fixture: plans, policy and evidence')

# Memories: pinned, markdown, long unbroken text, plain; one in the second project.
(
  cd "$ACME"
  y remember 'Rate limiter buckets are keyed by tenant id, never by IP, because the gateway rewrites every source address.' --pinned >>"$SETUP_LOG"
  y remember '**Retry policy** (share-me): retry only idempotent calls.
- three attempts
- exponential backoff
- `Retry-After` wins over the local schedule' >>"$SETUP_LOG"
  y remember "Long unbroken token for wrap checks: $LONG_TOKEN" >>"$SETUP_LOG"
  y remember 'Lockout screenshots live under shots/ and are hashed when the plan check captures them.' >>"$SETUP_LOG"
)
(cd "$BETA" && y remember 'beta-cli reads YOKE_PROJECT only inside gate commands.' >>"$SETUP_LOG")

# Sessions. Claude ids end in 1xx, Codex ids end in 2xx; finish-fixtures.sh binds plans to them.
A1=a1a1a1a1-1111-4111-8111-000000000101
A2=a1a1a1a1-1111-4111-8111-000000000102
A3=a1a1a1a1-1111-4111-8111-000000000103
C1=c0c0c0c0-2222-4222-8222-000000000201
C2=c0c0c0c0-2222-4222-8222-000000000202
C3=c0c0c0c0-2222-4222-8222-000000000203
B1=b1b1b1b1-3333-4333-8333-000000000301
B2=b2b2b2b2-3333-4333-8333-000000000302
DAY1=$(date -u +%Y-%m-%d)
DAY0=$(date -u +%Y-%m-01)

# Transcripts carry only accounting fields. Claude samples include two priced models and one
# unknown model; Codex totals are cumulative and keep cost unknown by design.
claude_transcript() {
  claude_file="$HOME/transcripts/$1.jsonl"
  printf '%s\n' \
    "{\"type\":\"assistant\",\"sessionId\":\"$1\",\"timestamp\":\"${DAY0}T09:12:00Z\",\"message\":{\"id\":\"msg-$1-1\",\"model\":\"claude-sonnet-4-6\",\"usage\":{\"input_tokens\":18400,\"output_tokens\":2100,\"cache_read_input_tokens\":6000,\"cache_creation_input_tokens\":0,\"service_tier\":\"standard\",\"inference_geo\":\"global\",\"speed\":\"standard\"}}}" \
    "{\"type\":\"assistant\",\"sessionId\":\"$1\",\"timestamp\":\"${DAY1}T10:30:00Z\",\"message\":{\"id\":\"msg-$1-2\",\"model\":\"claude-opus-5-5\",\"usage\":{\"input_tokens\":42000,\"output_tokens\":3600,\"cache_read_input_tokens\":30000,\"cache_creation_input_tokens\":0,\"service_tier\":\"standard\",\"inference_geo\":\"global\",\"speed\":\"standard\"}}}" \
    "{\"type\":\"assistant\",\"sessionId\":\"$1\",\"timestamp\":\"${DAY1}T11:05:00Z\",\"message\":{\"id\":\"msg-$1-3\",\"model\":\"fixture-unlisted-model\",\"usage\":{\"input_tokens\":900,\"output_tokens\":120}}}" \
    >"$claude_file"
}
codex_transcript() {
  codex_file="$HOME/transcripts/$1.jsonl"
  printf '%s\n' \
    "{\"type\":\"session_meta\",\"timestamp\":\"${DAY0}T14:00:00Z\",\"payload\":{\"id\":\"$1\"}}" \
    "{\"type\":\"turn_context\",\"timestamp\":\"${DAY0}T14:00:01Z\",\"payload\":{\"model\":\"gpt-5-codex\"}}" \
    "{\"type\":\"event_msg\",\"timestamp\":\"${DAY0}T14:20:00Z\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":8500,\"cached_input_tokens\":1200,\"cache_write_input_tokens\":0,\"output_tokens\":900}}}}" \
    "{\"type\":\"event_msg\",\"timestamp\":\"${DAY1}T16:45:00Z\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":31000,\"cached_input_tokens\":9000,\"cache_write_input_tokens\":0,\"output_tokens\":4100}}}}" \
    >"$codex_file"
}

LONG_COMMAND="./scripts/deploy --environment=staging --release-tag=$LONG_TOKEN --confirm"

# One session's observe-mode story: start, prompt, allowed tool, self-protection would-deny,
# a long unbroken command, an edit, then Stop, which imports the transcript.
story() {
  story_agent=$1
  story_session=$2
  story_project=$3
  if [ "$story_agent" = claude ]; then
    claude_transcript "$story_session"
    story_edit='"tool_name":"Edit","tool_input":{"file_path":"'"$story_project"'/src/lib.rs","old_string":"fixture()","new_string":"fixture_v2()"},"tool_use_id":"edit-'"$story_session"'"'
    story_edit_done='"tool_name":"Edit","tool_input":{"file_path":"'"$story_project"'/src/lib.rs","old_string":"fixture()","new_string":"fixture_v2()"},"tool_response":{"filePath":"'"$story_project"'/src/lib.rs"},"tool_use_id":"edit-'"$story_session"'","duration_ms":12'
    story_stop='"stop_hook_active":false,"last_assistant_message":"Done with this step.","background_tasks":[],"session_crons":[]'
  else
    codex_transcript "$story_session"
    story_edit='"tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-pub fn fixture()\n+pub fn fixture_v2()\n*** End Patch\n"},"tool_use_id":"patch-'"$story_session"'"'
    story_edit_done='"tool_name":"apply_patch","tool_input":{"command":"*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-pub fn fixture()\n+pub fn fixture_v2()\n*** End Patch\n"},"tool_response":"Success. Updated the following files:\nM src/lib.rs\n","tool_use_id":"patch-'"$story_session"'"'
    story_stop='"stop_hook_active":false,"last_assistant_message":"Done with this step."'
  fi
  hook "$story_agent" "$story_session" "$story_project" SessionStart '"source":"startup"'
  hook "$story_agent" "$story_session" "$story_project" UserPromptSubmit '"prompt":"Continue the fixture task and keep the tests green."'
  hook "$story_agent" "$story_session" "$story_project" PreToolUse '"tool_name":"Bash","tool_input":{"command":"git status --short"},"tool_use_id":"bash-1-'"$story_session"'"'
  hook "$story_agent" "$story_session" "$story_project" PostToolUse '"tool_name":"Bash","tool_input":{"command":"git status --short"},"tool_response":"","tool_use_id":"bash-1-'"$story_session"'"'
  hook "$story_agent" "$story_session" "$story_project" PreToolUse '"tool_name":"Bash","tool_input":{"command":"sqlite3 ~/.yoke/state.db .tables"},"tool_use_id":"bash-2-'"$story_session"'"'
  hook "$story_agent" "$story_session" "$story_project" PreToolUse '"tool_name":"Bash","tool_input":{"command":"'"$LONG_COMMAND"'"},"tool_use_id":"bash-3-'"$story_session"'"'
  hook "$story_agent" "$story_session" "$story_project" PostToolUse '"tool_name":"Bash","tool_input":{"command":"'"$LONG_COMMAND"'"},"tool_response":"deployed","tool_use_id":"bash-3-'"$story_session"'"'
  hook "$story_agent" "$story_session" "$story_project" PreToolUse "$story_edit"
  hook "$story_agent" "$story_session" "$story_project" PostToolUse "$story_edit_done"
  hook "$story_agent" "$story_session" "$story_project" Stop "$story_stop"
}

printf 'Recording sessions and observe-mode decisions...\n'
story claude "$A1" "$ACME"
story claude "$A2" "$ACME"
story claude "$A3" "$ACME"
story codex "$C1" "$ACME"
story codex "$C2" "$ACME"
story codex "$C3" "$ACME"
story claude "$B1" "$BETA"
story codex "$B2" "$BETA"
# Extra read traffic on one session so the Decisions page exceeds its 50-row page size.
for n in 1 2 3 4 5 6 7 8; do
  hook claude "$A1" "$ACME" PreToolUse '"tool_name":"Read","tool_input":{"file_path":"'"$ACME"'/src/lib.rs"},"tool_use_id":"read-'"$n"'"'
  hook claude "$A1" "$ACME" PostToolUse '"tool_name":"Read","tool_input":{"file_path":"'"$ACME"'/src/lib.rs"},"tool_response":{"filePath":"'"$ACME"'/src/lib.rs","numLines":3},"tool_use_id":"read-'"$n"'","duration_ms":3'
done
# Lifecycle variety: a Claude notification, a compaction, and some sessions that ended.
hook claude "$A3" "$ACME" Notification '"message":"Claude is waiting for your input","notification_type":"idle_prompt"'
hook codex "$C2" "$ACME" PreCompact '"trigger":"auto"'
hook claude "$A1" "$ACME" SessionEnd '"reason":"other"'
hook codex "$C1" "$ACME" SessionEnd '"reason":"other"'
hook codex "$B2" "$BETA" SessionEnd '"reason":"other"'

# An untrusted file is never parsed for diagnostics, so the fixture policy above keeps to the
# documented fields that `yoke init` and the bundled presets emit; `yoke trust` validates it.

cat >"$HOME/fixtures.env" <<EOF
REVIEW_HOME='$HOME'
YOKE_BIN='$YOKE_BIN'
REPO='$REPO'
ACME='$ACME'
BETA='$BETA'
A1='$A1'
A2='$A2'
A3='$A3'
C1='$C1'
C2='$C2'
C3='$C3'
B1='$B1'
B2='$B2'
EOF

printf '\nFixture summary\n'
printf '  launch project   %s\n' "$ACME"
printf '  second project   %s\n' "$BETA"
printf '  hook payloads    %s (log: %s)\n' "$HOOK_COUNT" "$HOOK_LOG"
decisions=$(cd "$ACME" && y log --limit 1000 --json)
for outcome in allow would_deny deny rewrite error; do
  count=$(printf '%s' "$decisions" | grep -o "\"outcome\":\"$outcome\"" | wc -l | tr -d ' ')
  printf '  decisions %-10s %s\n' "$outcome" "$count"
done
printf '  plans\n'
(cd "$ACME" && y plan list) | sed 's/^/    /'
printf '  notifications\n'
(cd "$ACME" && y notifications) | sed 's/^/    /'

cat <<EOF

Human step (the only one; agents cannot do it):
  sh "$REPO/scripts/acceptance/finish-fixtures.sh" "$HOME"
Why: yoke trust, yoke plan approve and yoke plan cancel refuse agent processes and non-interactive
input by design, and every plan state beyond draft, every enforce-mode decision and the stale-trust
diff depend on them. The script asks you to type 7 confirmation phrases, then finishes the rest.

Afterwards, launch the console from the launch project in your own terminal:
  cd "$ACME" && HOME="$HOME" "$YOKE_BIN" console --no-open

Tear down when finished (nothing outside this directory was written):
  rm -rf "$HOME"
EOF
