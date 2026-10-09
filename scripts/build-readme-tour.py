"""Build the README screenshot set and the animated tours from raw window captures.

GitHub READMEs cannot run JS, so "rotating" shots are animated WebP files with
crossfades baked in. Stills are emitted per theme so the README can swap them
with <picture> + prefers-color-scheme to match the reader's GitHub theme.

Raw captures are expected as <src>/<name>.webp|png. Hand-taken window captures
(2000 wide, caption strip on top) are cropped; pass --titlebar 0 for the
chromeless 3200x1800 shots from scripts/capture-release-tour.mjs.

Usage:
    python scripts/build-readme-tour.py [--src docs/assets/raw] [--out docs/assets/shots]
"""

from __future__ import annotations

import argparse
from pathlib import Path

from PIL import Image, ImageDraw

# Sized for 2x screens: the README shows stills at 900 and the tour at 960.
STILL_WIDTH = 1800
TOUR_WIDTH = 1600
CORNER_RADIUS = 16
# Caption strip on a 2000-wide Windows capture.
TITLEBAR_PX_AT_2000 = 24

HOLD_MS = 2600
FADE_FRAMES = 6
FADE_MS = 55

# Surface tour, video and genre first, alternating themes so the rotation also
# sells the theming.
TOUR = [
    "home-dark",
    "search-dark",
    "stations-dark",
    "videos-warm",
    "galaxy-dark",
    "artist-dark",
    "album-light",
    "library-dark",
    "analytics-warm",
    "home-light",
]
# Same screen, three palettes.
THEMES = ["home-dark", "home-warm", "home-light"]


def load(src: Path, name: str) -> Image.Image:
    for ext in (".webp", ".png"):
        p = src / f"{name}{ext}"
        if p.exists():
            im = Image.open(p).convert("RGB")
            crop = round(TITLEBAR_PX_AT_2000 * im.width / 2000)
            return im.crop((0, crop, im.width, im.height))
    raise SystemExit(f"missing raw capture: {name}")


def fit(im: Image.Image, width: int) -> Image.Image:
    return im.resize((width, round(im.height * width / im.width)), Image.LANCZOS)


def rounded(im: Image.Image) -> Image.Image:
    mask = Image.new("L", im.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (0, 0, im.width - 1, im.height - 1), radius=CORNER_RADIUS, fill=255
    )
    out = im.convert("RGBA")
    out.putalpha(mask)
    return out


def tour(src: Path, names: list[str], dst: Path) -> None:
    shots = [fit(load(src, n), TOUR_WIDTH) for n in names]
    frames: list[Image.Image] = []
    durations: list[int] = []
    for i, cur in enumerate(shots):
        frames.append(rounded(cur))
        durations.append(HOLD_MS)
        nxt = shots[(i + 1) % len(shots)]
        for step in range(1, FADE_FRAMES + 1):
            frames.append(rounded(Image.blend(cur, nxt, step / (FADE_FRAMES + 1))))
            durations.append(FADE_MS)
    frames[0].save(
        dst,
        "WEBP",
        save_all=True,
        append_images=frames[1:],
        duration=durations,
        loop=0,
        quality=82,
        method=6,
        # Every frame a keyframe: lossy inter-frame deltas stack up across the
        # crossfades and show as blocky colour glitches on GitHub.
        kmin=0,
        kmax=1,
        allow_mixed=False,
    )


def main() -> None:
    global TITLEBAR_PX_AT_2000
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", default="docs/assets/raw")
    ap.add_argument("--out", default="docs/assets/shots")
    ap.add_argument(
        "--titlebar",
        type=int,
        default=TITLEBAR_PX_AT_2000,
        help="caption strip rows at 2000 wide; 0 for scripts/capture-release-tour.mjs output",
    )
    args = ap.parse_args()
    TITLEBAR_PX_AT_2000 = args.titlebar
    src, out = Path(args.src), Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    for p in sorted([*src.glob("*.webp"), *src.glob("*.png")]):
        dst = out / f"{p.stem}.webp"
        rounded(fit(load(src, p.stem), STILL_WIDTH)).save(
            dst, "WEBP", quality=86, method=6
        )
        print(f"{p.stem:20s} {dst.stat().st_size // 1024:5d} KB")

    for name, names in (("tour", TOUR), ("themes", THEMES)):
        dst = out / f"{name}.webp"
        tour(src, names, dst)
        print(f"{name:20s} {dst.stat().st_size // 1024:5d} KB (animated)")


if __name__ == "__main__":
    main()
