#!/bin/sh
# Installs sshh (binary + man page).
#
#   curl -fsSL https://raw.githubusercontent.com/ewiggin/sshh/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/ewiggin/sshh/main/install.sh | sh -s -- --uninstall
#
# On Linux x86_64 / aarch64 it downloads the static binary of the latest
# release; otherwise (or if that fails) it builds sshh from source, which
# needs git, make and Rust. Run from a clone (./install.sh), it builds that
# code. See --help for the options.

set -eu

REPO="${SSHH_REPO:-https://github.com/ewiggin/sshh.git}"
RELEASES="${SSHH_RELEASES:-https://github.com/ewiggin/sshh/releases}"
MIN_RUST="1.88"

if [ "$(id -u)" -eq 0 ]; then
    PREFIX="${SSHH_PREFIX:-/usr/local}"
else
    PREFIX="${SSHH_PREFIX:-$HOME/.local}"
fi
BINDIR="$PREFIX/bin"
MANDIR="$PREFIX/share/man"

info() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() {
    printf '\033[1;31merror:\033[0m %s\n' "$*" >&2
    exit 1
}

usage() {
    cat <<EOF
Installs sshh (binary + man page).

Usage: install.sh [--uninstall | --help]

By default it downloads the static binary of the latest release (Linux x86_64
and aarch64) and falls back to building from source.

Environment:
  SSHH_PREFIX       install prefix (default: ~/.local, or /usr/local as root)
  SSHH_VERSION      release to install, e.g. v0.1.0 (default: the latest)
  SSHH_FROM_SOURCE  set to 1 to always build from source (needs git, make, Rust)
  SSHH_REF          branch or tag to build from source (default: main)
EOF
}

# Runs a command with sudo when the prefix isn't writable by us.
as_owner() {
    dir="$PREFIX"
    while [ ! -d "$dir" ]; do dir=$(dirname "$dir"); done
    if [ -w "$dir" ]; then
        "$@"
    elif command -v sudo >/dev/null 2>&1; then
        info "$PREFIX is not writable, using sudo"
        sudo "$@"
    else
        die "$PREFIX is not writable and sudo is not available (set SSHH_PREFIX)"
    fi
}

# version_ge 1.90.0 1.88 -> true
version_ge() {
    [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n 1)" = "$2" ]
}

uninstall() {
    info "Removing sshh from $PREFIX"
    as_owner rm -f "$BINDIR/sshh" "$MANDIR/man1/sshh.1"
    info "Done. Your connections are kept in ~/.local/share/sshh (delete it to remove them)."
}

# Prints the directory of the clone this script runs from, if any.
clone_dir() {
    here=$(cd "$(dirname "$0")" 2>/dev/null && pwd) || here=""
    if [ -n "$here" ] && [ -f "$here/Cargo.toml" ] && grep -q '^name = "sshh"' "$here/Cargo.toml"; then
        echo "$here"
    fi
}

# Rust target of the prebuilt binary for this machine, if there is one.
release_target() {
    [ "$(uname -s)" = Linux ] || return 0
    case "$(uname -m)" in
        x86_64 | amd64) echo x86_64-unknown-linux-musl ;;
        aarch64 | arm64) echo aarch64-unknown-linux-musl ;;
    esac
}

download() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        return 1
    fi
}

verify_checksum() {
    (
        cd "$1" || exit 1
        if command -v sha256sum >/dev/null 2>&1; then
            sha256sum -c "$2" >/dev/null
        elif command -v shasum >/dev/null 2>&1; then
            shasum -a 256 -c "$2" >/dev/null
        else
            echo "no sha256sum or shasum: skipping the checksum" >&2
        fi
    )
}

# Downloads and installs the release binary. Returns 1 if it isn't available.
install_binary() {
    target=$(release_target)
    if [ -z "$target" ]; then
        info "No prebuilt binary for $(uname -s) $(uname -m)"
        return 1
    fi
    asset="sshh-$target.tar.gz"
    case "${SSHH_VERSION:-latest}" in
        latest) url="$RELEASES/latest/download/$asset" ;;
        *) url="$RELEASES/download/$SSHH_VERSION/$asset" ;;
    esac
    info "Downloading $url"
    if ! download "$url" "$TMP/$asset" || ! download "$url.sha256" "$TMP/$asset.sha256"; then
        warn "could not download the release binary"
        return 1
    fi
    verify_checksum "$TMP" "$asset.sha256" || die "checksum mismatch for $asset"
    tar -xzf "$TMP/$asset" -C "$TMP" || return 1

    info "Installing to $PREFIX"
    # Called from an `if`, where `set -e` doesn't apply: check each step.
    as_owner install -Dm755 "$TMP/sshh-$target/sshh" "$BINDIR/sshh" || die "could not install the binary"
    as_owner install -Dm644 "$TMP/sshh-$target/sshh.1" "$MANDIR/man1/sshh.1" || die "could not install the man page"
}

check_build_requirements() {
    missing=""
    for cmd in git cargo make; do
        command -v "$cmd" >/dev/null 2>&1 || missing="$missing$cmd "
    done
    if [ -n "$missing" ]; then
        die "missing ${missing}to build from source (install Rust with https://rustup.rs and git/make with your package manager)"
    fi
    rust=$(rustc --version 2>/dev/null | awk '{print $2}')
    if [ -z "$rust" ] || ! version_ge "$rust" "$MIN_RUST"; then
        die "Rust >= $MIN_RUST is required (found ${rust:-none}); run 'rustup update'"
    fi
}

install_from_source() {
    check_build_requirements
    src=$(clone_dir)
    if [ -n "$src" ]; then
        info "Building $src"
    else
        ref="${SSHH_REF:-main}"
        info "Cloning $REPO ($ref)"
        git clone --quiet --depth 1 --branch "$ref" "$REPO" "$TMP/src" || die "could not clone $REPO"
        src="$TMP/src"
    fi
    info "Building (this can take a minute)"
    make -C "$src" build
    info "Installing to $PREFIX"
    as_owner make -C "$src" install PREFIX="$PREFIX"
}

main() {
    case "${1:-}" in
        --uninstall) uninstall; return ;;
        -h | --help) usage; return ;;
        "") ;;
        *) die "unknown option '$1' (try --help)" ;;
    esac

    command -v ssh >/dev/null 2>&1 || warn "ssh not found: sshh needs OpenSSH to connect"
    TMP=$(mktemp -d)
    trap 'rm -rf "$TMP"' EXIT
    trap 'exit 130' INT TERM

    # A clone, SSHH_FROM_SOURCE or SSHH_REF mean "build this code".
    if [ -n "$(clone_dir)" ] || [ "${SSHH_FROM_SOURCE:-0}" != 0 ] || [ -n "${SSHH_REF:-}" ]; then
        install_from_source
    elif ! install_binary; then
        info "Building from source instead"
        install_from_source
    fi

    info "Installed: $("$BINDIR/sshh" --version)"
    echo "    binary:   $BINDIR/sshh"
    echo "    man page: $MANDIR/man1/sshh.1"

    case ":$PATH:" in
        *":$BINDIR:"*) ;;
        *) warn "$BINDIR is not in your PATH; add it to your shell profile:
    export PATH=\"$BINDIR:\$PATH\"" ;;
    esac
    echo
    echo "Get started: run 'sshh', or 'sshh import-ssh-config' to import your ~/.ssh/config."
}

main "$@"
