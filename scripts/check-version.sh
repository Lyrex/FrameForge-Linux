#!/usr/bin/env bash
# Assert the three files carrying the version agree, and — when one is passed as
# $1 — that they agree with it too.
#
# The version is written out in three places with nothing tying them together.
# A drift is invisible until release time, when it surfaces as artifact
# filenames that contradict the release title, after the build has already run.
set -euo pipefail

expected="${1:-}"
root="$(dirname "$0")/.."

package_version=$(grep -m1 '"version"' "$root/package.json" | cut -d'"' -f4)
cargo_version=$(grep -m1 '^version = ' "$root/src-tauri/Cargo.toml" | cut -d'"' -f2)
tauri_version=$(grep -m1 '"version"' "$root/src-tauri/tauri.conf.json" | cut -d'"' -f4)

echo "package.json:     $package_version"
echo "Cargo.toml:       $cargo_version"
echo "tauri.conf.json:  $tauri_version"
if [ -n "$expected" ]; then
    echo "expected:         $expected"
fi

if [ "$cargo_version" != "$package_version" ] || [ "$cargo_version" != "$tauri_version" ]; then
    echo "::error::the three version files disagree"
    exit 1
fi

# Agreement alone would bless three files agreeing on garbage.
scheme='[0-9]+\.[0-9]+\.[0-9]+-linux\.[0-9]+'
if [[ ! "$cargo_version" =~ ^${scheme}$ ]]; then
    echo "::error::'$cargo_version' does not look like <upstream>-linux.<n>"
    exit 1
fi

readme_version=$(grep -m1 -oE "$scheme" "$root/README.md" || true)
if [ "$readme_version" != "$cargo_version" ]; then
    echo "::error::README says '$readme_version' but the tree is at $cargo_version"
    exit 1
fi

if [ -n "$expected" ] && [ "$expected" != "$cargo_version" ]; then
    echo "::error::expected $expected but the tree is at $cargo_version"
    exit 1
fi
