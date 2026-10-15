#!/bin/sh
# Human-only completion of the console acceptance fixtures made by make-fixtures.sh.
#
# Run this in your own interactive terminal, never from an agent. It asks you to type seven
# confirmation phrases: `trust` once, `approve <slug>` five times and `cancel <slug>` once, because
# those commands refuse agent processes and non-interactive input by design and nothing here
# bypasses that. Everything else it does is agent-safe and runs by itself: binding plans to
# recorded sessions, a failing check, an interrupted check, a question, a stall, a completed
# check with screenshot evidence, enforce-mode decisions for both agents, and an edit after trust
# so Trust & config shows a diff.
#
# Usage: sh scripts/acceptance/finish-fixtures.sh <review-home-printed-by-make-fixtures.sh>
# If it aborts midway, delete the review home and start again from make-fixtures.sh.
set -eu
umask 077

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd -P)
REVIEW_HOME=${1:?usage: finish-fixtures.sh <review-home>}
REAL_HOME=${HOME:-}
if [ -z "$REAL_HOME" ]; then
  printf 'HOME is empty; refusing to guess a home directory\n' >&2
  exit 1
fi
if [ "$REVIEW_HOME" = "$REAL_HOME" ] || [ ! -f "$REVIEW_HOME/fixtures.env" ] || [ ! -f "$REVIEW_HOME/.yoke/state.db" ]; then
  printf 'not a review home made by make-fixtures.sh: %s\n' "$REVIEW_HOME" >&2
  exit 1
fi
. "$REVIEW_HOME/fixtures.env"
export HOME="${REVIEW_HOME:?}"
unset CLAUDE_CONFIG_DIR CODEX_HOME YOKE_OFF XDG_CONFIG_HOME GIT_DIR GIT_WORK_TREE
export GIT_CONFIG_NOSYSTEM=1
export REAL_HOME YOKE_BIN ACME BETA
. "$SCRIPT_DIR/lib.sh"
guard
HOOK_LOG="$HOME/logs/hooks.log"
export HOOK_LOG
if [ "$(plan_state lockout-done)" != draft ]; then
  printf 'this review home was already finished or is damaged (lockout-done is "%s"); make a fresh one\n' "$(plan_state lockout-done)" >&2
  exit 1
fi

step() { printf '\n==> %s\n' "$1"; }
prompt_hint() { printf '    (when asked, type exactly: %s)\n' "$1"; }

step 'Trust the fixture policy (1 of 7 prompts)'
prompt_hint trust
(cd "$ACME" && y trust)
if ! (cd "$ACME" && y config) | grep -F "\"$ACME/yoke.toml\",true,false,\"\"" >/dev/null; then
  printf 'the fixture policy is not trusted; nothing else can proceed\n' >&2
  exit 1
fi

for slug in lockout-done rate-limit-building retry-policy-checking auth-header-waiting pagination-stalled; do
  step "Approve plan $slug"
  prompt_hint "approve $slug"
  (cd "$ACME" && y plan approve "$slug")
  expect_state "$slug" approved
done

step 'Cancel the draft plan legacy-cleanup-cancelled (last prompt)'
prompt_hint 'cancel legacy-cleanup-cancelled'
(cd "$ACME" && y plan cancel legacy-cleanup-cancelled --reason 'Superseded by the onboarding plan; cancelled from the fixture run sheet.')
expect_state legacy-cleanup-cancelled cancelled

step 'Bind approved plans to recorded sessions'
(
  cd "$ACME"
  y plan start lockout-done --session "$A1" >>"$HOME/logs/setup.log"
  y plan start rate-limit-building --session "$C1" >>"$HOME/logs/setup.log"
  y plan start retry-policy-checking --session "$C2" >>"$HOME/logs/setup.log"
  y plan start auth-header-waiting --session "$A3" >>"$HOME/logs/setup.log"
  y plan start pagination-stalled --session "$A2" >>"$HOME/logs/setup.log"
)
for slug in lockout-done rate-limit-building retry-policy-checking auth-header-waiting pagination-stalled; do
  expect_state "$slug" building
done

step 'Complete lockout-done with screenshot evidence'
(
  cd "$ACME"
  y plan task done lockout-done capture --evidence 'Captured shots/lockout.png from the documentation assets.' >>"$HOME/logs/setup.log"
  y plan check lockout-done --agent claude >>"$HOME/logs/setup.log"
)
expect_state lockout-done done

