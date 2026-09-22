#!/usr/bin/env python3
"""The codec fixtures, made once and committed.

One 33×21 picture — a red ramp across, a green ramp down, a blue
checker with hard edges, one white diagonal — saved by Pillow (libjpeg-
turbo) in every shape the house decoder answers, each beside the RGBA
bytes Pillow decodes it back to. The tests never run this file: they
read what it wrote. Run it again only to regenerate, and say so in the
commit.

    python3 crates/bunny_ui/tests/fixtures/codec/make.py
"""
import os
import struct
import zlib

from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
W, H = 33, 21


def picture():
    image = Image.new("RGB", (W, H))
    pixels = image.load()
    for y in range(H):
        for x in range(W):
            r = (x * 255) // (W - 1)
            g = (y * 255) // (H - 1)
            b = 220 if ((x // 4) + (y // 4)) % 2 == 0 else 30
            if x == y or x == y + 1:
                r, g, b = 255, 255, 255
            pixels[x, y] = (r, g, b)
    return image


def save(image, name, **options):
    path = os.path.join(HERE, name)
    image.save(path, **options)
    decoded = Image.open(path).convert("RGBA").tobytes()
    with open(os.path.join(HERE, name.rsplit(".", 1)[0] + ".rgba"), "wb") as out:
        out.write(decoded)
    print(f"{name:18} {os.path.getsize(path):6} bytes  {len(decoded)} rgba")


def chunk(kind, payload):
    body = kind + payload
    return struct.pack(">I", len(payload)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)


def adam7_png(image, name):
    """A PNG with interlace 1, written by hand — Pillow reads Adam7
    but never writes it. Seven passes, filter 0 on every row."""
    rgba = image.convert("RGBA")
    pixels = rgba.load()
    passes = [
        (0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4),
        (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2),
    ]
    raw = bytearray()
    for (x0, y0, dx, dy) in passes:
        rows = range(y0, H, dy)
        cols = range(x0, W, dx)
        if not rows or not cols:
            continue
        for y in rows:
            raw.append(0)
            for x in cols:
                raw.extend(pixels[x, y])
    data = b"\x89PNG\r\n\x1a\n"
    data += chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 6, 0, 0, 1))
    data += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    data += chunk(b"IEND", b"")
    path = os.path.join(HERE, name)
    with open(path, "wb") as out:
        out.write(data)
    with open(os.path.join(HERE, name.rsplit(".", 1)[0] + ".rgba"), "wb") as out:
        out.write(rgba.tobytes())
    print(f"{name:18} {len(data):6} bytes  {len(rgba.tobytes())} rgba")


if __name__ == "__main__":
    image = picture()
    save(image, "base_444.jpg", quality=85, subsampling=0)
    save(image, "base_422.jpg", quality=85, subsampling=1)
    save(image, "base_420.jpg", quality=85, subsampling=2)
    save(image.convert("L"), "base_gray.jpg", quality=85)
    save(image, "prog_420.jpg", quality=85, subsampling=2, progressive=True)
    save(image, "restart_420.jpg", quality=85, subsampling=2, restart_marker_blocks=2)
    save(image, "adobe_rgb.jpg", quality=85, keep_rgb=True)
    save(image, "plain.png")
    adam7_png(image, "adam7.png")
