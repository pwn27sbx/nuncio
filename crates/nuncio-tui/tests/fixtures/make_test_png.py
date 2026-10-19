#!/usr/bin/env python3
"""Regenerate `test.png`, the local image the S9 `ratatui-image` checks render.

Standard library only: a 64x64 RGBA PNG with four flat quadrants and a diagonal, so a
mis-scaled or mis-coloured render is obvious to the eye. Run from this directory:

    python3 make_test_png.py
"""

import struct
import zlib

WIDTH = HEIGHT = 64


def chunk(tag: bytes, data: bytes) -> bytes:
    crc = zlib.crc32(tag + data) & 0xFFFFFFFF
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", crc)


def pixels() -> bytes:
    rows = bytearray()
    for y in range(HEIGHT):
        rows.append(0)  # filter type: None
        for x in range(WIDTH):
            # RGBA quadrants: red / green / blue / white, with a diagonal stripe.
            if abs(x - y) < 4:
                rows += bytes((255, 255, 255, 255))
            elif x < WIDTH // 2 and y < HEIGHT // 2:
                rows += bytes((220, 50, 47, 255))
            elif x >= WIDTH // 2 and y < HEIGHT // 2:
                rows += bytes((133, 153, 0, 255))
            elif x < WIDTH // 2:
                rows += bytes((38, 139, 210, 255))
            else:
                rows += bytes((238, 232, 213, 255))
    return bytes(rows)


def main() -> None:
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(pixels(), 9))
    png += chunk(b"IEND", b"")
    with open("test.png", "wb") as handle:
        handle.write(png)
    print(f"wrote test.png ({len(png)} bytes, {WIDTH}x{HEIGHT} RGBA)")


if __name__ == "__main__":
    main()
