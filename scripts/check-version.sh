#!/usr/bin/env bash
# Assert the files carrying the version agree, and — when one is passed as $1 —
# that they agree with it too. Also rejects a version the updater cannot compare
# and a public key it cannot decode, both of which otherwise surface only after
# a full release build.
#
# The version is written out in several places with nothing tying them together.
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
scheme='[0-9]+\.[0-9]+\.[0-9]+'
if [[ ! "$cargo_version" =~ ^${scheme}$ ]]; then
    # The updater compares versions as semver, where a prerelease suffix sorts
    # below the bare version. Naming a platform there would make each platform's
    # release a downgrade of a bare version nothing ever publishes; platforms
    # are per-platform artifacts of one version instead.
    echo "::error::'$cargo_version' is not plain semver — a platform suffix such as '-linux.1' is not a version"
    exit 1
fi

# The bundler rejects an unusable key only after both platform builds ran.
pubkey=$(grep -m1 '"pubkey"' "$root/src-tauri/tauri.conf.json" | cut -d'"' -f4)
if [[ ! "$pubkey" =~ ^[A-Za-z0-9+/]+=*$ ]]; then
    echo "::error::the updater public key is not base64 — run scripts/setup-updater-signing.sh"
    exit 1
fi

readme_version=$(grep -m1 -oE "Companion \`v$scheme\`" "$root/README.md" | grep -oE "$scheme" || true)
if [ "$readme_version" != "$cargo_version" ]; then
    echo "::error::README says '$readme_version' but the tree is at $cargo_version"
    exit 1
fi

if [ -n "$expected" ] && [ "$expected" != "$cargo_version" ]; then
    echo "::error::expected $expected but the tree is at $cargo_version"
    exit 1
fi
