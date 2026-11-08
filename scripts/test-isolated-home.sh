#!/bin/sh
# Run tests with disposable state while retaining the installed Rust toolchain.
set -eu
if [ "$#" -eq 0 ]; then
    printf '%s\n' 'usage: sh scripts/test-isolated-home.sh <command> [args...]' >&2
    exit 2
fi
: "${HOME:?HOME must name the existing toolchain home}"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
original_home=$HOME
# Homebrew's rustup keeps its proxies outside CARGO_HOME, so a fresh machine may not have
# created it yet; cargo would create it anyway. Only the toolchain directory is required.
mkdir -p "$CARGO_HOME"
test -d "$RUSTUP_HOME" || {
    printf 'Rust toolchain directory %s is missing; install Rust before running tests.\n' "$RUSTUP_HOME" >&2
    exit 2
}
isolated=$(mktemp -d)
child=''
on_signal() {
    trap '' HUP INT TERM
    if [ -n "$child" ]; then
        kill -"$1" "$child" 2>/dev/null || true
        wait "$child" 2>/dev/null || true
    fi
    exit "$2"
}
trap 'rm -rf "$isolated"' EXIT
trap 'on_signal HUP 129' HUP
trap 'on_signal INT 130' INT
trap 'on_signal TERM 143' TERM
export HOME="${isolated:?}"
test -n "$HOME" && test "$HOME" != "$original_home" && test -d "$HOME" || exit 1
unset CLAUDE_CONFIG_DIR CODEX_HOME YOKE_SOAK_BIN
# Debug/test builds honor this transport override; no fixture can fall back to real curl.
export YOKE_CURL="$HOME/curl-offline"
printf '#!/bin/sh\nexit 1\n' > "$YOKE_CURL"
chmod 700 "$YOKE_CURL"
"$@" <&0 &
child=$!
wait "$child"
