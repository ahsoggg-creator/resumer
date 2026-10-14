"""Draws the Resumer icon (a "play" triangle in a ring) into crates/app/assets: icon.ico, icon.png, icon64.rgba."""
from pathlib import Path
from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent.parent / "crates" / "app" / "assets"
BG, RING, FG = (30, 30, 46, 255), (203, 166, 247, 255), (166, 227, 161, 255)


def draw(size: int) -> Image.Image:
    s = size * 4
    im = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    pad = s * 0.04
    d.rounded_rectangle([pad, pad, s - pad, s - pad], radius=s * 0.22, fill=BG)
    ring_w = s * 0.07
    c = s * 0.5
    r = s * 0.31
    d.ellipse([c - r, c - r, c + r, c + r], outline=RING, width=int(ring_w))
    t = s * 0.15
    d.polygon([(c - t * 0.55, c - t * 1.15), (c - t * 0.55, c + t * 1.15), (c + t * 1.25, c)], fill=FG)
    return im.resize((size, size), Image.LANCZOS)


OUT.mkdir(parents=True, exist_ok=True)
big = draw(256)
big.save(OUT / "icon.png")
big.save(OUT / "icon.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
(OUT / "icon64.rgba").write_bytes(draw(64).tobytes())
print("icons written to", OUT)
