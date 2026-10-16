#!/bin/sh
# Bumps the version of sshh and commits it as "bump to vX.Y.Z".
#
#   make version v0.2.0              # or scripts/bump.sh v0.2.0
#   make version patch|minor|major
#   make version                     # show the current version and the next ones
#
# Updates Cargo.toml, Cargo.lock, the man page (version and date) and
# CHANGES.md (`## Unreleased` becomes `## X.Y.Z — date`; it must exist and
# list something). It doesn't tag or push: it prints the commands to publish
# the release. POSIX sh with tools that work the same in GNU and BSD (macOS).

set -eu

cd "$(dirname "$0")/.."

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

# rewrite FILE CMD...: runs CMD with FILE as stdin and replaces FILE with its
# output (`sed -i` differs between GNU and BSD). Keeps FILE's permissions.
rewrite() {
    file=$1
    shift
    "$@" <"$file" >"$file.tmp" || { rm -f "$file.tmp"; die "could not update $file"; }
    cat "$file.tmp" >"$file"
    rm -f "$file.tmp"
}

[ $# -le 1 ] || die "usage: make version [vX.Y.Z | patch | minor | major]"

current=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
echo "$current" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || die "can't read the version in Cargo.toml"

major=${current%%.*}
rest=${current#*.}
minor=${rest%%.*}
patch=${rest#*.}

# Without arguments: show the current version and what each bump gives.
if [ $# -eq 0 ]; then
    tag=$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || echo none)
    cat <<EOF
Current version: $current (Cargo.toml; latest tag: $tag)

Usage: make version <vX.Y.Z | patch | minor | major>
    patch  -> $major.$minor.$((patch + 1))
    minor  -> $major.$((minor + 1)).0
    major  -> $((major + 1)).0.0
EOF
    exit 0
fi

case "$1" in
    patch) new="$major.$minor.$((patch + 1))" ;;
    minor) new="$major.$((minor + 1)).0" ;;
    major) new="$((major + 1)).0.0" ;;
    *) new="${1#v}" ;;
esac
echo "$new" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || die "'$1' is not a version (X.Y.Z) nor patch/minor/major"

# The new version must be greater than the current one.
highest=$(printf '%s\n%s\n' "$current" "$new" | sort -V | tail -n 1)
if [ "$new" = "$current" ] || [ "$highest" != "$new" ]; then
    die "the new version ($new) must be greater than the current one ($current)"
fi

# Only the bump goes in the commit.
if [ -n "$(git status --porcelain)" ]; then
    die "the working tree has uncommitted changes; commit or stash them first"
fi
if git rev-parse -q --verify "refs/tags/v$new" >/dev/null; then
    die "tag v$new already exists"
fi

# CHANGES.md must have a `## Unreleased` section with something in it.
grep -q '^## Unreleased$' CHANGES.md || die "CHANGES.md has no '## Unreleased' section"
awk '/^## / { inside = ($0 == "## Unreleased"); next } inside && NF { found = 1 } END { exit !found }' CHANGES.md ||
    die "the '## Unreleased' section of CHANGES.md is empty: list the changes first"

echo "Bumping $current -> $new"

today=$(date +%Y-%m-%d)

# The [package] version is the first `version = ` line of Cargo.toml.
rewrite Cargo.toml awk -v cur="version = \"$current\"" -v new="version = \"$new\"" \
    '!done && $0 == cur { print new; done = 1; next } { print }'
cargo update --workspace --offline --quiet
rewrite man/sshh.1 sed "1s/^\.TH SSHH 1 [^ ]* \"sshh [^\"]*\"/.TH SSHH 1 $today \"sshh $new\"/"
rewrite CHANGES.md sed "s/^## Unreleased\$/## $new — $today/"

grep -q "^version = \"$new\"" Cargo.toml || die "Cargo.toml was not updated"
grep -q "\"sshh $new\"" man/sshh.1 || die "man/sshh.1 was not updated"
grep -q "^## $new — $today\$" CHANGES.md || die "CHANGES.md was not updated"

git add Cargo.toml Cargo.lock man/sshh.1 CHANGES.md
git commit --quiet -m "bump to v$new"
git log --oneline -1

cat <<EOF

To publish the release:
    git tag v$new && git push origin main v$new
EOF
