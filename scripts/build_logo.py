"""Vectorise the cropped Gemini PNG directly using potrace.

Strategy:
1. Take the cropped logo PNG.
2. Build two masks: light-blue pixels (hex border) and dark-navy pixels
   (R, >_, wordmark).
3. Run potrace on each mask separately.
4. Compose into a multi-colored SVG: light blue for the hex outline,
   dark navy for the type. Background stays white.

Result: a vector that's geometrically identical to the source raster,
without depending on any specific font installation.
"""
import os
import re
import subprocess
import tempfile
from PIL import Image

# Hybrid approach:
# - Trace the hex border + "R" + ">_" directly from the Gemini source raster
#   (full resolution, exact geometry).
# - Render the "rstudio-cli" wordmark from Inter-Bold via Pillow + potrace
#   for crisp, typographically clean letters (tracing the AI-rendered raster
#   produced visible irregularities).
ORIG_SRC = "/home/endreas/code/aclemen1/rstudio-cli/scratch/Gemini_Generated_Image_286trn286trn286t.png"
CROP_X = 736
CROP_Y = 30
CROP_W = 1280
CROP_H = 1480
# Rectangular region (in cropped image px) covering the original wordmark.
# Pixels inside this rect are excluded from BOTH masks (the wordmark itself
# AND its antialiased blue halo, which would otherwise be picked up by the
# border mask and produce a ghost). The hex border doesn't intersect this
# rect, so the lower half of the hex outline is preserved.
# Covers the wordmark band exactly. Inset on all sides to keep clear of the
# hex border's antialiased halo (the hex right vertical edge sits around
# x=1170; antialiased halo extends inward to ~x=1130, so we cap x_max at
# 1080 for safety). The wordmark glyphs themselves span x=265..1015, so a
# 220..1080 horizontal band is plenty.
WORDMARK_RECT = (220, 870, 1080, 1050)

OUT = "/home/endreas/code/aclemen1/rstudio-cli/assets/logo.svg"

COLOR_BORDER = "#4878B0"
COLOR_TYPE = "#1A3654"
COLOR_FILL = "#F1F2F5"  # very light grey interior
HEX_STROKE_WIDTH = 50    # thicker than the original Gemini stroke (~30 px)

FONT_INTER_BOLD = "/nix/store/3cfynhzaw9nlk3afzg6vfbrvpbrx6fjz-inter-68966-tex/fonts/opentype/public/inter/Inter-Bold.otf"


def trace_mask(mask_img: Image.Image) -> tuple[str, str, int, int]:
    """Run potrace on a 1-bit mask and return (path_d, transform, width, height)."""
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
    """Render text as black-on-white bitmap suitable for potrace."""
    from PIL import ImageDraw, ImageFont
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
    raw = Image.open(ORIG_SRC).convert("RGB")
    img = raw.crop((CROP_X, CROP_Y, CROP_X + CROP_W, CROP_Y + CROP_H))
    W, H = img.size
    print(f"source: {W}x{H} (cropped from {raw.size})")

    # Build two binary masks: border (light blue) and type (dark navy).
    # For the type mask we omit pixels below WORDMARK_Y_CUTOFF — that's the
    # wordmark, which we render from a font for cleaner glyphs.
    border_mask = Image.new("1", (W, H), 1)
    type_mask = Image.new("1", (W, H), 1)

    bm_pix = border_mask.load()
    tm_pix = type_mask.load()
    wm_x0, wm_y0, wm_x1, wm_y1 = WORDMARK_RECT
    for y in range(H):
        in_wm_y = wm_y0 <= y <= wm_y1
        for x in range(W):
            # Skip the entire wordmark rectangle in both masks — we'll render
            # a fresh wordmark from a font on top.
            if in_wm_y and wm_x0 <= x <= wm_x1:
                continue
            r, g, b = img.getpixel((x, y))
            is_border = (b > r + 25) and (b > 100) and (b < 220) and (r < 150)
            is_type = (r < 90) and (g < 100) and (b < 150) and (b >= r)
            if is_border:
                bm_pix[x, y] = 0
            elif is_type:
                tm_pix[x, y] = 0

    # Trace from PNG: hex border + R + >_
    border_mask.save("/tmp/mask_border.png")
    type_mask.save("/tmp/mask_type.png")
    border_d, border_t, _, _ = trace_mask(border_mask)
    type_d, type_t, _, _ = trace_mask(type_mask)

    # Render wordmark from font + trace. Size chosen to roughly match the
    # original wordmark's footprint inside the hex.
    wordmark_pbm = render_text_to_pbm("rstudio-cli", FONT_INTER_BOLD, 1200)
    wordmark_d, wordmark_t, ww, wh = trace_mask(wordmark_pbm)

    # Position wordmark inside the hex, matching the original's y-band and
    # width. Original wordmark spans roughly 670 px wide and sits at y≈830.
    target_w_px = 670
    scale = target_w_px / ww
    wm_h_scaled = wh * scale
    wm_x = (W - target_w_px) / 2
    wm_y = 830

    # Hex polygon vertices (points-up, inscribed in the source-image bbox of
    # the original Gemini raster). The polygon underlies the trace and
    # provides the light-grey interior fill plus a slightly wider stroke;
    # the existing trace overlays the polygon's stroke at the original
    # border location.
    hex_pts = "640,140 1172,453 1172,1079 640,1392 108,1079 108,453"

    svg = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}">
  <!-- rstudio-cli logo: hex outline + R + >_ traced from the Gemini source
       raster (preserves original geometry exactly); the "rstudio-cli"
       wordmark is rendered from Inter-Bold and traced for crisp typography.
       Self-contained vector — no font dependency at render time.
       Programmatic hex polygon underneath provides the light-grey interior
       fill and a slightly wider blue stroke (the traced ring overlays it). -->
  <polygon points="{hex_pts}"
           fill="{COLOR_FILL}" stroke="{COLOR_BORDER}" stroke-width="{HEX_STROKE_WIDTH}" stroke-linejoin="miter"/>
  <g transform="{border_t}" fill="{COLOR_BORDER}"><path d="{border_d}"/></g>
  <g transform="{type_t}" fill="{COLOR_TYPE}"><path d="{type_d}"/></g>
  <g transform="translate({wm_x:.2f},{wm_y:.2f}) scale({scale:.6f}) {wordmark_t}" fill="{COLOR_TYPE}"><path d="{wordmark_d}"/></g>
</svg>
"""
    with open(OUT, "w") as f:
        f.write(svg)
    print(f"wrote {OUT} ({len(svg)} bytes)")


if __name__ == "__main__":
    main()
