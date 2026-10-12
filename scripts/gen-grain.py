#!/usr/bin/env python3
"""Write the pre-rendered grain tiles (docs/design.md, Texture and motion).

Paper: dark specks at low alpha (stands in for a multiply). Carbon: light
specks at lower alpha (stands in for a screen). Deterministic, so the PNGs
only change when this script does. Usage: scripts/gen-grain.py <out_dir>
"""
import pathlib, random, struct, sys, zlib

SIZE = 256

def png(path, rgb, max_alpha, seed):
    rng = random.Random(seed)
    rows = bytearray()
    for _ in range(SIZE):
        rows.append(0)  # filter: none
        for _ in range(SIZE):
            # Skew towards faint specks with the occasional darker one.
            a = int((rng.random() ** 2.2) * max_alpha)
            rows += bytes((*rgb, a))
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    ihdr = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(bytes(rows), 9)) + chunk(b"IEND", b""))

out = pathlib.Path(sys.argv[1])
png(out / "grain-paper.png", (60, 50, 35), 12, 1)
png(out / "grain-carbon.png", (255, 245, 225), 7, 2)
