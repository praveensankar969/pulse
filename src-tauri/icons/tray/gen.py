#!/usr/bin/env python3
"""Rasterize dual-square tray marks to mac 18/36 and win 16/32 PNGs.

Runtime icons are painted in src-tauri/src/platform/tray.rs. This script
keeps the checked-in PNG fallbacks in sync with the SVGs.
"""

from __future__ import annotations

import math
import struct
import zlib
from pathlib import Path

OK = (0x36, 0xA1, 0x5A)
WARN = (0xD4, 0xA0, 0x2A)
DANGER = (0xE2, 0x4B, 0x3C)
MUTED = (0xC7, 0xC7, 0xCC)
SLASH = (0x8E, 0x8E, 0x93)
WHITE = (0xFF, 0xFF, 0xFF)

GLYPHS = {
    "0": [1, 1, 1, 1, 0, 1, 1, 0, 1, 1, 0, 1, 1, 1, 1],
    "1": [0, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1, 1],
    "2": [1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 1],
    "3": [1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1],
    "4": [1, 0, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 1],
    "5": [1, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 1],
    "6": [1, 1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 1, 1, 1, 1],
    "7": [1, 1, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1],
    "8": [1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1],
    "9": [1, 1, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1],
    "+": [0, 0, 0, 0, 1, 0, 1, 1, 1, 0, 1, 0, 0, 0, 0],
}


def blend(px, i, rgb, cover):
    src_a = max(0.0, min(1.0, cover))
    dst_a = px[i + 3] / 255.0
    out_a = src_a + dst_a * (1.0 - src_a)
    if out_a <= 0:
        return
    for c in range(3):
        src = rgb[c] / 255.0
        dst = px[i + c] / 255.0
        px[i + c] = int((src * src_a + dst * dst_a * (1.0 - src_a)) / out_a * 255.0 + 0.5)
    px[i + 3] = int(out_a * 255.0 + 0.5)


def circle_cover(x, y, cx, cy, radius):
    dx = x + 0.5 - cx
    dy = y + 0.5 - cy
    dist = math.hypot(dx, dy)
    return max(0.0, min(1.0, radius + 0.5 - dist))


def dist_to_segment(px, py, x1, y1, x2, y2):
    dx, dy = x2 - x1, y2 - y1
    length = dx * dx + dy * dy
    t = 0.0 if length == 0 else max(0.0, min(1.0, ((px - x1) * dx + (py - y1) * dy) / length))
    return math.hypot(px - (x1 + t * dx), py - (y1 + t * dy))


def round_rect_cover(x, y, rx, ry, w, h, radius):
    px_c, py_c = x + 0.5, y + 0.5
    if not (rx <= px_c <= rx + w and ry <= py_c <= ry + h):
        return 0.0
    radius = min(radius, w / 2.0, h / 2.0)
    cx = min(max(px_c, rx + radius), rx + w - radius)
    cy = min(max(py_c, ry + radius), ry + h - radius)
    dist = math.hypot(px_c - cx, py_c - cy)
    if dist <= radius:
        return max(0.6, max(0.0, min(1.0, radius + 0.5 - dist)))
    return 0.0


