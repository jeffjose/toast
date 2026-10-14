"""Builds toast's icon font from Solar's linear icons, drawn at a heavier
line than Solar's own: jog's tool, with toast's picks. An icon both have
is at the same code point in each.

    uv run --no-project --with fonttools --with picosvg \
        tools/solar_font.py ICONS_JSON tools/icons.txt assets/solar.ttf [STROKE]

ICONS_JSON is the set as Iconify publishes it:
https://cdn.jsdelivr.net/npm/@iconify-json/solar/icons.json

tools/icons.txt has a line for each icon toast uses: the name of its
constant in `canvas::icons`, and the code point the font has it at, in
hex. To add an icon, give it a line there with a code point of its own
and say in PICKS below which of Solar's drawings it is.

The line is 2.25 wide where Solar draws it at 1.5: beside text at 13 to
24 pixels the thinner line is lighter than the letters.
"""

import json
import re
import sys

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.cu2quPen import Cu2QuPen
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.svgLib.path import parse_path
from picosvg.svg import SVG

# toast's name for an icon, and the Solar drawing that stands for it.
PICKS = {
    "X": "close", "SEARCH": "magnifier", "BELL": "bell", "BELL_OFF": "bell-off",
    # Filled ones, taken as Solar draws them: named in full, style and all.
    "CHECK_FILLED": "check-circle-bold", "INFO_FILLED": "info-circle-bold",
    "ALERT_FILLED": "danger-triangle-bold", "DANGER_FILLED": "danger-circle-bold",
}

EM = 1000
GRID = 24


def outline(body, stroke):
    """The filled outline of a drawing made of lines `stroke` wide, as a
    glyph: Solar's 24 by 24 square taken to the font's em, the right way up."""
    body = re.sub(r'stroke-width="[0-9.]+"', f'stroke-width="{stroke}"', body).replace("currentColor", "#000")
    svg = SVG.fromstring(f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {GRID} {GRID}">{body}</svg>')
    pen = TTGlyphPen(None)
    scale = EM / GRID
    to_em = TransformPen(Cu2QuPen(pen, max_err=1.0, reverse_direction=False), (scale, 0, 0, -scale, 0, EM))
    for shape in svg.topicosvg().shapes():
        parse_path(shape.as_path().d, to_em)
    return pen.glyph()


def main():
    icons_json, consts, out = sys.argv[1:4]
    stroke = sys.argv[4] if len(sys.argv) > 4 else "2.25"
    solar = json.load(open(icons_json))["icons"]

    glyphs, cmap, missing = {".notdef": TTGlyphPen(None).glyph()}, {}, []
    for line in open(consts):
        name, point = line.split()
        pick = PICKS.get(name, "")
        drawing = solar.get(pick if pick.endswith("-bold") else pick + "-linear")
        if not drawing:
            missing.append(f"{name} ({PICKS.get(name, 'no pick')})")
            continue
        glyphs[name] = outline(drawing["body"], stroke)
        cmap[int(point, 16)] = name
    if missing:
        sys.exit(f"Solar has no drawing for: {', '.join(missing)}")

    font = FontBuilder(EM, isTTF=True)
    font.setupGlyphOrder(list(glyphs))
    font.setupCharacterMap(cmap)
    font.setupGlyf(glyphs)
    font.setupHorizontalMetrics({name: (EM, 0) for name in glyphs})
    font.setupHorizontalHeader(ascent=EM, descent=0)
    font.setupNameTable({"familyName": "toast icons", "styleName": "Regular"})
    font.setupOS2(sTypoAscender=EM, sTypoDescender=0, usWinAscent=EM, usWinDescent=0)
    font.setupPost()
    font.save(out)
    print(f"{len(glyphs) - 1} icons")


if __name__ == "__main__":
    main()
