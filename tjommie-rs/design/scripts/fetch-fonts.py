"""Fetch and cut the Aegis fonts for the desktop app (aegis/README.md, "Fonts"), which bundles them.

Output: tjommie-rs/design/fonts/*.ttf and OFL.txt (committed). A re-run writes new font timestamps; commit the fonts
only when one really changes.
Downloads land in %LOCALAPPDATA%\\Tjommie-rs\\font-src\\ and are only read, never run.

- Archivo Regular 400 / Medium 500 / SemiBold 600: static instances (wdth 100) of the
  official variable font from google/fonts.
- Display: Archivo at wdth 125, wght 800, every advance tightened by 0.02 em, because GPUI
  has neither variable axes nor letter spacing. GPUI matches fonts with STRETCH_NORMAL, so
  the cut gets its own family name, "Archivo Expanded Tight".
- JetBrains Mono Regular / Medium / SemiBold: the official v2.304 static TTFs.

Run with: python -I tjommie-rs/design/scripts/fetch-fonts.py
"""

import hashlib
import os
import pathlib
import urllib.request

from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

ROOT = pathlib.Path(__file__).resolve().parents[1] / "fonts"
SRC = pathlib.Path(os.environ["LOCALAPPDATA"]) / "Tjommie-rs" / "font-src"
ARCHIVO = "https://github.com/google/fonts/raw/main/ofl/archivo/Archivo%5Bwdth%2Cwght%5D.ttf"
ARCHIVO_OFL = "https://github.com/google/fonts/raw/main/ofl/archivo/OFL.txt"
JBM = "https://github.com/JetBrains/JetBrainsMono/raw/v2.304/fonts/ttf/JetBrainsMono-{}.ttf"
JBM_OFL = "https://github.com/JetBrains/JetBrainsMono/raw/v2.304/OFL.txt"
DISPLAY_FAMILY = "Archivo Expanded Tight"
TRACKING_EM = -0.02


def fetch(url: str, name: str) -> pathlib.Path:
    path = SRC / name
    if not path.exists():
        with urllib.request.urlopen(url, timeout=60) as response:
            path.write_bytes(response.read())
    return path


def tighten(font: TTFont, em: float) -> None:
    """Letter spacing baked into the advances, as CSS letter-spacing would add it."""
    delta = round(em * font["head"].unitsPerEm)
    hmtx = font["hmtx"]
    for glyph, (advance, lsb) in hmtx.metrics.items():
        if advance > 0:
            hmtx.metrics[glyph] = (max(0, advance + delta), lsb)


def rename(font: TTFont, family: str, style: str) -> None:
    names = font["name"]
    for record in list(names.names):
        if record.nameID in (1, 2, 3, 4, 6, 16, 17, 21, 22, 25):
            names.removeNames(nameID=record.nameID)
    postscript = family.replace(" ", "") + "-" + style.replace(" ", "")
    for name_id, value in ((1, family), (2, "Regular"), (3, postscript), (4, f"{family} {style}"),
                           (6, postscript), (16, family), (17, style)):
        names.setName(value, name_id, 3, 1, 0x409)


def main() -> None:
    SRC.mkdir(parents=True, exist_ok=True)
    variable = fetch(ARCHIVO, "Archivo-wdth-wght.ttf")
    archivo_ofl = fetch(ARCHIVO_OFL, "Archivo-OFL.txt")
    jbm_ofl = fetch(JBM_OFL, "JetBrainsMono-OFL.txt")
    ROOT.mkdir(parents=True, exist_ok=True)
    (ROOT / "OFL.txt").write_text(
        "The bundled fonts are under the SIL Open Font License 1.1, reproduced below for each family.\n"
        "Archivo-*.ttf and ArchivoExpandedTight-ExtraBold.ttf (an instance of Archivo at wdth 125, wght 800, with tightened\n"
        "advances, renamed as the license allows for modified versions):\n\n"
        + archivo_ofl.read_text(encoding="utf-8") + "\n\nJetBrainsMono-*.ttf:\n\n" + jbm_ofl.read_text(encoding="utf-8"),
        encoding="utf-8", newline="\n")

    outputs = []
    for weight, style in ((400, "Regular"), (500, "Medium"), (600, "SemiBold")):
        font = instancer.instantiateVariableFont(
            TTFont(variable), {"wdth": 100, "wght": weight}, updateFontNames=True)
        out = ROOT / f"Archivo-{style}.ttf"
        font.save(out)
        outputs.append(out)

    display = instancer.instantiateVariableFont(TTFont(variable), {"wdth": 125, "wght": 800})
    tighten(display, TRACKING_EM)
    rename(display, DISPLAY_FAMILY, "ExtraBold")
    display["OS/2"].usWeightClass = 800
    out = ROOT / "ArchivoExpandedTight-ExtraBold.ttf"
    display.save(out)
    outputs.append(out)

    for style in ("Regular", "Medium", "SemiBold"):
        source = fetch(JBM.format(style), f"JetBrainsMono-{style}.ttf")
        out = ROOT / source.name
        out.write_bytes(source.read_bytes())
        outputs.append(out)

    for path in sorted(SRC.iterdir()) + outputs:
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        print(f"{digest}  {path}")
    for path in outputs:
        font = TTFont(path)
        print(path.name, "|", font["name"].getDebugName(16) or font["name"].getDebugName(1), "|",
              font["name"].getDebugName(17) or font["name"].getDebugName(2), "| wght",
              font["OS/2"].usWeightClass, "| wdth", font["OS/2"].usWidthClass)


if __name__ == "__main__":
    main()
