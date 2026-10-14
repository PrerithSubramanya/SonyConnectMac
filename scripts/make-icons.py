#!/usr/bin/env python3
"""Draws the app and tray icons (a simple headphone glyph) with only the standard library.

Writes crates/app/icons/{icon.png, tray.png, icon.icns}. Needs the macOS tools `sips` and
`iconutil` for the .icns file.
"""
import math
import os
import struct
import subprocess
import sys
import tempfile
import zlib

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "crates", "app", "icons")


def png(path, w, h, rgba):
    raw = b"".join(b"\x00" + bytes(rgba[y * w * 4:(y + 1) * w * 4]) for y in range(h))

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(raw, 9)))
        f.write(chunk(b"IEND", b""))


def in_glyph(x, y, s):
    """True if the point (x, y), in a 0..1 square, is inside the headphone glyph. `s` = thickness."""
    cx, cy = 0.5, 0.52
    r_out, r_in = 0.34, 0.34 - 0.075 * s
    dx, dy = x - cx, y - cy
    d = math.hypot(dx, dy)
    # Headband: upper half ring.
    if dy <= 0 and r_in <= d <= r_out:
        return True
    # Short arms down to the cups.
    for side in (-1, 1):
        ax = cx + side * (r_in + r_out) / 2
        if abs(x - ax) <= (r_out - r_in) / 2 and 0 <= dy <= 0.12:
            return True
        # Cups: tall ellipses.
        ex, ey = cx + side * (r_in + r_out) / 2, cy + 0.16
        if ((x - ex) / 0.085) ** 2 + ((y - ey) / 0.15) ** 2 <= 1:
            return True
    return False


def render(size, fg, bg=None, glow=False):
    ss = 3
    buf = bytearray(size * size * 4)
    for py in range(size):
        for px in range(size):
            cov = 0
            for sy in range(ss):
                for sx in range(ss):
                    x = (px + (sx + 0.5) / ss) / size
                    y = (py + (sy + 0.5) / ss) / size
                    if in_glyph(x, y, 1.0):
                        cov += 1
            a = cov / (ss * ss)
            i = (py * size + px) * 4
            if bg:
                # Rounded-square background with a vertical gradient.
                u, v = px / size, py / size
                rad = 0.225
                qx, qy = abs(u - 0.5) - (0.5 - rad), abs(v - 0.5) - (0.5 - rad)
                inside = math.hypot(max(qx, 0), max(qy, 0)) <= rad
                if not inside:
                    continue
                t = v
                base = tuple(int(bg[0][k] * (1 - t) + bg[1][k] * t) for k in range(3))
                if glow:
                    g = max(0.0, 1 - math.hypot(u - 0.5, v - 0.55) / 0.5) ** 2 * 0.55
                    base = tuple(min(255, int(base[k] * (1 - g) + (10, 132, 255)[k] * g)) for k in range(3))
                col = tuple(int(base[k] * (1 - a) + fg[k] * a) for k in range(3))
                buf[i:i + 4] = bytes((*col, 255))
            else:
                buf[i:i + 4] = bytes((*fg, int(255 * a)))
    return buf


def main():
    os.makedirs(OUT, exist_ok=True)
    # Tray icon: black glyph on transparent. macOS tints it as a template image.
    png(os.path.join(OUT, "tray.png"), 44, 44, render(44, (0, 0, 0)))
    big = render(512, (244, 246, 250), bg=((52, 58, 72), (16, 18, 24)), glow=True)
    png(os.path.join(OUT, "icon.png"), 512, 512, big)

    with tempfile.TemporaryDirectory() as tmp:
        iconset = os.path.join(tmp, "icon.iconset")
        os.makedirs(iconset)
        for name, px in [("icon_16x16", 16), ("icon_16x16@2x", 32), ("icon_32x32", 32), ("icon_32x32@2x", 64),
                         ("icon_128x128", 128), ("icon_128x128@2x", 256), ("icon_256x256", 256),
                         ("icon_256x256@2x", 512), ("icon_512x512", 512)]:
            subprocess.run(["sips", "-z", str(px), str(px), os.path.join(OUT, "icon.png"), "--out",
                            os.path.join(iconset, name + ".png")], check=True, capture_output=True)
        subprocess.run(["iconutil", "-c", "icns", iconset, "-o", os.path.join(OUT, "icon.icns")], check=True)
    print("wrote", sorted(os.listdir(OUT)))


if __name__ == "__main__":
    sys.exit(main())