step 'Record a failing check on rate-limit-building'
(cd "$ACME" && y plan check rate-limit-building --agent codex >>"$HOME/logs/setup.log" 2>&1) || true
expect_state rate-limit-building building
if ! (cd "$ACME" && y plan status rate-limit-building --json) | grep -F '"result":"fail"' >/dev/null; then
  printf 'expected a failed check run on rate-limit-building\n' >&2
  exit 1
fi

step 'Put auth-header-waiting into waiting'
(cd "$ACME" && y plan ask 'Which auth header should the retry keep: the original or the refreshed one?' --slug auth-header-waiting >>"$HOME/logs/setup.log")
expect_state auth-header-waiting waiting

step 'Stall pagination-stalled with three Stop events and no progress'
for n in 1 2 3; do
  hook claude "$A2" "$ACME" Stop '"stop_hook_active":false,"last_assistant_message":"Fixture stop without progress.","background_tasks":[],"session_crons":[]'
done
expect_state pagination-stalled stalled

step 'Interrupt the check on retry-policy-checking'
rm -f "$HOME/fixture-check.pid"
# exec makes $! the yoke process itself, so the kill below reaches the check owner.
(cd "$ACME" && guard && exec "$YOKE_BIN" plan check retry-policy-checking --agent codex >>"$HOME/logs/setup.log" 2>&1) &
check_owner=$!
tries=0
while [ "$(plan_state retry-policy-checking)" != checking ]; do
  tries=$((tries + 1))
  if [ $tries -gt 30 ] || ! kill -0 "$check_owner" 2>/dev/null; then
    printf 'the check never reached the checking state; see %s/logs/setup.log\n' "$HOME" >&2
    exit 1
  fi
  sleep 1
done
kill -9 "$check_owner" 2>/dev/null || true
wait "$check_owner" 2>/dev/null || true
if [ -f "$HOME/fixture-check.pid" ]; then
  kill -9 "$(cat "$HOME/fixture-check.pid")" 2>/dev/null || true
  rm -f "$HOME/fixture-check.pid"
fi
expect_state retry-policy-checking checking

step 'Record enforce-mode decisions for both agents'
# Denied by the fixture gates, denied by self-protection, rewritten, and allowed.
for pair in "claude $C3" "codex $C3"; do
  set -- $pair
  hook "$1" "$2" "$ACME" PreToolUse '"tool_name":"Bash","tool_input":{"command":"git push --force origin main"},"tool_use_id":"enforce-1-'"$1"'"'
  hook "$1" "$2" "$ACME" PreToolUse '"tool_name":"Bash","tool_input":{"command":"git reset --hard HEAD~1"},"tool_use_id":"enforce-2-'"$1"'"'
  hook "$1" "$2" "$ACME" PreToolUse '"tool_name":"Bash","tool_input":{"command":"sqlite3 ~/.yoke/state.db .tables"},"tool_use_id":"enforce-3-'"$1"'"'
  hook "$1" "$2" "$ACME" PreToolUse '"tool_name":"Bash","tool_input":{"command":"cargo test -p acme-api"},"tool_use_id":"enforce-4-'"$1"'"'
  hook "$1" "$2" "$ACME" PreToolUse '"tool_name":"Bash","tool_input":{"command":"cargo build"},"tool_use_id":"enforce-5-'"$1"'"'
done
hook codex "$C3" "$ACME" SessionEnd '"reason":"other"'

step 'Change trusted files on disk so Trust & config shows diffs'
printf '\n# Edited after trust by finish-fixtures.sh; Trust & config must show this line as a diff.\n' >>"$ACME/yoke.toml"
shared_id=$(cd "$ACME" && y recall share-me --json | sed -n 's/^\[{"id":\([0-9]*\).*/\1/p')
if [ -n "$shared_id" ]; then
  (cd "$ACME" && y share "$shared_id" >>"$HOME/logs/setup.log")
fi

step 'Sync notifications and summarize'
(cd "$ACME" && y notifications) | sed 's/^/  /'
printf '  plans\n'
(cd "$ACME" && y plan list) | sed 's/^/    /'
decisions=$(cd "$ACME" && y log --limit 1000 --json)
for outcome in allow would_deny deny rewrite error; do
  count=$(printf '%s' "$decisions" | grep -o "\"outcome\":\"$outcome\"" | wc -l | tr -d ' ')
  printf '  decisions %-10s %s\n' "$outcome" "$count"
done

cat <<EOF

Fixtures complete. Launch the console from the launch project in this terminal:
  cd "$ACME" && HOME="$HOME" "$YOKE_BIN" console --no-open
Open the printed one-time link within five minutes, follow docs/console-acceptance.md, and stop
the console with Ctrl-C. Tear down afterwards with: rm -rf "$HOME"
EOF
