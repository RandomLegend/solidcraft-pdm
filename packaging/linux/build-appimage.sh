#!/usr/bin/env bash
# Builds dist/SolidCraft-PDM-<version>-x86_64.AppImage: the release binaries, the desktop entry
# and the icon, packed with appimagetool. Used by CI (.gitlab-ci.yml) and locally:
#
#   packaging/linux/build-appimage.sh [output-dir]
#
# appimagetool is downloaded into target/appimage/ unless APPIMAGETOOL names one. It runs with
# APPIMAGE_EXTRACT_AND_RUN, so no FUSE is needed (CI containers have none).
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
out="${1:-$root/dist}"
work="$root/target/appimage"
appdir="$work/AppDir"
tool="${APPIMAGETOOL:-$work/appimagetool-x86_64.AppImage}"
tool_url="https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n1)"

cargo build --release --locked --manifest-path "$root/Cargo.toml" -p scpdm-gui -p scpdm

rm -rf "$appdir"
install -Dm755 "$root/target/release/solidcraft-pdm" "$appdir/usr/bin/solidcraft-pdm"
install -Dm755 "$root/target/release/scpdm" "$appdir/usr/bin/scpdm"
install -Dm755 "$root/packaging/linux/AppRun" "$appdir/AppRun"
install -Dm644 "$root/packaging/linux/solidcraft-pdm.desktop" "$appdir/usr/share/applications/solidcraft-pdm.desktop"
install -Dm644 "$root/assets/solidcraft-pdm.svg" "$appdir/usr/share/icons/hicolor/scalable/apps/solidcraft-pdm.svg"
cp "$appdir/usr/share/applications/solidcraft-pdm.desktop" "$appdir/"
cp "$root/assets/solidcraft-pdm.svg" "$appdir/solidcraft-pdm.svg"
cp "$root/assets/solidcraft-pdm.svg" "$appdir/.DirIcon"

if [ ! -x "$tool" ]; then
  mkdir -p "$(dirname "$tool")"
  curl -fsSL -o "$tool" "$tool_url"
  chmod +x "$tool"
fi

mkdir -p "$out"
image="$out/SolidCraft-PDM-$version-x86_64.AppImage"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$tool" --no-appstream "$appdir" "$image"
echo "$image"
