#!/usr/bin/env python3
"""Render bridge/assets/icon/app-icon.svg into the icons the bridge embeds.

    python3 scripts/make-icons.py

Writes AppIcon.icns (macOS app bundle) and t3wow.ico (Windows Start Menu
shortcut) next to the SVG. Needs macOS (iconutil, sips) and Google Chrome,
which renders the SVG, including its Georgia "T3", with a transparent background.
"""

import struct
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ICON_DIR = ROOT / "bridge/assets/icon"
SVG = ICON_DIR / "app-icon.svg"
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
# The artwork is the macOS 824 px squircle centered on a 1024 px canvas.
# Windows icons fill their canvas, so the .ico uses a tighter crop.
WINDOWS_CROP = 840


def run(*args: str) -> None:
    subprocess.run(args, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def render(work: Path) -> Path:
    page = work / "icon.html"
    page.write_text(
        "<!doctype html><style>html,body{margin:0;background:transparent}</style>"
        f'<img src="{SVG.as_uri()}" width="1024" height="1024">'
    )
    png = work / "icon-1024.png"
    run(CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars",
        "--default-background-color=00000000", "--window-size=1024,1024",
        f"--screenshot={png}", page.as_uri())
    return png


def resize(src: Path, size: int, dest: Path) -> Path:
    run("sips", "-z", str(size), str(size), str(src), "--out", str(dest))
    return dest


def make_icns(master: Path, work: Path) -> None:
    iconset = work / "AppIcon.iconset"
    iconset.mkdir()
    # No 512@2x (1024 px): it alone would add ~600 KB to the binary, and a
    # menu bar app is never shown that large.
    for size in (16, 32, 128, 256):
        resize(master, size, iconset / f"icon_{size}x{size}.png")
        resize(master, size * 2, iconset / f"icon_{size}x{size}@2x.png")
    resize(master, 512, iconset / "icon_512x512.png")
    run("iconutil", "-c", "icns", str(iconset), "-o", str(ICON_DIR / "AppIcon.icns"))


def make_ico(master: Path, work: Path) -> None:
    cropped = work / "icon-windows.png"
    run("sips", "-c", str(WINDOWS_CROP), str(WINDOWS_CROP), str(master), "--out", str(cropped))
    images = [resize(cropped, s, work / f"ico-{s}.png").read_bytes() for s in (16, 24, 32, 48, 64, 128, 256)]
    sizes = (16, 24, 32, 48, 64, 128, 256)
    # ICO with PNG-compressed entries (Windows Vista and later).
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries = b""
    for size, data in zip(sizes, images):
        dim = 0 if size == 256 else size
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    (ICON_DIR / "t3wow.ico").write_bytes(header + entries + b"".join(images))


def main() -> None:
    if sys.platform != "darwin":
        sys.exit("make-icons.py needs macOS (iconutil and sips).")
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        master = render(work)
        make_icns(master, work)
        make_ico(master, work)
    print(f"wrote {ICON_DIR / 'AppIcon.icns'} and {ICON_DIR / 't3wow.ico'}")


if __name__ == "__main__":
    main()
