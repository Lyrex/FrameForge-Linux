#!/usr/bin/env bash
# Repack an AppImage so its root FrameForge.desktop and .DirIcon are regular
# files rather than the symlinks linuxdeploy leaves behind.
#
# Launcher integration tools (shelly, appimaged and friends) read exactly
# those two root entries by partial extraction, where a symlink resolves to
# nothing. Worse, linuxdeploy's .DirIcon symlink is absolute into the build
# tree, so it is broken on every machine except the build host. The result is
# a nameless, iconless launcher entry. There is no Tauri hook between
# linuxdeploy and the squashfs packing, hence the repack after the fact.
set -euo pipefail

if [ $# -ne 1 ] || [ ! -f "$1" ]; then
  echo "usage: $0 <AppImage>" >&2
  exit 1
fi
appimage=$(realpath "$1")

if ! command -v mksquashfs > /dev/null; then
  echo "mksquashfs not found - install squashfs-tools" >&2
  exit 1
fi

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT
cd "$workdir"

# Everything before the squashfs is the runtime, reused byte-for-byte except
# its .upd_info ELF section, which Tauri leaves zeroed. Updaters
# (shelly, AppImageUpdate) read that section to learn where updates live; a
# zeroed section means "not updatable". Patch the string in with dd at the
# section's file offset rather than objcopy: the section already exists at a
# fixed size, so an in-place write cannot disturb the ELF layout.
offset=$("$appimage" --appimage-offset)
head -c "$offset" "$appimage" > runtime

updinfo='gh-releases-zsync|Lyrex|FrameForge-Linux|latest|FrameForge_*_amd64.AppImage.zsync'
sections=$(readelf -S runtime)
updinfo_offset=$(awk '/\.upd_info/{print $NF}' <<< "$sections")
updinfo_size=$(awk 'found{print $1; exit} /\.upd_info/{found=1}' <<< "$sections")
updinfo_offset=$((16#$updinfo_offset))
updinfo_size=$((16#$updinfo_size))
[ ${#updinfo} -lt "$updinfo_size" ]
# Zero the whole section first so a rerun over an already-patched image
# cannot leave a tail of a longer previous string behind.
dd if=/dev/zero of=runtime bs=1 seek="$updinfo_offset" count="$updinfo_size" \
  conv=notrunc status=none
printf '%s' "$updinfo" | dd of=runtime bs=1 seek="$updinfo_offset" \
  conv=notrunc status=none

"$appimage" --appimage-extract > /dev/null

# EGL comes from the host's Mesa, and Mesa built against wayland >= 1.23
# calls wl_display_create_queue_with_name. The libwayland-client that
# linuxdeploy bundles from the build distro predates that symbol, Mesa binds
# to the bundled copy, and EGL display creation fails with EGL_BAD_PARAMETER:
# WebKit's render process aborts and every window is blank. Same reason the
# AppImage excludelist bans exactly this library (mesa#11316); the sibling
# libwayland-* libs are fine to bundle and stay. -f, because a linuxdeploy
# with the updated excludelist will stop bundling it.
rm -f squashfs-root/usr/lib/libwayland-client.so.0

# The desktop file's real copy lives under usr/share/applications, and the
# root FrameForge.png is already a regular file, so both are safe sources.
rm squashfs-root/FrameForge.desktop squashfs-root/.DirIcon
cp squashfs-root/usr/share/applications/FrameForge.desktop squashfs-root/
cp squashfs-root/FrameForge.png squashfs-root/.DirIcon
grep -q '^Name=FrameForge$' squashfs-root/FrameForge.desktop

# Integration tools read the installed version from X-AppImage-Version;
# without it they fall back to parsing the filename, which shelly gets
# wrong. The version is only recorded in the Tauri-produced filename
# (FrameForge_<version>_amd64.AppImage), so recover it from there.
version=$(basename "$appimage")
version=${version#FrameForge_}
version=${version%_*}
[ -n "$version" ]
printf 'X-AppImage-Version=%s\n' "$version" >> squashfs-root/FrameForge.desktop

# Match the original filesystem's compression so the repack changes nothing
# but the two entries above.
comp=$(unsquashfs -s -offset "$offset" "$appimage" | awk '/^Compression/{print $2}')
mksquashfs squashfs-root filesystem.img \
  -comp "$comp" -b 131072 -noappend -all-root -quiet > /dev/null

cat runtime filesystem.img > repacked
chmod 755 repacked
mv repacked "$appimage"

# The failure this script exists for is a root entry a partial extraction
# cannot read, so that is what gets verified.
mkdir verify && cd verify
"$appimage" --appimage-extract FrameForge.desktop > /dev/null
"$appimage" --appimage-extract .DirIcon > /dev/null
grep -q '^Name=FrameForge$' squashfs-root/FrameForge.desktop
grep -q "^X-AppImage-Version=$version\$" squashfs-root/FrameForge.desktop
[ -s squashfs-root/.DirIcon ] && [ ! -L squashfs-root/.DirIcon ]
objcopy -O binary --only-section=.upd_info "$appimage" updinfo.bin
grep -q 'gh-releases-zsync' updinfo.bin

echo "repacked: $appimage"
