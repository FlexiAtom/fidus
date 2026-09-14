#!/usr/bin/env python3
"""Generate deterministic synthetic wallpaper fixtures for fidus tests.

Only Python's standard library is used.  The renderer deliberately writes
unfiltered RGB PNGs itself, so fixture bytes do not depend on Pillow, a system
image codec, locale, or external wallpaper files.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import struct
import zlib
from pathlib import Path

WIDTH, HEIGHT = 128, 96
SEED = 0xF1D05


def _byte(value: float) -> int:
    return max(0, min(255, int(round(value))))


def _noise(x: int, y: int, seed: int = SEED) -> float:
    """A stable integer hash, independent of Python's randomized hash()."""
    n = (x * 0x45D9F3B + y * 0x119DE1F3 + seed * 0x27D4EB2D) & 0xFFFFFFFF
    n ^= n >> 16
    n = (n * 0x45D9F3B) & 0xFFFFFFFF
    return ((n ^ (n >> 16)) & 0xFFFF) / 65535.0


def pixel(kind: str, x: int, y: int) -> tuple[int, int, int]:
    u, v = x / (WIDTH - 1), y / (HEIGHT - 1)
    if kind == "gradient":
        # Two-dimensional smooth gradient: useful for testing low-frequency
        # backgrounds without importing a colour/image library.
        return (_byte(18 + 210 * u), _byte(28 + 170 * v), _byte(90 + 100 * (1 - u)))
    if kind == "periodic":
        # Incommensurate periods make the fixture recognisably periodic while
        # avoiding a single solid colour that is a degenerate detector input.
        a = 0.5 + 0.5 * math.sin(2 * math.pi * x / 13.0)
        b = 0.5 + 0.5 * math.sin(2 * math.pi * y / 9.0)
        return (_byte(20 + 210 * a), _byte(25 + 180 * b), _byte(40 + 160 * (a * b)))
    if kind == "texture":
        # Multi-scale value noise gives a repeatable broadband texture.
        n0 = _noise(x, y)
        n1 = _noise(x // 4, y // 4, SEED + 11)
        n2 = _noise(x // 16, y // 16, SEED + 29)
        n = 0.60 * n0 + 0.28 * n1 + 0.12 * n2
        return (_byte(25 + 190 * n), _byte(35 + 150 * (1 - n)), _byte(70 + 150 * n))
    if kind == "fractal":
        # Bounded Mandelbrot escape-time field, coloured deterministically.
        cx, cy = (x / (WIDTH - 1)) * 3.2 - 2.35, (y / (HEIGHT - 1)) * 2.4 - 1.2
        zx = zy = 0.0
        steps = 0
        while zx * zx + zy * zy <= 4.0 and steps < 48:
            zx, zy = zx * zx - zy * zy + cx, 2 * zx * zy + cy
            steps += 1
        if steps == 48:
            return (8, 10, 24)
        t = steps / 48.0
        return (_byte(20 + 220 * t), _byte(30 + 150 * t * t), _byte(90 + 150 * (1 - t)))
    raise ValueError(f"unknown image kind: {kind}")


def _chunk(tag: bytes, payload: bytes) -> bytes:
    return struct.pack(">I", len(payload)) + tag + payload + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF)


def render(kind: str) -> bytes:
    rows = bytearray()
    for y in range(HEIGHT):
        rows.append(0)  # PNG filter type: none
        for x in range(WIDTH):
            rows.extend(pixel(kind, x, y))
    header = b"\x89PNG\r\n\x1a\n"
    ihdr = struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 2, 0, 0, 0)
    return header + _chunk(b"IHDR", ihdr) + _chunk(b"IDAT", zlib.compress(bytes(rows), 9)) + _chunk(b"IEND", b"")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("tests/fixtures/images"))
    parser.add_argument("--manifest", type=Path, default=None)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    entries = []
    for kind in ("fractal", "texture", "gradient", "periodic"):
        data = render(kind)
        path = args.output / f"{kind}.png"
        path.write_bytes(data)
        entries.append({"name": kind, "path": path.as_posix(), "width": WIDTH, "height": HEIGHT,
                        "sha256": hashlib.sha256(data).hexdigest()})
    if args.manifest is not None:
        args.manifest.parent.mkdir(parents=True, exist_ok=True)
        args.manifest.write_text(json.dumps({"format": "png-rgb8", "seed": SEED,
                                              "generator": "tools/generate_test_images.py",
                                              "images": entries}, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
