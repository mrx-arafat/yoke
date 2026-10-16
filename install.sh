#!/bin/sh
# Install or upgrade Yoke: https://github.com/mrx-arafat/yoke
#
#   curl -fsSL https://raw.githubusercontent.com/mrx-arafat/yoke/main/install.sh | bash
#
# Downloads the release executable for this platform and verifies its SHA-256
# checksum, or builds from source with Cargo when no release asset exists,
# installing a C compiler and Rust first when they are missing. Then connects
# the agent hooks, creates or migrates the state database, adds Yoke's line to
# Claude Code's status bar when you have none, offers a starter global policy for
# you to trust, and checks health. It uses sudo only to install a C compiler,
# after asking. Every question defaults to yes, except wrapping a statusline you
# already have, which defaults to no and which YOKE_YES=1 never answers.
#
# Environment:
#   YOKE_INSTALL_DIR      absolute install directory
#                         (default: /usr/local/bin for root, else $HOME/.local/bin)
#   YOKE_VERSION          release tag to install, for example v0.1.0 (default: latest)
#   YOKE_DOWNLOAD_URL     base URL serving yoke-<target> and yoke-<target>.sha256
#   YOKE_FROM_SOURCE=1    build from source with Cargo instead of downloading
#   YOKE_YES=1            answer yes to every question, also without a terminal;
#                         trusting a policy still needs you to type the phrase
#   YOKE_INSTALL_RUST=1   install Rust with rustup without asking when Cargo is missing
#   YOKE_NO_MODIFY_PATH=1 never add the install directory to shell startup files
#   YOKE_NO_STATUSLINE=1  never touch Claude Code's statusLine setting
#   YOKE_AGENTS           agents to connect, for example claude,codex, or none
#                         (default: each of claude and codex that is found)

say() { printf 'yoke-install: %s\n' "$*"; }
err() { printf 'yoke-install: error: %s\n' "$*" >&2; }
die() {
    err "$@"
    exit 1
}

cleanup() {
    if [ -n "$staged" ]; then rm -f "$staged"; fi
    if [ -n "$tmp" ]; then rm -rf "$tmp"; fi
}

has_tty() { (exec </dev/tty) 2>/dev/null; }

# Ask $1 on the terminal, defaulting to yes. Returns 0 for yes, 1 for no and 2 without a
# terminal. YOKE_YES=1 answers yes without asking.
ask() {
    if [ "${YOKE_YES:-}" = 1 ]; then
        say "$1 yes (YOKE_YES=1)"
        return 0
    fi
    has_tty || return 2
    printf 'yoke-install: %s [Y/n] ' "$1" >/dev/tty
    read -r answer </dev/tty || answer=
    case $answer in
        '' | y | Y | yes | Yes | YES) return 0 ;;
        *) return 1 ;;
    esac
}

# Like ask, but the answer defaults to no and YOKE_YES=1 does not answer it: it changes
# something the user already set up. Returns 0 for yes, 1 for no and 2 without a terminal.
ask_no() {
    has_tty || return 2
    printf 'yoke-install: %s [y/N] ' "$1" >/dev/tty
    read -r answer </dev/tty || answer=
    case $answer in
        y | Y | yes | Yes | YES) return 0 ;;
        *) return 1 ;;
    esac
}

# Alpine and other musl systems cannot run the glibc release executables.
is_musl() {
    if command -v ldd >/dev/null 2>&1; then
        case $(ldd --version 2>&1 || true) in
            *musl*) return 0 ;;
            *) return 1 ;;
        esac
    fi
    for loader in /lib/ld-musl-*; do
        if [ -e "$loader" ]; then return 0; fi
    done
    return 1
}

detect_target() {
    arch=$(uname -m)
    case $arch in
        arm64 | aarch64) arch=aarch64 ;;
        x86_64) ;;
        *) die "unsupported CPU architecture: $arch (Yoke supports x86_64 and aarch64)" ;;
    esac
    case $(uname -s) in
        Darwin) target=$arch-apple-darwin ;;
        Linux)
            target=$arch-unknown-linux-gnu
            if is_musl; then target=$arch-unknown-linux-musl; fi
            ;;
        *) die "unsupported operating system: $(uname -s) (Yoke supports macOS and Linux)" ;;
    esac
}

