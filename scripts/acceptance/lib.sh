# Shared helpers for the console acceptance fixture scripts. POSIX sh; sourced, never executed.
# Every helper refuses to run against an empty or real HOME, so a bug cannot reach live Yoke state.

# guard: abort unless HOME is a non-empty, existing directory that differs from the invoking user's home.
guard() {
  test -n "${HOME:-}" && test "$HOME" != "${REAL_HOME:?REAL_HOME must be set before guard}" \
    && test "$HOME" != "/Users/easinarafat" && test -d "$HOME" || {
    printf 'refusing to run: HOME=%s is empty, missing, or the real home\n' "${HOME:-}" >&2
    exit 1
  }
}

# y: run the yoke binary under the guarded HOME.
y() {
  guard
  "${YOKE_BIN:?YOKE_BIN must be set}" "$@"
}

# hook AGENT SESSION PROJECT EVENT [EXTRA_JSON_FIELDS]
# Sends one hook payload shaped like the public Claude Code or Codex contract to `yoke run-hook`.
# EXTRA_JSON_FIELDS is a comma-joined list of already-quoted JSON members without surrounding braces.
# Output goes to $HOOK_LOG; a nonzero exit is reported but does not abort, because enforce-mode
# denials and Stop blocks are expected outcomes.
HOOK_COUNT=0
hook() {
  hook_agent=$1
  hook_session=$2
  hook_project=$3
  hook_event=$4
  hook_extra=${5:-}
  hook_transcript="$HOME/transcripts/$hook_session.jsonl"
  hook_json="{\"session_id\":\"$hook_session\",\"transcript_path\":\"$hook_transcript\",\"cwd\":\"$hook_project\",\"hook_event_name\":\"$hook_event\""
  HOOK_COUNT=$((HOOK_COUNT + 1))
  if [ "$hook_agent" = codex ]; then
    [ "$hook_event" = SessionEnd ] || hook_json="$hook_json,\"model\":\"gpt-5-codex\""
    case $hook_event in
      UserPromptSubmit|PreToolUse|PostToolUse|Stop|PreCompact) hook_json="$hook_json,\"turn_id\":\"$hook_session-turn-$HOOK_COUNT\"" ;;
    esac
    case $hook_event in
      SessionStart|UserPromptSubmit|PreToolUse|PostToolUse|Stop) hook_json="$hook_json,\"permission_mode\":\"default\"" ;;
    esac
  else
    case $hook_event in
      SessionStart|SessionEnd|PreCompact|Notification) ;;
      *) hook_json="$hook_json,\"permission_mode\":\"default\"" ;;
    esac
  fi
  [ -n "$hook_extra" ] && hook_json="$hook_json,$hook_extra"
  hook_json="$hook_json}"
  printf '\n### %s %s %s\n' "$hook_agent" "$hook_event" "$hook_session" >>"${HOOK_LOG:?HOOK_LOG must be set}"
  (cd "$hook_project" && printf '%s' "$hook_json" | y run-hook "$hook_event" --agent "$hook_agent" >>"$HOOK_LOG" 2>&1) \
    || printf 'hook %s %s for %s exited %s (expected for enforce denials; see %s)\n' "$hook_agent" "$hook_event" "$hook_session" "$?" "$HOOK_LOG"
}

# plan_state SLUG: print the current state of one plan in $ACME, or nothing if it does not exist.
plan_state() {
  (cd "${ACME:?ACME must be set}" && y plan status --json) \
    | sed -n 's/.*"slug":"'"$1"'","file":"[^"]*","state":"\([a-z]*\)".*/\1/p'
}

# expect_state SLUG STATE: abort with a clear message when a plan is not in the expected state.
expect_state() {
  expect_actual=$(plan_state "$1")
  if [ "$expect_actual" != "$2" ]; then
    printf 'plan %s is "%s", expected "%s"; see %s and %s/logs\n' "$1" "$expect_actual" "$2" "$HOOK_LOG" "$HOME" >&2
    exit 1
  fi
  printf '  plan %-26s %s\n' "$1" "$2"
}
