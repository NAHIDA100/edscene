#!/usr/bin/env python3
"""Regenerate the small PNG assets edscene ships (window icon + hatch tile).

Kept as a script so the binaries are reproducible and never hand-edited:
`python scripts/gen_assets.py`.
"""
import struct
import zlib
from pathlib import Path

UI = Path(__file__).resolve().parent.parent / "crates" / "eds-app" / "ui"
INK = (0x2A, 0x2C, 0x2E, 255)
YELLOW = (0xF5, 0xFF, 0x01, 255)


def write_png(path: Path, w: int, h: int, px) -> None:
    raw = bytearray()
    for y in range(h):
        raw.append(0)  # filter type: none
        for x in range(w):
            raw.extend(px(x, y))
    chunks = [
        b"IHDR" + struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0),
        b"IDAT" + zlib.compress(bytes(raw), 9),
        b"IEND",
    ]
    out = b"\x89PNG\r\n\x1a\n"
    for c in chunks:
        out += struct.pack(">I", len(c) - 4) + c + struct.pack(">I", zlib.crc32(c) & 0xFFFFFFFF)
    path.write_bytes(out)


def icon(x: int, y: int):
    if 12 <= x < 52 and 16 <= y < 23:
        return INK
    if 12 <= x < 40 and 30 <= y < 37:
        return INK
    if 12 <= x < 46 and 44 <= y < 51:
        return INK
    return YELLOW


def hatch(x: int, y: int):
    # 45 degree stripes, 18px pitch, ~5px wide, 8% white.
    on = (x + y) % 18 < 5 or (x + y) % 18 > 62
    return (255, 255, 255, 20) if on else (0, 0, 0, 0)


def main() -> None:
    UI.mkdir(parents=True, exist_ok=True)
    write_png(UI / "icon.png", 64, 64, icon)
    write_png(UI / "hatch.png", 64, 64, hatch)
    for name in ("icon.png", "hatch.png"):
        p = UI / name
        print(f"{name}: {p.stat().st_size} bytes")


if __name__ == "__main__":
    main()
