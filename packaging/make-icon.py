#!/usr/bin/env python3
"""The task board's app icon, drawn in code (no image tools needed): a board.

Three cards-columns on a blue squircle, the middle one shorter, with an orange "needs you"
dot on the last column. The colors are the board's accent and warn tokens. The dev icon is
purple with a yellow "DEV" band across the bottom, so it can't be mistaken for the real one.

Usage: packaging/make-icon.py packaging/assets/Taskboard.icns
       packaging/make-icon.py --dev packaging/assets/TaskboardDev.icns   (Taskboard Dev: purple, "DEV" band)
Writes a 1024px PNG with the stdlib only, then uses sips + iconutil for the .icns sizes.
The design is on a 100-unit grid (the body is 9..91, the macOS 824px icon body).
"""
import math, os, struct, subprocess, sys, tempfile, zlib

N = 1024
U = N / 100
BODY = (0x2E, 0x5B, 0xE0)
BODY_DEV = (0x5B, 0x3F, 0xC4)
COLUMN = (0xF4, 0xF6, 0xFB)
CARD = (0xC9, 0xD6, 0xF8)
CARD_DEV = (0xD6, 0xCC, 0xF5)
DOT = (0xF2, 0x8A, 0x4B)
BAND = (0xFF, 0xC8, 0x3D)
INK = (0x1E, 0x14, 0x3D)
BAND_TOP = 63
# "DEV" as strokes on the 100-unit grid: line segments plus the D's half-circle (cx, cy, r).
DEV_LINES = (
    ((32, 71.5), (32, 84.5)), ((32, 71.5), (36, 71.5)), ((32, 84.5), (36, 84.5)),
    ((46, 71.5), (46, 84.5)), ((46, 71.5), (55, 71.5)), ((46, 78), (53.5, 78)), ((46, 84.5), (55, 84.5)),
    ((58, 71.5), (63, 84.5)), ((63, 84.5), (68, 71.5)),
)
DEV_ARC = (36, 78, 6.5)
STROKE = 3.2


def rrect_sdf(x, y, cx, cy, hw, hh, r):
    qx, qy = abs(x - cx) - hw + r, abs(y - cy) - hh + r
    return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - r


def cov(d):  # signed distance (px) -> coverage, ~1px anti-aliasing
    return max(0.0, min(1.0, 0.5 - d))


def blend(dst, src, a):
    return tuple(round(d + (s - d) * a) for d, s in zip(dst, src))


def box(x, y, left, top, w, h, r):
    return cov(rrect_sdf(x, y, (left + w / 2) * U, (top + h / 2) * U, w / 2 * U, h / 2 * U, r * U))


def segment_sdf(x, y, a, b):
    (ax, ay), (bx, by) = a, b
    dx, dy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(x - ax - t * dx, y - ay - t * dy)


def dev_ink(x, y):  # coverage of the "DEV" letters at pixel (x, y)
    ux, uy = x / U, y / U
    d = min(segment_sdf(ux, uy, a, b) for a, b in DEV_LINES)
    cx, cy, r = DEV_ARC
    if ux >= cx:  # the D's right half-circle
        d = min(d, abs(math.hypot(ux - cx, uy - cy) - r))
    return cov((d - STROKE / 2) * U)


def pixel(x, y, dev):
    body = box(x, y, 9, 9, 82, 82, 18.5)
    if body <= 0:
        return (0, 0, 0, 0)
    c = BODY_DEV if dev else BODY
    card = CARD_DEV if dev else CARD
    # Three columns.
    for left, h in ((20, 58), (40, 42), (60, 58)):
        c = blend(c, COLUMN, box(x, y, left, 21, 17, h, 4))
    # Cards inside the columns.
    for left, tops in ((20, (25, 37, 49)), (40, (25, 37)), (60, (25, 37))):
        for top in tops:
            c = blend(c, card, box(x, y, left + 3, top, 11, 8.5, 2.2))
    # The "needs you" dot; on the dev icon it moves up, clear of the band.
    d = math.hypot(x - 73 * U, y - (56 if dev else 70) * U) - 6.5 * U
    c = blend(c, DOT, cov(d))
    if dev:
        c = blend(c, BAND, cov(BAND_TOP * U - y))
        c = blend(c, INK, dev_ink(x, y))
    return (*c, round(255 * body))


def write_png(path, w, h, rows):
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    raw = b"".join(b"\x00" + bytes(v for p in row for v in p) for row in rows)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(raw, 9)))
        f.write(chunk(b"IEND", b""))


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    dev = "--dev" in sys.argv
    if len(args) != 1:
        print(__doc__)
        sys.exit(2)
    out = args[0]
    with tempfile.TemporaryDirectory() as tmp:
        master = os.path.join(tmp, "icon.png")
        rows = [[pixel(x + 0.5, y + 0.5, dev) for x in range(N)] for y in range(N)]
        write_png(master, N, N, rows)
        iconset = os.path.join(tmp, "icon.iconset")
        os.mkdir(iconset)
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                px = size * scale
                name = f"icon_{size}x{size}{'@2x' if scale == 2 else ''}.png"
                subprocess.run(["sips", "-z", str(px), str(px), master, "--out", os.path.join(iconset, name)], check=True, capture_output=True)
        subprocess.run(["iconutil", "-c", "icns", iconset, "-o", out], check=True)
        png_out = os.path.splitext(out)[0] + ".png"
        subprocess.run(["sips", "-z", "512", "512", master, "--out", png_out], check=True, capture_output=True)
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