# Download $1 to $2. Returns 0 on success, 2 when the server answers 404, 1 otherwise.
fetch() {
    if command -v curl >/dev/null 2>&1; then
        code=$(curl -sSL --proto-redir '=https' --retry 3 -o "$2" -w '%{http_code}' "$1") || return 1
        case $code in
            200) return 0 ;;
            404) return 2 ;;
            *)
                err "HTTP $code from $1"
                return 1
                ;;
        esac
    elif command -v wget >/dev/null 2>&1; then
        if wget -q -S -O "$2" "$1" 2>"$tmp/wget.log"; then return 0; fi
        case $(cat "$tmp/wget.log") in
            *"HTTP/"*" 404"*) return 2 ;;
        esac
        cat "$tmp/wget.log" >&2
        return 1
    else
        die "curl or wget is required"
    fi
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sum=$(sha256sum "$1") || die "sha256sum failed"
    elif command -v shasum >/dev/null 2>&1; then
        sum=$(shasum -a 256 "$1") || die "shasum failed"
    else
        die "sha256sum or shasum is required to verify the download"
    fi
    printf '%s\n' "${sum%% *}"
}

# Verify $tmp/$name against $tmp/$name.sha256, written as "<hex>  <name>".
verify() {
    set -f
    # shellcheck disable=SC2046 # split the record on whitespace, globbing is off
    set -- $(cat "$tmp/$name.sha256")
    set +f
    if [ $# -ne 2 ] || [ "$2" != "$name" ] || [ ${#1} -ne 64 ]; then
        die "malformed checksum file $name.sha256; refusing to install"
    fi
    case $1 in *[!0-9a-fA-F]*) die "malformed checksum file $name.sha256; refusing to install" ;; esac
    expected=$(printf '%s' "$1" | tr 'A-F' 'a-f')
    actual=$(sha256 "$tmp/$name") || exit 1
    if [ "$actual" != "$expected" ]; then
        die "checksum mismatch for $name (expected $expected, got $actual); refusing to install"
    fi
    say "verified SHA-256 checksum $actual"
}

# Sets $candidate to a verified release executable. Returns 2 when no release asset exists.
download_release() {
    name=yoke-$target
    if [ -n "${YOKE_DOWNLOAD_URL:-}" ]; then
        base=${YOKE_DOWNLOAD_URL%/}
    elif [ -n "${YOKE_VERSION:-}" ]; then
        base=https://github.com/mrx-arafat/yoke/releases/download/$YOKE_VERSION
    else
        base=https://github.com/mrx-arafat/yoke/releases/latest/download
    fi
    say "downloading $base/$name"
    for file in "$name.sha256" "$name"; do
        status=0
        fetch "$base/$file" "$tmp/$file" || status=$?
        case $status in
            0) ;;
            2) return 2 ;;
            *) die "could not download $base/$file" ;;
        esac
    done
    verify
    candidate=$tmp/$name
}

