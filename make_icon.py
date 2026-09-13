"""
Generate az_trainer.ico - a crimson infinity mark on a dark rounded square.

Pure stdlib: renders supersampled RGBA, box-downsamples for antialiasing,
encodes PNG via zlib, and packs a multi-size .ico (PNG-compressed entries,
which Windows has accepted since Vista).

    python make_icon.py
"""

import math
import struct
import zlib

SS = 4  # supersample factor
MASTER = 256
SIZES = [256, 128, 64, 48, 32, 16]

BG = (0x14, 0x16, 0x1D, 255)      # near-black panel
EDGE = (0x2A, 0x2F, 0x3B, 255)    # subtle border
MARK = (0xC2, 0x3B, 0x3B, 255)    # crimson accent
GLINT = (0xE2, 0x6A, 0x6A, 255)   # lighter top edge of the stroke


def blend(dst, src, a):
    return tuple(int(d + (s - d) * a) for d, s in zip(dst[:3], src[:3])) + (255,)


def rounded_rect_mask(w, h, r):
    """Coverage 1.0 inside a rounded rectangle."""
    m = [[0.0] * w for _ in range(h)]
    for y in range(h):
        for x in range(w):
            cx = min(max(x, r), w - 1 - r)
            cy = min(max(y, r), h - 1 - r)
            d = math.hypot(x - cx, y - cy)
            m[y][x] = 1.0 if d <= r else 0.0
    return m


def stamp(px, w, h, cx, cy, radius, color):
    """Filled circle, used as a brush along the curve."""
    x0, x1 = max(0, int(cx - radius) - 1), min(w - 1, int(cx + radius) + 1)
    y0, y1 = max(0, int(cy - radius) - 1), min(h - 1, int(cy + radius) + 1)
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            if math.hypot(x - cx, y - cy) <= radius:
                px[y][x] = color


def render_master():
    w = h = MASTER * SS
    px = [[(0, 0, 0, 0)] * w for _ in range(h)]

    # plate
    mask = rounded_rect_mask(w, h, int(w * 0.22))
    for y in range(h):
        row = px[y]
        mrow = mask[y]
        for x in range(w):
            if mrow[x]:
                row[x] = BG

    # 1px-ish border, drawn as the ring between two masks
    inner = rounded_rect_mask(w, h, int(w * 0.22))
    inset = int(w * 0.018)
    for y in range(h):
        for x in range(w):
            if not mask[y][x]:
                continue
            near_edge = (
                x < inset or y < inset or x >= w - inset or y >= h - inset
                or not inner[min(h - 1, y + inset)][min(w - 1, x + inset)]
                or not inner[max(0, y - inset)][max(0, x - inset)]
            )
            if near_edge:
                px[y][x] = EDGE

    # lemniscate of Bernoulli: the infinity mark
    a = w * 0.30
    cx, cy = w / 2, h / 2
    stroke = w * 0.052
    steps = 1200
    for i in range(steps):
        t = (i / steps) * 2 * math.pi
        denom = 1 + math.sin(t) ** 2
        lx = cx + a * math.cos(t) / denom
        ly = cy + a * math.sin(t) * math.cos(t) / denom
        stamp(px, w, h, lx, ly, stroke, MARK)

    # a lighter pass slightly above, so the stroke reads as lit from the top
    for i in range(steps):
        t = (i / steps) * 2 * math.pi
        denom = 1 + math.sin(t) ** 2
        lx = cx + a * math.cos(t) / denom
        ly = cy + a * math.sin(t) * math.cos(t) / denom - stroke * 0.42
        stamp(px, w, h, lx, ly, stroke * 0.30, GLINT)

    return px, w, h


def downsample(px, sw, sh, size):
    """Box filter from the supersampled master to `size`, keeping alpha."""
    factor = sw // size
    out = []
    for y in range(size):
        row = []
        for x in range(size):
            r = g = b = a = 0
            for dy in range(factor):
                for dx in range(factor):
                    p = px[y * factor + dy][x * factor + dx]
                    r += p[0] * p[3]
                    g += p[1] * p[3]
                    b += p[2] * p[3]
                    a += p[3]
            if a:
                row.append((r // a, g // a, b // a, a // (factor * factor)))
            else:
                row.append((0, 0, 0, 0))
        out.append(row)
    return out


def png_bytes(rows, size):
    raw = bytearray()
    for y in range(size):
        raw.append(0)  # filter: none
        for x in range(size):
            raw.extend(bytes(rows[y][x]))

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def main():
    master, mw, mh = render_master()
    images = []
    for s in SIZES:
        rows = downsample(master, mw, mh, s)
        images.append((s, png_bytes(rows, s)))
        print(f"  rendered {s}x{s}")

    out = bytearray(struct.pack("<HHH", 0, 1, len(images)))
    offset = 6 + 16 * len(images)
    for s, data in images:
        dim = 0 if s >= 256 else s
        out += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    for _, data in images:
        out += data

    with open("az_trainer.ico", "wb") as f:
        f.write(out)
    print(f"wrote az_trainer.ico ({len(out)} bytes, {len(images)} sizes)")


if __name__ == "__main__":
    main()
