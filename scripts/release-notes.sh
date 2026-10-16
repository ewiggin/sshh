#!/bin/sh
# Prints the notes of a version from CHANGES.md: the body of its
# `## X.Y.Z — date` section, without the heading or surrounding blank lines.
# The release workflow uses it as the body of the GitHub release.
#
#   scripts/release-notes.sh v0.8.1      # or 0.8.1
#
# Fails if the section is missing or empty.

set -eu

cd "$(dirname "$0")/.."

[ $# -eq 1 ] || {
    echo "usage: $0 <vX.Y.Z>" >&2
    exit 1
}
version=${1#v}

# Lines between `## <version> ` (or `## <version>` alone) and the next `## `,
# dropping blank lines at the start and the end.
notes=$(awk -v v="$version" '
    /^## / { inside = ($0 == "## " v || index($0, "## " v " ") == 1); next }
    inside && (NF || started) { started = 1; print }
' CHANGES.md)

if [ -z "$notes" ]; then
    echo "error: CHANGES.md has no notes for $version" >&2
    exit 1
fi
# $(…) already dropped the trailing blank lines.
printf '%s\n' "$notes"
