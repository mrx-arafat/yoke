#!/bin/sh
# Install or upgrade Yoke: https://github.com/mrx-arafat/yoke
#
#   curl -fsSL https://raw.githubusercontent.com/mrx-arafat/yoke/main/install.sh | bash
#
# Downloads the release executable for this platform and verifies its SHA-256
# checksum, or builds from source with Cargo when no release asset exists.
# Then connects the agent hooks, creates or migrates the state database and
# checks health. It never uses sudo.
#
# Environment:
#   YOKE_INSTALL_DIR    absolute install directory (default: $HOME/.local/bin)
#   YOKE_VERSION        release tag to install, for example v0.1.0 (default: latest)
#   YOKE_DOWNLOAD_URL   base URL serving yoke-<target> and yoke-<target>.sha256
#   YOKE_FROM_SOURCE=1  build from source with Cargo instead of downloading
#   YOKE_INSTALL_RUST=1 install Rust with rustup without asking when Cargo is missing
#   YOKE_AGENTS         agents to connect, for example claude,codex, or none
#                       (default: each of claude and codex that is found)

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

detect_target() {
    arch=$(uname -m)
    case $arch in
        arm64 | aarch64) arch=aarch64 ;;
        x86_64) ;;
        *) die "unsupported CPU architecture: $arch (Yoke supports x86_64 and aarch64)" ;;
    esac
    case $(uname -s) in
        Darwin) target=$arch-apple-darwin ;;
        Linux) target=$arch-unknown-linux-gnu ;;
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

ensure_cargo() {
    if command -v cargo >/dev/null 2>&1; then return 0; fi
    cargo_env=${CARGO_HOME:-$HOME/.cargo}/env
    if [ -f "$cargo_env" ]; then
        # shellcheck disable=SC1090 # rustup's environment script
        . "$cargo_env"
        if command -v cargo >/dev/null 2>&1; then return 0; fi
    fi
    rustup_cmd="curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path"
    if [ "${YOKE_INSTALL_RUST:-}" = 1 ]; then
        :
    elif (exec </dev/tty) 2>/dev/null; then
        printf 'yoke-install: building Yoke needs Cargo. Install Rust with rustup now? [y/N] ' >/dev/tty
        read -r answer </dev/tty || answer=
        case $answer in
            y | Y | yes | YES) ;;
            *) die "Cargo is required. Install Rust with: $rustup_cmd" ;;
        esac
    else
        die "Cargo is required. Install Rust with: $rustup_cmd (or rerun with YOKE_INSTALL_RUST=1)"
    fi
    say "installing Rust with rustup"
    fetch https://sh.rustup.rs "$tmp/rustup.sh" || die "could not download https://sh.rustup.rs"
    sh "$tmp/rustup.sh" -y --profile minimal --no-modify-path || die "rustup failed"
    # shellcheck disable=SC1090 # rustup's environment script
    . "$cargo_env"
    command -v cargo >/dev/null 2>&1 || die "cargo is still not on PATH after installing Rust"
}

# Sets $candidate to an executable built by cargo install into a temporary root.
build_from_source() {
    ensure_cargo
    set -- --locked --git https://github.com/mrx-arafat/yoke --root "$tmp/root"
    if [ -n "${YOKE_VERSION:-}" ]; then set -- "$@" --tag "$YOKE_VERSION"; fi
    say "building Yoke from source with: cargo install $* yoke-cli"
    say "this takes a few minutes"
    # The temporary root on PATH keeps Cargo from suggesting it be added there.
    PATH="$tmp/root/bin:$PATH" cargo install "$@" yoke-cli || die "cargo install failed"
    candidate=$tmp/root/bin/yoke
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
            say "found neither Claude Code nor Codex; skipping hooks"
            say "after installing one, run: yoke install --agent claude,codex"
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
            ;;
        *)
            printf '%s\n' "$output" >&2
            die "yoke install --agent $agents failed"
            ;;
    esac
}

main() {
    set -eu
    tmp=
    staged=
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM

    case ${YOKE_VERSION:-} in
        *[!A-Za-z0-9._-]* | [!A-Za-z0-9]*) die "invalid YOKE_VERSION: $YOKE_VERSION" ;;
    esac
    detect_target
    dir=${YOKE_INSTALL_DIR:-$HOME/.local/bin}
    case $dir in /*) ;; *) die "YOKE_INSTALL_DIR must be an absolute path: $dir" ;; esac
    bin=$dir/yoke
    mkdir -p "$dir" || die "cannot create $dir"
    tmp=$(mktemp -d) || die "cannot create a temporary directory"
    say "installing Yoke for $target into $dir"

    if [ "${YOKE_FROM_SOURCE:-}" = 1 ]; then
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
    say "checking health: yoke health"
    status=0
    "$bin" health || status=$?
    case $status in
        0) ;;
        1) say "yoke health reported problems; follow its guidance above" ;;
        *) die "yoke health failed with exit status $status" ;;
    esac

    case ":$PATH:" in
        *":$dir:"*)
            found=$(command -v yoke || true)
            if [ "$found" != "$bin" ]; then
                say "warning: yoke on your PATH is $found, which shadows $bin"
                say "remove it (cargo uninstall yoke-cli if Cargo installed it) or put $dir first on PATH"
            fi
            ;;
        *)
            say "$dir is not on your PATH. Add it with one of:"
            say "  bash: echo 'export PATH=\"$dir:\$PATH\"' >> ~/.bashrc"
            say "  zsh:  echo 'export PATH=\"$dir:\$PATH\"' >> ~/.zshrc"
            ;;
    esac
    say "next steps:"
    say "  1. in Codex, approve the Yoke hooks with /hooks"
    say "  2. in each project to govern, run yoke init, review yoke.toml, then yoke trust from your own terminal"
}

main "$@"