# Cargo links with the system C compiler, which also builds the bundled SQLite.
ensure_cc() {
    if [ "$(uname -s)" = Darwin ]; then
        if xcode-select -p >/dev/null 2>&1; then return 0; fi
        status=0
        ask "building Yoke needs the Xcode Command Line Tools; open their installer with xcode-select --install?" || status=$?
        if [ "$status" -eq 0 ]; then
            xcode-select --install || true
            die "rerun this installer when the Command Line Tools finish installing"
        fi
        die "building Yoke needs the Xcode Command Line Tools; run xcode-select --install, then rerun this installer"
    fi
    if command -v cc >/dev/null 2>&1; then return 0; fi
    # Detect the package manager by its command, not by distribution name.
    if command -v apt-get >/dev/null 2>&1; then
        cc_cmd="apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq build-essential"
    elif command -v dnf >/dev/null 2>&1; then
        cc_cmd="dnf install -y gcc"
    elif command -v yum >/dev/null 2>&1; then
        cc_cmd="yum install -y gcc"
    elif command -v zypper >/dev/null 2>&1; then
        cc_cmd="zypper --non-interactive install gcc"
    elif command -v pacman >/dev/null 2>&1; then
        cc_cmd="pacman -Sy --noconfirm --needed gcc"
    elif command -v apk >/dev/null 2>&1; then
        cc_cmd="apk add build-base"
    else
        die "building Yoke needs a C compiler (cc); install gcc with your package manager, then rerun"
    fi
    if [ "$(id -u)" = 0 ]; then
        how="as root"
    elif command -v sudo >/dev/null 2>&1; then
        how="with sudo"
    else
        die "building Yoke needs a C compiler; as root, run: $cc_cmd, then rerun"
    fi
    status=0
    ask "building Yoke needs a C compiler; run '$cc_cmd' $how?" || status=$?
    case $status in
        0) ;;
        1) die "a C compiler is required; install it $how with: $cc_cmd, then rerun" ;;
        *) die "a C compiler is required; install it $how with: $cc_cmd, or rerun with YOKE_YES=1" ;;
    esac
    say "installing a C compiler $how; this takes a minute"
    status=0
    if [ "$(id -u)" = 0 ]; then
        sh -c "$cc_cmd" >"$tmp/compiler.log" 2>&1 || status=$?
    else
        # shellcheck disable=SC2024 # the log is the user's own file in $tmp
        sudo sh -c "$cc_cmd" >"$tmp/compiler.log" 2>&1 || status=$?
    fi
    if [ "$status" -ne 0 ]; then
        cat "$tmp/compiler.log" >&2
        die "installing a C compiler $how failed"
    fi
    command -v cc >/dev/null 2>&1 || die "cc is still missing after installing a C compiler"
}

ensure_cargo() {
    if command -v cargo >/dev/null 2>&1; then return 0; fi
    cargo_env=${CARGO_HOME:-$HOME/.cargo}/env
    if [ -f "$cargo_env" ]; then
        # shellcheck disable=SC1090 # rustup's environment script
        . "$cargo_env"
        if command -v cargo >/dev/null 2>&1; then return 0; fi
    fi
    rustup_cmd="curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path"
    status=0
    if [ "${YOKE_INSTALL_RUST:-}" != 1 ]; then
        ask "building Yoke needs Cargo; install Rust with rustup?" || status=$?
    fi
    case $status in
        0) ;;
        1) die "Cargo is required. Install Rust with: $rustup_cmd" ;;
        *) die "Cargo is required. Install Rust with: $rustup_cmd (or rerun with YOKE_INSTALL_RUST=1 to let this script do it; building takes a few minutes)" ;;
    esac
    say "installing Rust with rustup into ${CARGO_HOME:-$HOME/.cargo}; this takes a minute"
    fetch https://sh.rustup.rs "$tmp/rustup.sh" || die "could not download https://sh.rustup.rs"
    # rustup prints shell setup advice that this script follows itself, so keep its log for errors.
    if ! sh "$tmp/rustup.sh" -y --profile minimal --no-modify-path >"$tmp/rustup.log" 2>&1; then
        cat "$tmp/rustup.log" >&2
        die "rustup failed"
    fi
    # shellcheck disable=SC1090 # rustup's environment script
    . "$cargo_env"
    command -v cargo >/dev/null 2>&1 || die "cargo is still not on PATH after installing Rust"
}

# Sets $candidate to an executable built by cargo install into a temporary root.
build_from_source() {
    ensure_cc
    ensure_cargo
    set -- --locked --git https://github.com/mrx-arafat/yoke --root "$tmp/root"
    if [ -n "${YOKE_VERSION:-}" ]; then set -- "$@" --tag "$YOKE_VERSION"; fi
    say "building Yoke from source with: cargo install $* yoke-cli"
    say "this takes a few minutes"
    # The temporary root on PATH keeps Cargo from suggesting it be added there.
    PATH="$tmp/root/bin:$PATH" cargo install "$@" yoke-cli || die "cargo install failed"
    candidate=$tmp/root/bin/yoke
}

