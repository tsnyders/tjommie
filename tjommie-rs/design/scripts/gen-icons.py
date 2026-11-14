"""Write the desktop app's icons (tjommie-rs/design/icons) from the inline SVG symbols of an Aegis screen.

GPUI's svg() is a single-colour mask with a fixed stroke, so every Lucide icon is emitted at
the stroke widths the design uses (i/<width*100>/<name>.svg), and every bot avatar as
two layers (bot/<shape>/body.svg and eyes.svg), as aegis/README.md's rendering table says.
GPUI transforms an svg about the centre of its own bounds, so each shape also gets eyes-crop.svg,
cropped to the eyes' box (its viewBox is that box in the 40-unit avatar): a blink then scales the
eyes about their own centre.

Run (the chat screen has every icon and 7 avatars; the avatar card adds the pill):
  python -I tjommie-rs/design/scripts/gen-icons.py design/claude-design/aegis/screens/desktop-chat.html \
    design/claude-design/aegis/components/bots-avatar.html tjommie-rs/design/icons
The first file to define a symbol wins.
"""

import os
import re
import shutil
import sys

WIDTHS = (125, 175, 200, 220)
SYMBOL = re.compile(r'<symbol id="([^"]+)" viewBox="([^"]+)">(.*?)</symbol>', re.S)
PATH = re.compile(r'<path d="([^"]+)"')
NUMBER = re.compile(r"-?\d+(?:\.\d+)?")
EYE_STROKE = 3
# The eyes live per avatar instance, inside the look > scan > lid layers, after <use href="#bot-...">.
INSTANCE_EYES = re.compile(r'<use href="#bot-([a-z-]+)"/><g class="look"><g class="scan"><g class="lid"><path d="([^"]+)"')


def main(sources: list[str], target: str) -> None:
    html = "".join(open(source, encoding="utf-8").read() for source in sources)
    entries = []
    eyes = {}
    for shape, path in INSTANCE_EYES.findall(html):
        eyes.setdefault(shape, path)
    seen = set()
    for name, view_box, body in SYMBOL.findall(html):
        if name in seen:
            continue
        seen.add(name)
        if name.startswith("i-"):
            body = body.replace("currentColor", "#000")
            for width in WIDTHS:
                svg = (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{view_box}" fill="none" '
                       f'stroke="#000" stroke-width="{width / 100}" stroke-linecap="round" '
                       f'stroke-linejoin="round">{body}</svg>')
                entries.append((f"i/{width}/{name[2:]}.svg", svg))
        elif name.startswith("bot-"):
            paths = PATH.findall(body)
            body_path = paths[0]
            eyes_path = paths[1] if len(paths) > 1 else eyes[name[4:]]
            head = f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{view_box}">'
            entries.append((f"bot/{name[4:]}/body.svg", f'{head}<path d="{body_path}" fill="#000"/></svg>'))
            entries.append((f"bot/{name[4:]}/eyes.svg",
                            f'{head}<path d="{eyes_path}" fill="none" stroke="#000" stroke-width="3" '
                            f'stroke-linecap="round"/></svg>'))
            values = [float(n) for n in NUMBER.findall(eyes_path)]
            xs, ys = values[0::2], values[1::2]
            half = EYE_STROKE / 2
            box = (min(xs) - half, min(ys) - half, max(xs) - min(xs) + EYE_STROKE, max(ys) - min(ys) + EYE_STROKE)
            entries.append((f"bot/{name[4:]}/eyes-crop.svg",
                            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{box[0]:g} {box[1]:g} {box[2]:g} {box[3]:g}">'
                            f'<path d="{eyes_path}" fill="none" stroke="#000" stroke-width="3" stroke-linecap="round"/></svg>'))
    # The generated folders only: a stale icon must not outlive its symbol.
    for folder in ("i", "bot"):
        shutil.rmtree(os.path.join(target, folder), ignore_errors=True)
    for path, svg in entries:
        file = os.path.join(target, *path.split("/"))
        os.makedirs(os.path.dirname(file), exist_ok=True)
        with open(file, "w", encoding="utf-8", newline="\n") as out:
            out.write(svg + "\n")
    print(f"{len(entries)} icons -> {target}")


if __name__ == "__main__":
    main(sys.argv[1:-1], sys.argv[-1])
