#!/usr/bin/env bash
# Write a new version into every file that carries one, then verify they
# agree. The version lives in five places with nothing tying them together:
# package.json, src-tauri/Cargo.toml, src-tauri/tauri.conf.json, the crate's
# own entry in src-tauri/Cargo.lock, and the README title. check-version.sh
# catches drift between the first three at release time; this script is how
# they move in lockstep in the first place.
set -euo pipefail

if [ $# -ne 1 ]; then
  echo "usage: $0 <version>   e.g. $0 2.8.0-linux.2" >&2
  exit 1
fi
new="$1"

# A regex rather than a case glob: globs would wave through trailing garbage
# like 2.9.0-linux.1junk.
if [[ ! "$new" =~ ^[0-9]+\.[0-9]+\.[0-9]+-linux\.[0-9]+$ ]]; then
  echo "version must look like <upstream>-linux.<n>, e.g. 2.8.0-linux.2" >&2
  exit 1
fi

root="$(dirname "$0")/.."

# First occurrence only, mirroring how check-version.sh reads each file:
# both JSON files carry other "version" keys further down (dependencies,
# bundle sections) that must not be touched.
sed -i "0,/\"version\": \".*\"/s//\"version\": \"$new\"/" "$root/package.json"
sed -i "0,/\"version\": \".*\"/s//\"version\": \"$new\"/" "$root/src-tauri/tauri.conf.json"
sed -i "0,/^version = \".*\"/s//version = \"$new\"/" "$root/src-tauri/Cargo.toml"

# The README's title names the current release; the other versions in its
# prose are worked examples and stay as written.
sed -i "s/Companion \`v[^\`]*\`/Companion \`v$new\`/" "$root/README.md"

# Resync the lock file's warframe-companion entry from the manifest. Offline:
# only the workspace member's version changes, no dependency needs resolving.
# Upstream once shipped a lock that lagged its manifest (v2.8.0); a --locked
# build catches that only after this script has already prevented it.
(cd "$root/src-tauri" && cargo update --workspace --offline --quiet)

bash "$root/scripts/check-version.sh" "$new"
grep -A1 'name = "warframe-companion"' "$root/src-tauri/Cargo.lock"