# Refuse, before anything is replaced or connected, when the state database was written by a
# newer Yoke: hooks that run this older executable would deny every tool call.
check_state() {
    report=$("$1" health 2>&1) || true
    line=$(printf '%s\n' "$report" | grep 'is newer than supported' | head -n 1 || true)
    if [ -n "$line" ]; then
        err "the state database was written by a newer Yoke than this one:$line"
        die "nothing was changed; rerun without YOKE_VERSION to install the newest release, or restore an older backup of ~/.yoke/state.db with its -wal and -shm files (docs/upgrade-notes.md)"
    fi
}

# Replace $bin atomically from $candidate: live agent hooks may execute it at any moment.
install_binary() {
    if [ -d "$bin" ]; then die "$bin is a directory"; fi
    staged=$(mktemp "$dir/.yoke.XXXXXX") || die "cannot create a file in $dir"
    cp "$candidate" "$staged" || die "cannot write $staged"
    chmod 755 "$staged"
    if ! "$staged" --version >/dev/null 2>&1; then
        die "the new executable does not run on this system; rerun with YOKE_FROM_SOURCE=1 to build it"
    fi
    check_state "$staged"
    mv -f "$staged" "$bin" || die "cannot replace $bin"
    staged=
}

# Sets $connected to 1 when the hooks now run $bin.
connect_agents() {
    connected=
    agents=${YOKE_AGENTS:-}
    if [ -z "$agents" ]; then
        if command -v claude >/dev/null 2>&1 || [ -d "${CLAUDE_CONFIG_DIR:-$HOME/.claude}" ]; then agents=claude; fi
        if command -v codex >/dev/null 2>&1 || [ -d "${CODEX_HOME:-$HOME/.codex}" ]; then agents=${agents:+$agents,}codex; fi
        if [ -z "$agents" ]; then
            say "found neither Claude Code nor Codex, so no hooks were connected"
            say "after installing one, rerun this installer to connect its hooks"
            return 0
        fi
    fi
    if [ "$agents" = none ]; then
        say "skipping hooks because YOKE_AGENTS=none"
        return 0
    fi
    say "connecting hooks: yoke install --agent $agents"
    if output=$("$bin" install --agent "$agents" 2>&1); then
        printf '%s\n' "$output"
        connected=1
        return 0
    fi
    case $output in
        *"executable changed"*)
            say "your hooks run a Yoke executable in another location; moving them to $bin"
            say "removing them: yoke uninstall --agent $agents"
            "$bin" uninstall --agent "$agents" || die "yoke uninstall --agent $agents failed"
            say "connecting them again: yoke install --agent $agents"
            "$bin" install --agent "$agents" || die "yoke install --agent $agents failed"
            connected=1
            # The refusal names the executable the hooks used to run; that copy stays on disk.
            old=$(printf '%s\n' "$output" | sed -n 's/^.*hooks run \(.*\), not .*$/\1/p' | head -n 1)
            if [ -n "$old" ] && [ "$old" != "$bin" ] && [ -f "$old" ]; then
                say "your previous Yoke executable is still at $old; once nothing else uses it, remove it so it cannot shadow $bin: rm -f '$old'"
            fi
            ;;
        *)
            printf '%s\n' "$output" >&2
            die "yoke install --agent $agents failed"
            ;;
    esac
}

# Append $path_line to the startup file $1 unless it already has it.
add_path_line() {
    if [ -f "$1" ]; then
        while IFS= read -r existing || [ -n "$existing" ]; do
            if [ "$existing" = "$path_line" ]; then
                say "$1 already adds $dir to PATH"
                return 0
            fi
        done <"$1"
    fi
    printf '\n%s\n' "$path_line" >>"$1" || die "cannot write $1"
    say "added $dir to PATH in $1"
}

