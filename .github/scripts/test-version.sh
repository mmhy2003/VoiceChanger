#!/usr/bin/env bash
# Tests for version.sh. Run: bash .github/scripts/test-version.sh
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
v="$here/version.sh"
fx="$here/fixtures"
fail=0

check() { # name expected actual
  if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: expected [$2], got [$3]"; fail=1; fi
}
status() { "$@" > /dev/null 2>&1 && echo 0 || echo $?; }

check "read fixture skips [package]" "1.2.3" "$(bash "$v" read "$fx/Cargo.fixture.toml")"
check "read repo is X.Y.Z" "1" "$(bash "$v" read "$here/../../Cargo.toml" | grep -cE '^[0-9]+\.[0-9]+\.[0-9]+$')"
check "changelog section" "$(printf '### Added\n- Two.')" "$(bash "$v" changelog 1.2.3 "$fx/CHANGELOG.fixture.md")"
check "changelog last section" "$(printf '### Fixed\n- Three.')" "$(bash "$v" changelog 1.2.1 "$fx/CHANGELOG.fixture.md")"
check "changelog missing fails" "1" "$(status bash "$v" changelog 9.9.9 "$fx/CHANGELOG.fixture.md")"
check "changelog empty fails" "1" "$(status bash "$v" changelog 1.2.2 "$fx/CHANGELOG.fixture.md")"
check "changelog prefix not matched" "1" "$(status bash "$v" changelog 1.2 "$fx/CHANGELOG.fixture.md")"
repo_version=$(bash "$v" read "$here/../../Cargo.toml")
check "repo changelog has the Cargo version" "0" "$(status bash "$v" changelog "$repo_version" "$here/../../CHANGELOG.md")"
check "no command is usage error" "2" "$(status bash "$v")"
exit $fail
