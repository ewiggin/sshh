#!/bin/sh
# Bumps the version of sshh and commits it as "bump to vX.Y.Z".
#
#   make version v0.2.0              # or scripts/bump.sh v0.2.0
#   make version patch|minor|major
#
# Updates Cargo.toml, Cargo.lock and the man page (version and date). It
# doesn't tag or push: it prints the commands to publish the release.

set -eu

cd "$(dirname "$0")/.."

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

[ $# -eq 1 ] || die "usage: $0 <X.Y.Z | patch | minor | major>"

current=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
echo "$current" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || die "can't read the version in Cargo.toml"

major=${current%%.*}
rest=${current#*.}
minor=${rest%%.*}
patch=${rest#*.}

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

echo "Bumping $current -> $new"

# The [package] version is the first `version = ` line of Cargo.toml.
sed -i "0,/^version = \"$current\"/s//version = \"$new\"/" Cargo.toml
cargo update --workspace --offline --quiet
sed -i "1s/^\.TH SSHH 1 [^ ]* \"sshh [^\"]*\"/.TH SSHH 1 $(date +%Y-%m-%d) \"sshh $new\"/" man/sshh.1

grep -q "^version = \"$new\"" Cargo.toml || die "Cargo.toml was not updated"
grep -q "\"sshh $new\"" man/sshh.1 || die "man/sshh.1 was not updated"

git add Cargo.toml Cargo.lock man/sshh.1
git commit --quiet -m "bump to v$new"
git log --oneline -1

cat <<EOF

To publish the release:
    git tag v$new && git push origin main v$new
EOF
