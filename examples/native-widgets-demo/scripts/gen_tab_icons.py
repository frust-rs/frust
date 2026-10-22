#!/usr/bin/env python3
"""Generate the demo's tab icons, its action icons and its Home avatar as
96x96 RGBA PNGs.

Writes ``assets/icons/{home,controls,dialogs,lab,plus,search,more,avatar}.png``
beside this script's parent directory. Standard library only (``zlib`` +
``struct``): each glyph is a union/difference of simple shapes sampled 4x4 per
pixel for anti-aliased edges, so the output is byte-for-byte reproducible.

The tab and action glyphs are a mid grey on transparent. The Android bottom
navigation tints an icon by its alpha alone, so the colour is unseen there; the
frust-drawn fallback tab row draws byte icons untinted, and grey stays legible
on both a light and a dark surface where white would vanish on light. Apple
builds use SF Symbols instead of these files.

Run from anywhere: ``python3 scripts/gen_tab_icons.py``.
"""

import os
import struct
import zlib

SIZE = 96
SAMPLES = 4
GLYPH_RGB = (0x8A, 0x8A, 0x8A)
AVATAR_RGB = (0x6E, 0x40, 0xC9)
INITIAL_RGB = (0xFF, 0xFF, 0xFF)


def rect(x0, y0, x1, y1):
    return lambda x, y: x0 <= x <= x1 and y0 <= y <= y1


def circle(cx, cy, r):
    return lambda x, y: (x - cx) ** 2 + (y - cy) ** 2 <= r * r


def rounded_rect(x0, y0, x1, y1, r):
    def inside(x, y):
        if not (x0 <= x <= x1 and y0 <= y <= y1):
            return False
        cx = min(max(x, x0 + r), x1 - r)
        cy = min(max(y, y0 + r), y1 - r)
        return (x - cx) ** 2 + (y - cy) ** 2 <= r * r

    return inside


def polygon(*points):
    """A convex polygon, its points in either winding order."""

    def inside(x, y):
        sign = 0
        for (ax, ay), (bx, by) in zip(points, points[1:] + points[:1]):
            cross = (bx - ax) * (y - ay) - (by - ay) * (x - ax)
            if cross != 0:
                if sign == 0:
                    sign = 1 if cross > 0 else -1
                elif (cross > 0) != (sign > 0):
                    return False
        return True

    return inside


def union(*shapes):
    return lambda x, y: any(shape(x, y) for shape in shapes)


def minus(shape, *holes):
    return lambda x, y: shape(x, y) and not any(hole(x, y) for hole in holes)


def home():
    roof = polygon((48, 12), (8, 50), (88, 50))
    body = rect(20, 44, 76, 86)
    door = rect(40, 62, 56, 86)
    return minus(union(roof, body), door)


def controls():
    tracks = [rect(12, y - 3, 84, y + 3) for y in (24, 48, 72)]
    knobs = [circle(x, y, 10) for x, y in ((32, 24), (64, 48), (42, 72))]
    holes = [circle(x, y, 5) for x, y in ((32, 24), (64, 48), (42, 72))]
    return minus(union(*tracks, *knobs), *holes)


def dialogs():
    back = union(
        rounded_rect(6, 10, 60, 48, 11),
        polygon((14, 44), (28, 44), (12, 58)),
    )
    front = union(
        rounded_rect(34, 38, 90, 76, 11),
        polygon((68, 72), (82, 72), (84, 88)),
    )
    gap = union(
        rounded_rect(30, 34, 94, 80, 15),
        polygon((64, 68), (86, 68), (88, 92)),
    )
    return union(minus(back, gap), front)


def lab():
    rim = rounded_rect(30, 8, 66, 16, 3)
    neck = rect(39, 14, 57, 42)
    body = polygon((39, 40), (57, 40), (84, 80), (12, 80))
    base = rounded_rect(12, 70, 84, 88, 8)
    bubble = circle(52, 70, 5)
    return minus(union(rim, neck, body, base), bubble)


def plus():
    bar = rect(16, 44, 80, 52)
    stem = rect(44, 16, 52, 80)
    return union(bar, stem)


def search():
    ring = minus(circle(42, 42, 26), circle(42, 42, 17))
    handle = polygon((58, 62), (62, 58), (84, 80), (80, 84))
    return union(ring, handle)


def more():
    return union(*(circle(x, 48, 8) for x in (20, 48, 76)))


def avatar():
    """A filled circle with a white "F" cut from rectangles."""
    disc = circle(48, 48, 47)
    initial = union(rect(34, 24, 44, 72), rect(34, 24, 64, 33), rect(34, 44, 58, 52))
    return disc, initial


def coverage(shape, px, py):
    hits = 0
    for sy in range(SAMPLES):
        for sx in range(SAMPLES):
            x = px + (sx + 0.5) / SAMPLES
            y = py + (sy + 0.5) / SAMPLES
            if shape(x, y):
                hits += 1
    return hits / (SAMPLES * SAMPLES)


def glyph_pixels(shape, rgb):
    rows = []
    for py in range(SIZE):
        row = bytearray()
        for px in range(SIZE):
            alpha = round(255 * coverage(shape, px, py))
            row += bytes((*rgb, alpha)) if alpha else bytes(4)
        rows.append(bytes(row))
    return rows


def avatar_pixels():
    disc, initial = avatar()
    rows = []
    for py in range(SIZE):
        row = bytearray()
        for px in range(SIZE):
            disc_cov = coverage(disc, px, py)
            ink = coverage(initial, px, py)
            alpha = round(255 * disc_cov)
            if not alpha:
                row += bytes(4)
                continue
            rgb = tuple(
                round(base * (1 - ink) + top * ink)
                for base, top in zip(AVATAR_RGB, INITIAL_RGB)
            )
            row += bytes((*rgb, alpha))
        rows.append(bytes(row))
    return rows


def png(rows):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    raw = b"".join(b"\x00" + row for row in rows)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def main():
    out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets", "icons")
    os.makedirs(out, exist_ok=True)
    images = {
        "home": glyph_pixels(home(), GLYPH_RGB),
        "controls": glyph_pixels(controls(), GLYPH_RGB),
        "dialogs": glyph_pixels(dialogs(), GLYPH_RGB),
        "lab": glyph_pixels(lab(), GLYPH_RGB),
        "plus": glyph_pixels(plus(), GLYPH_RGB),
        "search": glyph_pixels(search(), GLYPH_RGB),
        "more": glyph_pixels(more(), GLYPH_RGB),
        "avatar": avatar_pixels(),
    }
    for name, rows in images.items():
        path = os.path.join(out, name + ".png")
        with open(path, "wb") as f:
            f.write(png(rows))
        print(os.path.normpath(path))


if __name__ == "__main__":
    main()
