"""Turn release tour stills into the release-page video.

Input is the 3200x1800 captures from scripts/capture-release-tour.mjs plus a
milestone card from scripts/build-release-cards.mjs. Each slide is composed at
full output size with a caption pill, then ffmpeg crossfades them. Nothing is
upscaled: the stills are larger than every output size.

Outputs (in --out):
    <name>-1440p.mp4  2560x1440, for the release page and download
    <name>-1080p.mp4  1920x1080, small enough for a GitHub comment embed

Usage:
    python scripts/build-release-tour.py --src <raw dir> \
        --card frontend/static/social/release-1-0-video.png --out release-media
"""

from __future__ import annotations

import argparse
import subprocess
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

FONT_BOLD = "C:/Windows/Fonts/segoeuib.ttf"
FONT_REGULAR = "C:/Windows/Fonts/segoeui.ttf"
INK = (10, 10, 10)
TEAL = (45, 212, 212)

HOLD_S = 3.2
CARD_S = 3.4
FADE_S = 0.6
FPS = 30

# (still, title, line). Order tells the story: home, finding music, the new
# detail pages, video, the maps, then the themes as a closer.
SLIDES = [
    ("home-dark", "Home", "Jump back in, mixes and your recommendations up front"),
    ("search-light", "Search", "The top result beside the five best songs. Enter plays it."),
    ("artist-dark", "Artist pages", "One action bar, counts once, every row with its duration"),
    ("album-light", "Album pages", "Liner notes: tracks grouped by work, nothing repeated"),
    ("library-dark", "Library", "A command header, tab counts and an A to Z index"),
    ("mix-light", "Mix", "Automix and DJ transitions on one page"),
    ("stations-dark", "Video stations", "A fresh lineup every day, drawn from your taste"),
    ("videos-light", "Your music videos", "Every video for the songs you love, on one wall"),
    ("galaxy-dark", "Genre Galaxy", "Your whole library as a star field"),
    ("charts-light", "Charts", "Instant charts, refreshed daily"),
    ("space-dark", "Sound Space", "Discovery that branches from any song"),
    ("analytics-light", "Analytics", "Listening pulse, rhythm and your routine"),
]


def font(path: str, size: int) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(path, size)


def fill(im: Image.Image, w: int, h: int) -> Image.Image:
    scale = max(w / im.width, h / im.height)
    im = im.resize((round(im.width * scale), round(im.height * scale)), Image.LANCZOS)
    left, top = (im.width - w) // 2, (im.height - h) // 2
    return im.crop((left, top, left + w, top + h))


def caption(base: Image.Image, title: str, line: str) -> Image.Image:
    w, h = base.size
    u = h / 1080
    tf, lf = font(FONT_BOLD, round(34 * u)), font(FONT_REGULAR, round(26 * u))
    probe = ImageDraw.Draw(base)
    tw = probe.textlength(title, font=tf)
    lw = probe.textlength(line, font=lf)
    pad_x, pad_y, gap = round(34 * u), round(20 * u), round(22 * u)
    bar = round(5 * u)
    pw = round(pad_x * 2 + bar + gap + tw + gap * 1.4 + lw)
    ph = round(pad_y * 2 + 40 * u)
    px, py = (w - pw) // 2, h - ph - round(44 * u)

    # Soft shadow, then a near-opaque pill so the caption reads on any page.
    shadow = Image.new("RGBA", base.size, (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle(
        (px, py + round(10 * u), px + pw, py + ph + round(10 * u)), radius=ph // 2, fill=(0, 0, 0, 150)
    )
    out = Image.alpha_composite(base.convert("RGBA"), shadow.filter(ImageFilter.GaussianBlur(round(18 * u))))
    layer = Image.new("RGBA", base.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    d.rounded_rectangle((px, py, px + pw, py + ph), radius=ph // 2, fill=(12, 12, 14, 232))
    d.rounded_rectangle(
        (px, py, px + pw, py + ph), radius=ph // 2, outline=(255, 255, 255, 38), width=max(1, round(1.5 * u))
    )
    cy = py + ph / 2
    x = px + pad_x
    d.rounded_rectangle((x, cy - 15 * u, x + bar, cy + 15 * u), radius=bar, fill=TEAL + (255,))
    x += bar + gap
    d.text((x, cy), title, font=tf, fill=(255, 255, 255, 255), anchor="lm")
    x += tw + gap * 1.4
    d.text((x, cy), line, font=lf, fill=(255, 255, 255, 190), anchor="lm")
    return Image.alpha_composite(out, layer).convert("RGB")


def card_slide(card: Path, w: int, h: int) -> Image.Image:
    im = Image.open(card).convert("RGB")
    canvas = Image.new("RGB", (w, h), INK)
    scale = min(w / im.width, h / im.height)
    im = im.resize((round(im.width * scale), round(im.height * scale)), Image.LANCZOS)
    canvas.paste(im, ((w - im.width) // 2, (h - im.height) // 2))
    return canvas


def encode(frames: list[tuple[Path, float]], dst: Path, crf: int) -> None:
    args = ["ffmpeg", "-y", "-hide_banner", "-loglevel", "error"]
    for path, dur in frames:
        args += ["-loop", "1", "-framerate", str(FPS), "-t", f"{dur:.3f}", "-i", str(path)]
    chains, prev, offset = [], "[0:v]", 0.0
    for i in range(1, len(frames)):
        offset += frames[i - 1][1] - FADE_S
        out = f"[x{i}]"
        chains.append(f"{prev}[{i}:v]xfade=transition=fade:duration={FADE_S}:offset={offset:.3f}{out}")
        prev = out
    # Fade in from and out to the card's ink.
    total = sum(d for _, d in frames) - FADE_S * (len(frames) - 1)
    chains.append(f"{prev}fade=t=in:st=0:d=0.5,fade=t=out:st={total - 0.7:.3f}:d=0.7,format=yuv420p[v]")
    args += ["-filter_complex", ";".join(chains), "-map", "[v]", "-r", str(FPS)]
    args += ["-c:v", "libx264", "-preset", "slow", "-crf", str(crf), "-tune", "stillimage"]
    args += ["-movflags", "+faststart", str(dst)]
    subprocess.run(args, check=True)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", required=True)
    ap.add_argument("--card", required=True)
    ap.add_argument("--out", default="docs/assets/video")
    ap.add_argument("--name", default="noorwave-1-0-tour")
    args = ap.parse_args()
    src, out = Path(args.src), Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    for (w, h), crf, label in (((2560, 1440), 18, "1440p"), ((1920, 1080), 24, "1080p")):
        with tempfile.TemporaryDirectory() as tmp:
            tmpdir = Path(tmp)
            frames: list[tuple[Path, float]] = []
            card = tmpdir / "card.png"
            card_slide(Path(args.card), w, h).save(card)
            frames.append((card, CARD_S))
            for i, (name, title, line) in enumerate(SLIDES):
                p = tmpdir / f"{i:02d}.png"
                caption(fill(Image.open(src / f"{name}.png").convert("RGB"), w, h), title, line).save(p)
                frames.append((p, HOLD_S))
            frames.append((card, CARD_S))
            dst = out / f"{args.name}-{label}.mp4"
            encode(frames, dst, crf)
            print(f"{dst}  {dst.stat().st_size / 1e6:.1f} MB")


if __name__ == "__main__":
    main()