# Sets $yoke_cmd to how the user runs Yoke, adding $dir to PATH for new shells when asked.
setup_path() {
    yoke_cmd=yoke
    found=$(command -v yoke || true)
    case ":$PATH:" in
        *":$dir:"*)
            if [ "$found" != "$bin" ]; then
                say "warning: yoke on your PATH is $found, which shadows $bin"
                say "remove it (cargo uninstall yoke-cli if Cargo installed it) or put $dir first on PATH"
                yoke_cmd=$bin
            fi
            return 0
            ;;
    esac
    path_line="export PATH=\"$dir:\$PATH\"  # added by yoke install.sh"
    files="$HOME/.bashrc"
    case ${SHELL:-} in */zsh) files="$files and $HOME/.zshrc" ;; esac
    status=1
    if [ "${YOKE_NO_MODIFY_PATH:-}" != 1 ]; then
        status=0
        ask "$dir is not on your PATH; add it in $files?" || status=$?
    fi
    if [ "$status" -ne 0 ]; then
        yoke_cmd=$bin
        say "$dir is not on your PATH; to add it, put this line in your shell startup file:"
        say "  $path_line"
        return 0
    fi
    add_path_line "$HOME/.bashrc"
    case ${SHELL:-} in */zsh) add_path_line "$HOME/.zshrc" ;; esac
    say "new interactive shells find yoke; in this shell, run: exec \$SHELL -l"
    say "shells that skip that file (cron, scripts, and on Ubuntu 'ssh host command') need the full path: $bin"
    if [ -n "$found" ] && [ "$found" != "$bin" ]; then
        say "warning: another yoke is already on your PATH at $found; remove it so it cannot shadow $bin"
    fi
}

# Offer a starter global policy when none is trusted. Only a human at the terminal can
# trust it, by typing the confirmation phrase; YOKE_YES never answers that.
activate() {
    report=$("$bin" health 2>&1) || true
    case $report in
        *'/.yoke/config.toml",true,'*) return 0 ;;
    esac
    status=2
    if has_tty; then
        status=0
        ask "no trusted global policy yet; add the git-safety preset in observe mode, which records but blocks nothing, and review it for trust?" || status=$?
    fi
    if [ "$status" -eq 0 ]; then
        "$bin" preset add git-safety --global || status=1
    fi
    if [ "$status" -eq 0 ]; then
        say "review the policy below; type trust to approve it, or anything else to skip"
        "$bin" trust --global </dev/tty >/dev/tty 2>&1 || true
        report=$("$bin" health 2>&1) || true
        case $report in
            *'/.yoke/config.toml",true,'*)
                say "trusted the global policy; Yoke now records what it would deny in every project"
                return 0
                ;;
        esac
    fi
    say "to activate Yoke for every project, review and trust a starter global policy in your own terminal:"
    say "  $yoke_cmd preset add git-safety --global"
    say "  $yoke_cmd trust --global"
}

# Claude Code draws Yoke's line only when its statusLine runs `yoke statusline`, and that line
# is also what records Claude's quota for `yoke usage`. Add it when none is set. Never replace
# a statusline the user already has, and wrap it only when they say yes at a terminal: Yoke
# strips its colors, joins its lines, and drops the whole line if it takes over 250 ms.
# Sets $statusline_on to 1 when Claude Code now runs Yoke's statusline.
setup_statusline() {
    case ,$agents, in *,claude,*) ;; *) return 0 ;; esac
    if [ "${YOKE_NO_STATUSLINE:-}" = 1 ]; then
        say "skipping Claude's statusline because YOKE_NO_STATUSLINE=1; to add it later: $yoke_cmd install --agent claude --statusline-if-absent"
        return 0
    fi
    if ! plan=$("$bin" install --agent claude --statusline-if-absent --dry-run 2>&1); then
        printf '%s\n' "$plan" >&2
        say "skipping Claude's statusline because this yoke could not check it (an older release without --statusline-if-absent?); see: $yoke_cmd install --help"
        return 0
    fi
    case $plan in
        *"statusline kept"*)
            status=1
            why="keeping your statusline"
            if [ "${YOKE_YES:-}" = 1 ]; then
                why="keeping your statusline (YOKE_YES=1 never replaces or wraps one)"
            else
                status=0
                ask_no "Claude Code already has a statusline; wrap it so Yoke's line comes first? Yoke removes its colors, joins its lines, and drops it if it takes over 250 ms" || status=$?
            fi
            if [ "$status" -eq 0 ]; then
                "$bin" install --agent claude --statusline || die "yoke install --agent claude --statusline failed"
                statusline_on=1
            else
                say "$why; to show Yoke's line in front of it: $yoke_cmd install --agent claude --statusline"
            fi
            ;;
        *"changes: 0"*)
            say "Claude Code already shows Yoke's line"
            ;;
        *)
            status=0
            ask "add Yoke's line (mode, context, 5-hour and weekly quota) to Claude Code's status bar?" || status=$?
            if [ "$status" -eq 0 ]; then
                "$bin" install --agent claude --statusline-if-absent || die "yoke install --agent claude --statusline-if-absent failed"
                statusline_on=1
            else
                say "not adding Yoke's statusline; to add it later: $yoke_cmd install --agent claude --statusline-if-absent"
            fi
            ;;
    esac
}