def paint(kind: str, size: int, logical: float, badge: int | None = None) -> bytearray:
    px = bytearray(size * size * 4)
    scale = size / logical
    side = 7.2 * scale
    offset = 5.0 * scale
    radius = 1.6 * scale
    total_w = offset + side
    x0 = (size - total_w) / 2.0
    y0 = (size - side) / 2.0
    stroke = 1.35 * scale
    cx = cy = size / 2.0
    slash_half = 6.0 * scale
    slash_w = 0.75 * scale

    def fill_sq(x, y, rgb, alpha):
        for py in range(size):
            for px_i in range(size):
                cover = round_rect_cover(px_i, py, x, y, side, side, radius) * alpha
                if cover > 0:
                    blend(px, (py * size + px_i) * 4, rgb, cover)

    def stroke_sq(x, y, rgb, alpha):
        inner = max(0.6, stroke)
        for py in range(size):
            for px_i in range(size):
                outer = round_rect_cover(px_i, py, x, y, side, side, radius)
                hole = round_rect_cover(
                    px_i, py, x + inner, y + inner, max(0.0, side - inner * 2), max(0.0, side - inner * 2), max(0.0, radius - inner)
                )
                cover = max(0.0, min(1.0, outer - hole)) * alpha
                if cover > 0:
                    blend(px, (py * size + px_i) * 4, rgb, cover)

    def pair_fill(rgb):
        fill_sq(x0, y0, rgb, 1.0)
        fill_sq(x0 + offset, y0, rgb, 0.38)

    def pair_stroke(rgb):
        stroke_sq(x0, y0, rgb, 1.0)
        stroke_sq(x0 + offset, y0, rgb, 0.38)

    def slash(rgb):
        x1, y1, x2, y2 = cx - slash_half, cy + slash_half, cx + slash_half, cy - slash_half
        for y in range(size):
            for x in range(size):
                cover = max(0.0, min(1.0, slash_w + 0.5 - dist_to_segment(x + 0.5, y + 0.5, x1, y1, x2, y2)))
                if cover > 0:
                    blend(px, (y * size + x) * 4, rgb, cover)

    if kind == "healthy":
        pair_fill(OK)
    elif kind == "degraded":
        pair_fill(WARN)
    elif kind == "down":
        pair_fill(DANGER)
        if badge:
            draw_badge(px, size, scale, badge)
    elif kind == "hollow":
        pair_stroke(MUTED)
    elif kind == "offline":
        pair_fill(MUTED)
        slash(SLASH)
    elif kind == "poller-dead":
        pair_stroke(DANGER)
        slash(DANGER)
    return px


def draw_badge(px, size, scale, count):
    label = "99+" if count > 99 else str(count)
    cell = max(1.0, scale)
    glyph_w, glyph_h, gap = 3.0 * cell, 5.0 * cell, cell
    text_w = len(label) * glyph_w + max(0, len(label) - 1) * gap
    pad_x, pad_y = 2.0 * scale, 1.5 * scale
    bw = max(text_w + pad_x * 2.0, 6.0 * scale)
    bh = glyph_h + pad_y * 2.0
    bx, by, radius = size - bw, 0.0, bh / 2.0
    for y in range(size):
        for x in range(size):
            px_c, py_c = x + 0.5, y + 0.5
            cx = min(max(px_c, bx + radius), bx + bw - radius)
            cy = min(max(py_c, by + radius), by + bh - radius)
            if bx <= px_c <= bx + bw and by <= py_c <= by + bh:
                dist = math.hypot(px_c - cx, py_c - cy)
                if dist <= radius:
                    blend(px, (y * size + x) * 4, DANGER, max(0.6, max(0.0, min(1.0, radius + 0.5 - dist))))
    tx = bx + (bw - text_w) / 2.0
    ty = by + (bh - glyph_h) / 2.0
    for ch in label:
        bits = GLYPHS[ch]
        for row in range(5):
            for col in range(3):
                if bits[row * 3 + col]:
                    x0, y0 = tx + col * cell, ty + row * cell
                    for y in range(int(y0), min(size, int(math.ceil(y0 + cell)))):
                        for x in range(int(x0), min(size, int(math.ceil(x0 + cell)))):
                            l = min(x + 1.0, x0 + cell) - max(x, x0)
                            t = min(y + 1.0, y0 + cell) - max(y, y0)
                            cover = max(0.0, min(1.0, l * t))
                            if cover > 0:
                                blend(px, (y * size + x) * 4, WHITE, cover)
        tx += glyph_w + gap


def write_png(path: Path, rgba: bytearray, size: int) -> None:
    raw = b"".join(b"\x00" + bytes(rgba[y * size * 4 : (y + 1) * size * 4]) for y in range(size))

    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def main() -> None:
    root = Path(__file__).resolve().parent
    specs = [
        ("mac", 18, 18.0, ""),
        ("mac", 36, 18.0, "@2x"),
        ("win", 16, 16.0, ""),
        ("win", 32, 16.0, "@2x"),
    ]
    kinds = ["healthy", "degraded", "down", "hollow", "offline", "poller-dead"]
    for platform, size, logical, suffix in specs:
        dest = root / platform
        dest.mkdir(parents=True, exist_ok=True)
        for kind in kinds:
            write_png(dest / f"{kind}{suffix}.png", paint(kind, size, logical), size)
        write_png(dest / f"down-2{suffix}.png", paint("down", size, logical, badge=2), size)


if __name__ == "__main__":
    main()
