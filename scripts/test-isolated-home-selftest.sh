#!/bin/sh
# Verify isolation, toolchain resolution and cleanup without building or invoking Yoke.
set -eu
runner=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)/test-isolated-home.sh
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT HUP INT TERM
export ISOLATION_CAPTURE="$scratch/home"
export CLAUDE_CONFIG_DIR="$scratch/real-claude"
export CODEX_HOME="$scratch/real-codex"
export ISOLATION_ORIGINAL_HOME="$HOME"
export EXPECTED_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export EXPECTED_RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
sh "$runner" sh -eu -c '
    test "$HOME" != "$ISOLATION_ORIGINAL_HOME"
    test -d "$HOME"
    test "${CLAUDE_CONFIG_DIR+x}" != x
    test "${CODEX_HOME+x}" != x
    test -x "$YOKE_CURL"
    test -d "$CARGO_HOME"
    test -d "$RUSTUP_HOME"
    test "$CARGO_HOME" = "$EXPECTED_CARGO_HOME"
    test "$RUSTUP_HOME" = "$EXPECTED_RUSTUP_HOME"
    cargo --version > /dev/null
    printf "%s" "$HOME" > "$ISOLATION_CAPTURE"
    if "$YOKE_CURL" https://example.invalid; then exit 1; fi
'
isolated=$(cat "$ISOLATION_CAPTURE")
test ! -e "$isolated"
set +e
sh "$runner" sh -c 'printf "%s" "$HOME" > "$ISOLATION_CAPTURE"; exit 23'
result=$?
set -e
test "$result" -eq 23
test ! -e "$(cat "$ISOLATION_CAPTURE")"
set +e
sh "$runner" sh -c '
    # A long-lived child proves forwarding; sleep models work, not synchronization.
    trap '\''kill "$sleeper" 2>/dev/null || :; wait "$sleeper" 2>/dev/null || :; printf forwarded > "$ISOLATION_CAPTURE.signal"; exit 0'\'' TERM
    sleep 30 &
    sleeper=$!
    printf "%s" "$HOME" > "$ISOLATION_CAPTURE"
    kill -TERM "$PPID"
    wait "$sleeper"
'
result=$?
set -e
test "$result" -eq 143
test "$(cat "$ISOLATION_CAPTURE.signal")" = forwarded
test ! -e "$(cat "$ISOLATION_CAPTURE")"
printf '%s\n' 'isolated-home selftest: 3 passed, 0 failed'
