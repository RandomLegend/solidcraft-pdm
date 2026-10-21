#!/usr/bin/env python3
"""Draws the app icon as PNGs and a Windows .ico from the shapes of assets/solidcraft-pdm.svg.

    python3 packaging/make-icons.py

Needs Pillow. The SVG is the source: keep the shapes below in step with it. Drawn at 4x and
scaled down, for smooth edges.
"""
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets"
SCALE = 4
SIZE = 256 * SCALE


def s(points):
    return [(x * SCALE, y * SCALE) for x, y in points]


def outline(draw, points, color, width):
    pts = s(points)
    draw.line(pts + [pts[0], pts[1]], fill=color, width=width * SCALE, joint="curve")


def icon():
    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([8 * SCALE, 8 * SCALE, 248 * SCALE, 248 * SCALE], radius=44 * SCALE, fill="#22304a")
    # Earlier versions, behind (as in the SVG, at 55 % and 80 % opacity over the background).
    outline(d, [(150, 40), (206, 66), (206, 124), (150, 150), (94, 124), (94, 66)], "#4d6590", 7)
    outline(d, [(136, 58), (192, 84), (192, 142), (136, 168), (80, 142), (80, 84)], "#7894c6", 7)
    # The current version.
    d.polygon(s([(120, 78), (178, 105), (120, 132), (62, 105)]), fill="#f0a050")
    d.polygon(s([(62, 105), (120, 132), (120, 196), (62, 169)]), fill="#c97a32")
    d.polygon(s([(120, 132), (178, 105), (178, 169), (120, 196)]), fill="#9c5a22")
    return img.resize((256, 256), Image.LANCZOS)


def main():
    big = icon()
    for size in (256, 128, 64, 48, 32, 16):
        big.resize((size, size), Image.LANCZOS).save(ASSETS / f"solidcraft-pdm-{size}.png")
    big.save(ASSETS / "solidcraft-pdm.ico", sizes=[(256, 256), (64, 64), (48, 48), (32, 32), (16, 16)])
    print("wrote", ", ".join(sorted(p.name for p in ASSETS.glob("solidcraft-pdm*") if p.suffix != ".svg")))


if __name__ == "__main__":
    main()
