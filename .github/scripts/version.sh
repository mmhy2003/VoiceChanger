#!/usr/bin/env bash
# Version and changelog reads shared by ci.yml (release-guard, dev label) and release.yml.
#   version.sh read [Cargo.toml]               prints [workspace.package] version
#   version.sh changelog X.Y.Z [CHANGELOG.md]  prints the body of "## [X.Y.Z]"; fails if missing or empty
set -euo pipefail

case "${1:-}" in
  read)
    file=${2:-Cargo.toml}
    version=$(awk '
      { sub(/\r$/, "") }
      /^\[workspace\.package\]/ { in_pkg = 1; next }
      /^\[/ { in_pkg = 0 }
      in_pkg && /^version[[:space:]]*=/ {
        sub(/^[^"]*"/, ""); sub(/".*$/, ""); print; exit
      }' "$file")
    if [ -z "$version" ]; then
      echo "version.sh: no [workspace.package] version in $file" >&2
      exit 1
    fi
    printf '%s\n' "$version"
    ;;
  changelog)
    version=${2:?usage: version.sh changelog X.Y.Z [CHANGELOG.md]}
    file=${3:-CHANGELOG.md}
    body=$(awk -v want="## [$version]" '
      { sub(/\r$/, "") }
      index($0, want) == 1 { found = 1; next }
      found && /^## \[/ { exit }
      found { print }' "$file" | sed '/./,$!d')
    if [ -z "${body//[[:space:]]/}" ]; then
      echo "version.sh: $file has no non-empty '## [$version]' section" >&2
      exit 1
    fi
    printf '%s\n' "$body"
    ;;
  *)
    echo "usage: version.sh read [Cargo.toml] | changelog X.Y.Z [CHANGELOG.md]" >&2
    exit 2
    ;;
esac
