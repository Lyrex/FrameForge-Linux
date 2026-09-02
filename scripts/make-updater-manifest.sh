#!/usr/bin/env bash
# Compose the updater's latest.json from the signatures sitting beside a
# release's artifacts.
#
# usage: make-updater-manifest.sh <tag> <sig-dir> <notes-file> <download-base-url>
#
# The bundler writes one `<artifact>.sig` per updatable bundle and names the
# artifact after the version, so the signature filenames are the only input
# needed: strip `.sig` for the asset name, and the extension tells us which
# platform the entry belongs to.
set -euo pipefail

if [ $# -ne 4 ]; then
    echo "usage: $0 <tag> <sig-dir> <notes-file> <download-base-url>" >&2
    exit 1
fi
tag="$1"
sig_dir="$2"
notes_file="$3"
base_url="${4%/}"

version="${tag#v}"

platforms='{}'
for sig in "$sig_dir"/*.sig; do
    [ -e "$sig" ] || continue
    asset=$(basename "${sig%.sig}")
    case "$asset" in
        # Linux x86_64 only: nothing else is built, and a key for a platform
        # with no artifact would offer an update the updater cannot download.
        *.AppImage) key='linux-x86_64' ;;
        *) continue ;;
    esac
    platforms=$(jq -n \
        --argjson acc "$platforms" \
        --arg key "$key" \
        --arg url "$base_url/$asset" \
        --arg signature "$(cat "$sig")" \
        '$acc + {($key): {url: $url, signature: $signature}}')
done

# An empty manifest would tell every user that they are up to date.
if [ "$(jq 'has("linux-x86_64")' <<<"$platforms")" != true ]; then
    echo "::error::no updater signature for linux-x86_64 in $sig_dir" >&2
    exit 1
fi

jq -n \
    --arg version "$version" \
    --rawfile notes "$notes_file" \
    --arg pub_date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --argjson platforms "$platforms" \
    '{version: $version, notes: $notes, pub_date: $pub_date, platforms: $platforms}'
