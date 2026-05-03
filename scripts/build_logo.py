"""Build the rstudio-cli hexsticker logo as a self-contained SVG.

Strategy:
1. Hex border + interior fill: programmatic <polygon> with fill + stroke.
   Uniform thickness, easy to adjust (HEX_STROKE_WIDTH, COLOR_FILL).
2. R + ">_" : traced from `assets/logo.png` (the cropped Gemini raster) via
   potrace, preserving the original geometry. The R mask is restricted to
   a central rectangle so the hex border (same color as R) is excluded.
3. "rstudio-cli" wordmark: rendered fresh from Inter-Bold via Pillow, then
   traced — crisp typography, no font dependency at render time.

The output SVG embeds every glyph as a <path>, so it renders identically
across browsers, GitHub server-side, and CLI tools.
"""
import os
import re
import subprocess
import tempfile
from PIL import Image, ImageDraw, ImageFont

# Source raster (already cropped + resized to 800x925 — the original 2752x1536
# Gemini PNG is no longer in the tree).
SRC = "/home/endreas/code/aclemen1/rstudio-cli/assets/logo.png"
OUT = "/home/endreas/code/aclemen1/rstudio-cli/assets/logo.svg"

# Region (in source px) covering the R + ">_" group only — excludes the
# hex border perimeter (same blue color as the R) so we trace the R
# without dragging the hex into the path.
# R glyph footprint in source-px (avoids the hex border on the perimeter
# AND the ">_" halo to the right of R).
R_RECT = (220, 230, 450, 510)
# ">_" sits in the dark-navy mask but in the upper portion of the hex.
TYPE_RECT = (115, 75, 700, 525)
# Region covering the original wordmark (dark navy + blue antialiased halo).
# Pixels here are excluded from both masks — we render a fresh Inter-Bold
# wordmark on top.
WORDMARK_RECT = (140, 540, 680, 660)

COLOR_BORDER = "#4878B0"
COLOR_TYPE = "#1A3654"
COLOR_FILL = "#F1F2F5"     # very light grey interior
HEX_STROKE_WIDTH = 32       # uniform border thickness in source-px

# Hex polygon vertices (points-up, inscribed in the 800x925 source image).
# Computed so the polygon's centerline matches the original Gemini hex's
# centerline; with a stroke width of 32 the visible outer edge sits at
# the source's bbox edges.
HEX_POINTS = "400,90 728,283 728,672 400,866 72,672 72,283"

FONT_INTER_BOLD = "/nix/store/3cfynhzaw9nlk3afzg6vfbrvpbrx6fjz-inter-68966-tex/fonts/opentype/public/inter/Inter-Bold.otf"


def trace_mask(mask_img: Image.Image) -> tuple[str, str, int, int]:
    """Run potrace on a 1-bit mask and return (path_d, transform, w, h)."""
    with tempfile.TemporaryDirectory() as tmp:
        pbm = os.path.join(tmp, "in.pbm")
        svg = os.path.join(tmp, "out.svg")
        mask_img.save(pbm)
        subprocess.run(
            ["potrace", "-s", "-o", svg, "-t", "2", "-O", "0.6",
             "-a", "1.0", "--flat", pbm],
            check=True, capture_output=True,
        )
        with open(svg) as f:
            content = f.read()
    md = re.search(r'<path[^>]*\bd="([^"]+)"', content)
    mt = re.search(r'<g[^>]*transform="([^"]+)"', content)
    if not md:
        raise RuntimeError("no path produced by potrace")
    return md.group(1), mt.group(1) if mt else "", *mask_img.size


def render_text_to_pbm(text: str, font_path: str, size_pt: int) -> Image.Image:
    """Render text as a tight black-on-white bitmap suitable for potrace."""
    font = ImageFont.truetype(font_path, size_pt)
    bbox = font.getbbox(text)
    pad = 30
    w = bbox[2] - bbox[0] + 2 * pad
    h = bbox[3] - bbox[1] + 2 * pad
    img = Image.new("L", (w, h), 255)
    draw = ImageDraw.Draw(img)
    draw.text((pad - bbox[0], pad - bbox[1]), text, font=font, fill=0)
    return img.convert("1")


def main():
    img = Image.open(SRC).convert("RGB")
    W, H = img.size
    print(f"source: {W}x{H}")

    r_mask = Image.new("1", (W, H), 1)
    type_mask = Image.new("1", (W, H), 1)

    rm_pix = r_mask.load()
    tm_pix = type_mask.load()
    rx0, ry0, rx1, ry1 = R_RECT
    tx0, ty0, tx1, ty1 = TYPE_RECT
    for y in range(H):
        for x in range(W):
            r, g, b = img.getpixel((x, y))
            is_border_color = (b > r + 25) and (b > 100) and (b < 220) and (r < 150)
            is_navy = (r < 90) and (g < 100) and (b < 150) and (b >= r)
            # R mask: blue pixels in the central R rect only.
            if is_border_color and rx0 <= x <= rx1 and ry0 <= y <= ry1:
                rm_pix[x, y] = 0
            # Type mask: dark navy pixels in the upper region (covers ">_",
            # excludes the wordmark which is below).
            elif is_navy and tx0 <= x <= tx1 and ty0 <= y <= ty1:
                tm_pix[x, y] = 0

    r_d, r_t, _, _ = trace_mask(r_mask)
    type_d, type_t, _, _ = trace_mask(type_mask)

    wordmark_pbm = render_text_to_pbm("rstudio-cli", FONT_INTER_BOLD, 1200)
    wordmark_d, wordmark_t, ww, wh = trace_mask(wordmark_pbm)

    # Wordmark target footprint inside the hex (source-px).
    target_w_px = 420
    scale = target_w_px / ww
    wm_x = (W - target_w_px) / 2
    wm_y = 555

    svg = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}">
  <!-- rstudio-cli logo. The hex outline is a programmatic polygon (uniform
       stroke, light-grey interior). The R + ">_" group is traced from the
       Gemini source raster (geometry preserved), with the hex border
       excluded via R_RECT / TYPE_RECT so the polygon stroke is free of
       trace artifacts. The "rstudio-cli" wordmark is rendered fresh from
       Inter-Bold and traced for crisp typography. Self-contained vector,
       no font dependency at render time. -->
  <polygon points="{HEX_POINTS}"
           fill="{COLOR_FILL}" stroke="{COLOR_BORDER}" stroke-width="{HEX_STROKE_WIDTH}" stroke-linejoin="miter"/>
  <g transform="{r_t}" fill="{COLOR_BORDER}"><path d="{r_d}"/></g>
  <g transform="{type_t}" fill="{COLOR_TYPE}"><path d="{type_d}"/></g>
  <g transform="translate({wm_x:.2f},{wm_y:.2f}) scale({scale:.6f}) {wordmark_t}" fill="{COLOR_TYPE}"><path d="{wordmark_d}"/></g>
</svg>
"""
    with open(OUT, "w") as f:
        f.write(svg)
    print(f"wrote {OUT} ({len(svg)} bytes)")


if __name__ == "__main__":
    main()