main() {
    set -eu
    tmp=
    staged=
    statusline_on=
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM

    case ${YOKE_VERSION:-} in
        *[!A-Za-z0-9._-]* | [!A-Za-z0-9]*) die "invalid YOKE_VERSION: $YOKE_VERSION" ;;
    esac
    detect_target
    if [ "$(id -u)" = 0 ]; then
        dir=${YOKE_INSTALL_DIR:-/usr/local/bin}
    else
        dir=${YOKE_INSTALL_DIR:-$HOME/.local/bin}
    fi
    case $dir in /*) ;; *) die "YOKE_INSTALL_DIR must be an absolute path: $dir" ;; esac
    bin=$dir/yoke
    mkdir -p "$dir" || die "cannot create $dir"
    tmp=$(mktemp -d) || die "cannot create a temporary directory"
    say "installing Yoke for $target into $dir"

    from_source=${YOKE_FROM_SOURCE:-}
    case $target in
        *-musl)
            say "this system uses musl libc and release executables need glibc, so Yoke is built from source"
            from_source=1
            ;;
    esac
    if [ "$from_source" = 1 ]; then
        build_from_source
    else
        status=0
        download_release || status=$?
        if [ "$status" -eq 2 ]; then
            say "no release executable found for $target; building from source instead"
            build_from_source
        elif [ "$status" -ne 0 ]; then
            die "release download failed"
        fi
    fi
    install_binary
    version=$("$bin" --version) || die "$bin --version failed"
    say "installed Yoke $version at $bin"

    connect_agents
    # Migrating the database while hooks still run an older executable would make those hooks
    # fail closed, so migrate only once the hooks run this one.
    if [ "$connected" = 1 ]; then
        if ! output=$("$bin" log --limit 1 2>&1); then
            printf '%s\n' "$output" >&2
            die "yoke could not create or migrate its state database"
        fi
    else
        say "left the state database as it is, because no hooks were connected to $bin"
    fi
    setup_path
    if [ "$connected" = 1 ]; then
        setup_statusline
        activate
    fi
    say "checking health: yoke health"
    status=0
    "$bin" health || status=$?
    case $status in
        0) ;;
        1) say "yoke health reported problems; follow its guidance above" ;;
        *) die "yoke health failed with exit status $status" ;;
    esac

    if [ "$connected" = 1 ]; then
        say "Yoke now records hook events from every session of the agents it connected, in every project, for this user; observe mode, the default, never blocks anything"
    fi
    say "no gate runs until a policy is trusted. next steps:"
    say "  - in each project to govern, run $yoke_cmd init, review yoke.toml, then $yoke_cmd trust from your own terminal"
    if [ "$connected" = 1 ]; then
        case ,$agents, in *,codex,*) say "  - in Codex, approve the Yoke hooks with /hooks; Codex skips unapproved hooks without any warning" ;; esac
        if [ "$statusline_on" = 1 ]; then
            say "Claude Code shows Yoke's line (mode, context, 5-hour and weekly quota) from its next session; restart running sessions"
        fi
        case ,$agents, in *,codex,*) say "Codex has no statusline hook; $yoke_cmd usage shows Codex quota when Codex records it" ;; esac
    fi
}

main "$@"
