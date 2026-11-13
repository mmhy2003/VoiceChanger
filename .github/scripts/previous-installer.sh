#!/usr/bin/env bash
# Downloads the installer of the latest published release into previous/ for the upgrade test,
# and writes its path to $GITHUB_OUTPUT as `path`. Before the first release it writes nothing.
set -euo pipefail
tag=$(gh release view --json tagName -q .tagName 2>/dev/null || true)
if [ -z "$tag" ]; then
  echo "no published release yet: the upgrade test installs this build over itself"
  exit 0
fi
mkdir -p previous
gh release download "$tag" --pattern 'Setup-VoiceChanger-*.exe' --dir previous --clobber
path=$(ls previous/Setup-VoiceChanger-*.exe | head -n 1)
echo "upgrade test starts from $tag ($path)"
echo "path=$path" >> "$GITHUB_OUTPUT"
