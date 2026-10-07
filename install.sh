#!/bin/sh
# Installs sshh (binary + man page) from source.
#
#   curl -fsSL https://raw.githubusercontent.com/ewiggin/sshh/master/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/ewiggin/sshh/master/install.sh | sh -s -- --uninstall
#
# Environment:
#   SSHH_PREFIX  install prefix (default: ~/.local, or /usr/local as root)
#   SSHH_REF     branch or tag to install (default: master)
#   SSHH_REPO    git repository (default: https://github.com/ewiggin/sshh.git)
#
# Run from a clone of the repository (./install.sh), it installs that code
# instead of cloning it again.

set -eu

REPO="${SSHH_REPO:-https://github.com/ewiggin/sshh.git}"
REF="${SSHH_REF:-master}"
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
Installs sshh (binary + man page) from source.

Usage: install.sh [--uninstall | --help]

Environment:
  SSHH_PREFIX  install prefix (default: ~/.local, or /usr/local as root)
  SSHH_REF     branch or tag to install (default: master)
  SSHH_REPO    git repository (default: https://github.com/ewiggin/sshh.git)
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

check_requirements() {
    for cmd in git cargo make; do
        command -v "$cmd" >/dev/null 2>&1 || missing="${missing:-}$cmd "
    done
    if [ -n "${missing:-}" ]; then
        die "missing ${missing}(install Rust with https://rustup.rs and git/make with your package manager)"
    fi
    rust=$(rustc --version 2>/dev/null | awk '{print $2}')
    if [ -z "$rust" ] || ! version_ge "$rust" "$MIN_RUST"; then
        die "Rust >= $MIN_RUST is required (found ${rust:-none}); run 'rustup update'"
    fi
    command -v ssh >/dev/null 2>&1 || warn "ssh not found: sshh needs OpenSSH to connect"
}

# Sets SRC to the directory to build: this clone, or a fresh temporary one.
# (Not a command substitution: the cleanup trap must live in this shell.)
find_source() {
    here=$(cd "$(dirname "$0")" 2>/dev/null && pwd || true)
    if [ -n "$here" ] && [ -f "$here/Cargo.toml" ] && grep -q '^name = "sshh"' "$here/Cargo.toml"; then
        info "Installing from $here"
        SRC="$here"
        return
    fi
    TMP=$(mktemp -d)
    trap 'rm -rf "$TMP"' EXIT
    trap 'exit 130' INT TERM
    info "Cloning $REPO ($REF)"
    git clone --quiet --depth 1 --branch "$REF" "$REPO" "$TMP/sshh" || die "could not clone $REPO"
    SRC="$TMP/sshh"
}

main() {
    case "${1:-}" in
        --uninstall) uninstall; return ;;
        -h | --help) usage; return ;;
        "") ;;
        *) die "unknown option '$1' (try --help)" ;;
    esac

    check_requirements
    find_source

    info "Building (this can take a minute)"
    make -C "$SRC" build

    info "Installing to $PREFIX"
    as_owner make -C "$SRC" install PREFIX="$PREFIX"

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
