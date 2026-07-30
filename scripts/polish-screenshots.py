"""Turn raw 1920x1080 app captures into README-ready cards.

Raw captures have hard 90-degree corners, which read as "screenshot" rather than
"product". This downscales to a size GitHub will not re-compress into mush and
rounds the corners with a real alpha mask, so the card floats on both the light
and dark README background.

Pairs with scripts/capture-screenshots.mjs, whose output has no window chrome.
Pass --titlebar 28 for a hand-taken Windows capture that still has the caption
strip on it.

Usage:
    python scripts/polish-screenshots.py [--src docs/assets] [--out docs/assets/shots]

Rerun it whenever the shots in --src are replaced.
"""

from __future__ import annotations

import argparse
from pathlib import Path

from PIL import Image, ImageDraw

TARGET_WIDTH = 1280
CORNER_RADIUS = 16


def polish(src: Path, dst: Path, titlebar_px: int) -> tuple[int, int]:
    im = Image.open(src).convert("RGB")

    if titlebar_px:
        # Scaled in case the capture was not 1920 wide.
        crop_top = round(titlebar_px * im.width / 1920)
        im = im.crop((0, crop_top, im.width, im.height))

    scale = TARGET_WIDTH / im.width
    im = im.resize((TARGET_WIDTH, round(im.height * scale)), Image.LANCZOS)

    mask = Image.new("L", im.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (0, 0, im.width - 1, im.height - 1), radius=CORNER_RADIUS, fill=255
    )
    out = im.convert("RGBA")
    out.putalpha(mask)

    dst.parent.mkdir(parents=True, exist_ok=True)
    out.save(dst, "WEBP", quality=88, method=6)
    return src.stat().st_size, dst.stat().st_size


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", default="docs/assets")
    ap.add_argument("--out", default="docs/assets/shots")
    ap.add_argument(
        "--titlebar",
        type=int,
        default=0,
        help="rows of window chrome to crop off the top (28 for a raw Windows capture)",
    )
    args = ap.parse_args()

    src_dir, out_dir = Path(args.src), Path(args.out)
    shots = sorted(p for p in src_dir.glob("screenshot-*.png"))
    if not shots:
        raise SystemExit(f"no screenshot-*.png found in {src_dir}")

    for shot in shots:
        name = shot.stem.removeprefix("screenshot-")
        before, after = polish(shot, out_dir / f"{name}.webp", args.titlebar)
        print(f"{name:14s} {before // 1024:5d} KB -> {after // 1024:4d} KB")


if __name__ == "__main__":
    main()
