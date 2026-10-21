#!/usr/bin/env bash
# Makes the README's images in docs/images/: a demo vault of SolidCraft's own sample designs,
# with thumbnails SolidCraft renders, photographed by the GUI test apps/scpdm-gui/tests/readme.rs,
# and a cover from the first screenshot.
#
#   SOLIDCRAFT_CLI=/path/to/solidcraft-cli docs/make-screenshots.sh [solidcraft-checkout]
#
# solidcraft-cli: from an extracted SolidCraft AppImage (`--appimage-extract`, then
# squashfs-root/usr/bin/solidcraft-cli). The checkout (cloned into target/ if not given) has the
# sample designs (MIT/Apache-2.0). Needs Python with Pillow, and a GPU or software renderer.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cli="${SOLIDCRAFT_CLI:?set SOLIDCRAFT_CLI to solidcraft-cli}"
upstream="${1:-$root/target/upstream/solidcraft}"
if [ ! -d "$upstream" ]; then
  git clone --depth 1 https://github.com/tangency-labs/solidcraft.git "$upstream"
fi
input="$root/target/readme/input"
rm -rf "$input"
mkdir -p "$input" "$root/docs/images"

samples=(pi-case-0.8 pi-case issue-616-lid-ribs battery-tpu-case-v2 battery-tpu-case modern-plastic
  joints-hinge m0-box-fillet-hole modern-path-pattern modern-sheet-metal logo-up-down-join
  cubeish-chamfer-too-large crazy-geom-cuts)
for name in "${samples[@]}"; do
  design="$(find "$upstream" -name "$name.solidcraft" -not -path "*/vendor/*" | head -n1)"
  cp "$design" "$input/$name.solidcraft"
  "$cli" snapshot "$design" --out "$input/$name.png" --width 480 --height 300 --view iso --no-grid --no-axes >/dev/null
done
python3 -I "$root/docs/readme-images.py" sketch "$input/reference-sketch.png"

SCPDM_README_INPUT="$input" SCPDM_README_OUT="$root/docs/images" \
  cargo test --manifest-path "$root/Cargo.toml" -p scpdm-gui --test readme -- --nocapture
python3 -I "$root/docs/readme-images.py" cover "$root/docs/images/vault.png" "$root/docs/images/cover.png"
ls -la "$root/docs/images"
