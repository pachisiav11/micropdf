"""Writes extension/public/icons/{16,32,48,128}.png: a page with a folded corner on an accent tile."""
from pathlib import Path

from PIL import Image, ImageDraw

ACCENT = (74, 91, 212, 255)
PAGE = (255, 255, 255, 255)
FOLD = (200, 207, 240, 255)
LINE = (141, 156, 255, 255)
SIZES = (16, 32, 48, 128)
# Drawn large and scaled down, so small sizes stay smooth.
BIG = 512

out = Path(__file__).resolve().parent.parent / "extension" / "public" / "icons"
out.mkdir(parents=True, exist_ok=True)

big = Image.new("RGBA", (BIG, BIG), (0, 0, 0, 0))
d = ImageDraw.Draw(big)
d.rounded_rectangle((16, 16, BIG - 16, BIG - 16), radius=96, fill=ACCENT)

left, top, right, bottom, fold = 136, 96, 376, 416, 80
d.polygon(
    [(left, top), (right - fold, top), (right, top + fold), (right, bottom), (left, bottom)],
    fill=PAGE,
)
d.polygon([(right - fold, top), (right - fold, top + fold), (right, top + fold)], fill=FOLD)
for i, width in enumerate((160, 160, 112)):
    y = 232 + i * 56
    d.rounded_rectangle((left + 40, y, left + 40 + width, y + 24), radius=12, fill=LINE)

for size in SIZES:
    big.resize((size, size), Image.LANCZOS).save(out / f"{size}.png", optimize=True)
    print(out / f"{size}.png")
