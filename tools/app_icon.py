"""Makes toast's own icon, the one a list of programs shows: Solar's
unread-notification square, from its bold duotone set, with the two
tones made a color and white as clutch's and hotshot's icons are, so
that it reads on a dark panel and on a light one alike. The dot that
says something is new is the amber clutch marks what has changed in.

    python3 tools/app_icon.py ICONS_JSON assets/toast.svg
    convert -background none -density 1024 assets/toast.svg \
        -resize 256x256 assets/toast.png

ICONS_JSON is the set as Iconify publishes it:
https://cdn.jsdelivr.net/npm/@iconify-json/solar/icons.json

The SVG is for `~/.local/share/icons/hicolor/scalable/apps/`, where
`toast setup` puts it; the PNG is what a notification of toast's own
shows, and the README.
"""

import json
import re
import sys

# The square, the lines drawn on it, and the dot.
BODY = "#8A6CFF"
MARKS = "#FFFFFF"
DOT = "#FFB020"


def main(icons_json, out):
    with open(icons_json) as file:
        body = json.load(file)["icons"]["notification-unread-lines-bold-duotone"]["body"]
    paths = re.findall(r"<path[^>]*/>", body)
    drawn = []
    for at, path in enumerate(paths):
        # The tone Solar draws at half strength is the body; the last
        # of the rest is the dot, and the others the lines.
        faint = 'opacity=".5"' in path
        path = path.replace(' opacity=".5"', "")
        color = BODY if faint else DOT if at == len(paths) - 1 else MARKS
        drawn.append(path.replace("<path", f'<path fill="{color}"', 1))
    # A little room round it, as hotshot's has, so that it is not hard
    # against the edge of whatever shows it.
    svg = f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="1 1 22 22">{"".join(drawn)}</svg>\n'
    with open(out, "w") as file:
        file.write(svg)
    print(f"{len(drawn)} paths")


if __name__ == "__main__":
    main(*sys.argv[1:3])
