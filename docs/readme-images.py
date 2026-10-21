#!/usr/bin/env python3
"""Images for the README (needs Pillow; used by docs/make-screenshots.sh):

    readme-images.py sketch OUT.png              a dimension sketch, the demo's reference image
    readme-images.py cover SCREENSHOT OUT.png    the cover: icon, name, and a screenshot
"""
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parent.parent
FONT_DIRS = ["/usr/share/fonts/truetype/ubuntu", "/usr/share/fonts/truetype/dejavu", "/usr/share/fonts/TTF"]


def font(names, size):
    for d in FONT_DIRS:
        for n in names:
            p = Path(d) / n
            if p.exists():
                return ImageFont.truetype(str(p), size)
    return ImageFont.load_default(size)


def sketch(out):
    """Raspberry Pi 4 board outline with its main dimensions, like a photographed drawing."""
    w, h = 1200, 800
    img = Image.new("RGB", (w, h), "#f4f1e8")
    d = ImageDraw.Draw(img)
    for y in range(0, h, 40):  # squared paper
        d.line([(0, y), (w, y)], fill="#dfe6ec", width=1)
    for x in range(0, w, 40):
        d.line([(x, 0), (x, h)], fill="#dfe6ec", width=1)
    ink, f, small = "#24324f", font(["Ubuntu-M.ttf", "DejaVuSans.ttf"], 30), font(["Ubuntu-R.ttf", "DejaVuSans.ttf"], 24)
    x0, y0, x1, y1 = 200, 180, 1000, 640  # 85 x 56 mm at ~9.4 px/mm
    d.rounded_rectangle([x0, y0, x1, y1], radius=28, outline=ink, width=5)
    for cx, cy in [(x0 + 33, y0 + 33), (x0 + 33 + 545, y0 + 33), (x0 + 33, y1 - 33), (x0 + 33 + 545, y1 - 33)]:
        d.ellipse([cx - 13, cy - 13, cx + 13, cy + 13], outline=ink, width=4)  # M2.5 holes
    d.rectangle([x0 + 60, y0 - 4, x0 + 560, y0 + 46], outline=ink, width=3)  # GPIO header
    for i, (yy, label) in enumerate([(y0 + 40, "USB 3"), (y0 + 170, "USB 2"), (y0 + 310, "LAN")]):
        d.rectangle([x1 - 30, yy, x1 + 40, yy + 110], outline=ink, width=4)
        d.text((x1 + 50, yy + 40), label, fill=ink, font=small)
    d.line([(x0, y1 + 60), (x1, y1 + 60)], fill="#c0392b", width=3)  # dimensions
    d.line([(x0, y1 + 40), (x0, y1 + 80)], fill="#c0392b", width=3)
    d.line([(x1, y1 + 40), (x1, y1 + 80)], fill="#c0392b", width=3)
    d.text(((x0 + x1) / 2 - 40, y1 + 75), "85 mm", fill="#c0392b", font=f)
    d.line([(x0 - 60, y0), (x0 - 60, y1)], fill="#c0392b", width=3)
    d.line([(x0 - 80, y0), (x0 - 40, y0)], fill="#c0392b", width=3)
    d.line([(x0 - 80, y1), (x0 - 40, y1)], fill="#c0392b", width=3)
    d.text((x0 - 175, (y0 + y1) / 2 - 15), "56 mm", fill="#c0392b", font=f)
    d.text((x0 + 120, y0 + 200), "holes 58 x 49 mm, M2.5", fill=ink, font=f)
    d.text((40, 40), "Raspberry Pi 4 · board", fill=ink, font=font(["Ubuntu-B.ttf", "DejaVuSans-Bold.ttf"], 40))
    img.rotate(-1.2, resample=Image.BICUBIC, fillcolor="#f4f1e8").save(out)


def cover(screenshot, out):
    w, h = 1600, 640
    bg = Image.new("RGB", (w, h), "#141b29")
    d = ImageDraw.Draw(bg)
    for y in range(h):  # a soft vertical gradient
        t = y / h
        d.line([(0, y), (w, y)], fill=(int(20 + 14 * t), int(27 + 16 * t), int(41 + 24 * t)))
    icon = Image.open(ROOT / "assets" / "solidcraft-pdm-256.png").convert("RGBA").resize((176, 176), Image.LANCZOS)
    bg.paste(icon, (80, 120), icon)
    title = font(["Ubuntu-B.ttf", "DejaVuSans-Bold.ttf"], 74)
    sub = font(["Ubuntu-R.ttf", "DejaVuSans.ttf"], 30)
    d.text((80, 330), "SolidCraft PDM", fill="#f2f5fa", font=title)
    lines = ["Folders, versions, thumbnails, search", "and releases for your SolidCraft designs.", "Native on Linux, in Rust."]
    for i, line in enumerate(lines):
        d.text((84, 430 + i * 42), line, fill="#aab6cc", font=sub)

    shot = Image.open(screenshot).convert("RGB")
    sw = 900
    shot = shot.resize((sw, int(shot.height * sw / shot.width)), Image.LANCZOS)
    x, y = w - sw - 60, 70
    mask = Image.new("L", shot.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, shot.width, shot.height], radius=18, fill=255)
    shadow = Image.new("RGBA", (shot.width + 80, shot.height + 80), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle([40, 50, shot.width + 40, shot.height + 50], radius=24, fill=(0, 0, 0, 150))
    shadow = shadow.filter(ImageFilter.GaussianBlur(22))
    bg.paste(shadow, (x - 40, y - 40), shadow)
    bg.paste(shot, (x, y), mask)
    # Fade the screenshot out at the bottom edge of the cover.
    fade = Image.new("RGBA", (w, 120), (0, 0, 0, 0))
    fd = ImageDraw.Draw(fade)
    for i in range(120):
        t = i / 119
        fd.line([(0, i), (w, i)], fill=(34, 43, 65, int(255 * t * t)))
    bg.paste(fade, (0, h - 120), fade)
    bg.save(out, optimize=True)


if __name__ == "__main__":
    if sys.argv[1:2] == ["sketch"]:
        sketch(sys.argv[2])
    elif sys.argv[1:2] == ["cover"]:
        cover(sys.argv[2], sys.argv[3])
    else:
        sys.exit(__doc__)
