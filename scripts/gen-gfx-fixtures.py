#!/usr/bin/env python3
"""Generates otter-gfx's host-verified test fixtures (brief M4-T2).

Writes to crates/otter-gfx/tests/fixtures/:
  - zlib streams covering stored/fixed/dynamic Huffman blocks (and a few
    corrupted variants that must be rejected), each `<name>.zlib` with a
    companion `<name>.expected` holding the exact decompressed bytes;
  - PNGs covering every supported colour type, bit depth 1-8-16, all five
    filter types, a palette + tRNS, an RGB tRNS, and one Adam7-interlaced file
    that must be rejected -- plus one with a deliberately corrupted CRC. Each
    `<name>.png` has a companion `<name>.json` with the exact expected
    premultiplied-ARGB pixels (row-major), computed with the same rounding
    otter-gfx's Rust code uses, so the Rust test can assert bit-exact equality.

Python stdlib only (zlib for compression/CRC-32, struct for binary packing) --
this script is tooling, not the crate under test; see DECISIONS.md D27.

Usage: scripts/gen-gfx-fixtures.py
"""
import json
import os
import struct
import zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIXTURES = os.path.join(ROOT, "crates", "otter-gfx", "tests", "fixtures")
os.makedirs(FIXTURES, exist_ok=True)

SIGNATURE = bytes([137, 80, 78, 71, 13, 10, 26, 10])


# --------------------------------------------------------------------------
# zlib/DEFLATE stream fixtures
# --------------------------------------------------------------------------

def write_zlib_fixture(name: str, data: bytes, level: int = 6) -> bytes:
    stream = zlib.compress(data, level)
    with open(os.path.join(FIXTURES, f"{name}.zlib"), "wb") as f:
        f.write(stream)
    with open(os.path.join(FIXTURES, f"{name}.expected"), "wb") as f:
        f.write(data)
    return stream


def block_type_of(stream: bytes) -> int:
    """Reads the first DEFLATE block's BTYPE (bits 1-2 of the first body byte,
    i.e. right after the 2-byte zlib header), for this script's own sanity
    check that the fixture set covers every block type."""
    first = stream[2]
    return (first >> 1) & 0b11


def gen_zlib_fixtures():
    seen_types = set()

    # level 0 (no compression) reliably forces a stored (BTYPE=00) block.
    s = write_zlib_fixture("zlib_stored", b"Hello, OtterOS! " * 4, level=0)
    seen_types.add(block_type_of(s))

    s = write_zlib_fixture("zlib_empty", b"", level=6)
    seen_types.add(block_type_of(s))

    # A single repeated byte at low effort commonly picks a fixed-Huffman block
    # (too little data to be worth a dynamic Huffman table).
    s = write_zlib_fixture("zlib_fixed_huffman", b"x", level=1)
    seen_types.add(block_type_of(s))

    # Real-ish varied English text (many distinct symbols, uneven frequency)
    # reliably makes zlib decide a custom Huffman table is worth it -- a short
    # repetitive phrase does not (it stays fixed-Huffman, verified while
    # writing this script), so this deliberately is not that.
    import random

    rng = random.Random(20260923)
    words = [
        "otter", "kernel", "graphics", "font", "raster", "gradient", "shadow",
        "zlib", "png", "glyph", "path", "vector", "window", "widget", "kelp",
        "surface", "pixel", "coverage", "winding", "bezier", "subpixel",
    ]
    text = " ".join(rng.choice(words) for _ in range(3000)).encode()
    s = write_zlib_fixture("zlib_dynamic_huffman", text, level=6)
    seen_types.add(block_type_of(s))

    # A longer, more varied payload built from a small pseudo-random generator
    # (deterministic, no external dependency) covering many literal values and
    # back-references together.
    state = 12345
    payload = bytearray()
    for _ in range(6000):
        state = (state * 1103515245 + 12345) & 0x7FFFFFFF
        payload.append((state >> 16) & 0xFF if (state & 7) else payload[-1] if payload else 0)
    s = write_zlib_fixture("zlib_mixed", bytes(payload), level=9)
    seen_types.add(block_type_of(s))

    # Multi-block stream: flush between two writes with a streaming compressor.
    co = zlib.compressobj(6)
    parts = [co.compress(b"first part of the stream, " * 8)]
    parts.append(co.flush(zlib.Z_FULL_FLUSH))
    parts.append(co.compress(b"second part after a full flush. " * 8))
    parts.append(co.flush())
    stream = b"".join(parts)
    with open(os.path.join(FIXTURES, "zlib_multi_block.zlib"), "wb") as f:
        f.write(stream)
    with open(os.path.join(FIXTURES, "zlib_multi_block.expected"), "wb") as f:
        f.write((b"first part of the stream, " * 8) + (b"second part after a full flush. " * 8))

    assert seen_types >= {0, 1, 2}, f"fixture set only covers block types {seen_types}, want all of 0,1,2"

    # --- corrupted streams (must be rejected, never panic) -----------------
    good = zlib.compress(b"a valid message for corruption tests", 6)

    truncated = good[: len(good) - 6]
    with open(os.path.join(FIXTURES, "zlib_bad_truncated.zlib"), "wb") as f:
        f.write(truncated)

    flipped = bytearray(good)
    flipped[len(flipped) // 2] ^= 0xFF
    with open(os.path.join(FIXTURES, "zlib_bad_flipped_byte.zlib"), "wb") as f:
        f.write(bytes(flipped))

    bad_adler = bytearray(good)
    bad_adler[-1] ^= 0xFF
    bad_adler[-2] ^= 0xFF
    with open(os.path.join(FIXTURES, "zlib_bad_adler.zlib"), "wb") as f:
        f.write(bytes(bad_adler))

    # Hand-crafted: valid zlib header, one raw DEFLATE byte with BFINAL=1 and
    # the reserved BTYPE=11, i.e. binary ...111 -> 0x07.
    bad_block_type = bytes([0x78, 0x01, 0x07, 0, 0, 0, 0, 0, 0, 0])
    with open(os.path.join(FIXTURES, "zlib_bad_block_type.zlib"), "wb") as f:
        f.write(bad_block_type)

    print(f"gen-gfx-fixtures: zlib block types covered: {sorted(seen_types)}")


# --------------------------------------------------------------------------
# PNG fixtures
# --------------------------------------------------------------------------

def chunk(ctype: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + ctype + data + struct.pack(">I", zlib.crc32(ctype + data) & 0xFFFFFFFF)


def paeth(a: int, b: int, c: int) -> int:
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    if pb <= pc:
        return b
    return c


def filter_row(filt: int, raw: bytes, prev, bpp: int) -> bytes:
    out = bytearray(len(raw))
    for i in range(len(raw)):
        a = raw[i - bpp] if i >= bpp else 0
        b = prev[i] if prev is not None else 0
        c = prev[i - bpp] if (prev is not None and i >= bpp) else 0
        x = raw[i]
        if filt == 0:
            v = x
        elif filt == 1:
            v = (x - a) & 0xFF
        elif filt == 2:
            v = (x - b) & 0xFF
        elif filt == 3:
            v = (x - (a + b) // 2) & 0xFF
        elif filt == 4:
            v = (x - paeth(a, b, c)) & 0xFF
        else:
            raise ValueError(filt)
        out[i] = v
    return bytes(out)


def pack_bits(samples, bit_depth: int) -> bytes:
    if bit_depth == 16:
        out = bytearray()
        for s in samples:
            out += struct.pack(">H", s)
        return bytes(out)
    if bit_depth == 8:
        return bytes(samples)
    total_bits = len(samples) * bit_depth
    out = bytearray((total_bits + 7) // 8)
    bitpos = 0
    for s in samples:
        byte_idx = bitpos // 8
        bit_off = bitpos % 8
        shift = 8 - bit_depth - bit_off
        out[byte_idx] |= (s & ((1 << bit_depth) - 1)) << shift
        bitpos += bit_depth
    return bytes(out)


def div255(x: int) -> int:
    t = x + 128
    return (t + (t >> 8)) >> 8


def premul_argb(r: int, g: int, b: int, a: int) -> int:
    return (a << 24) | (div255(r * a) << 16) | (div255(g * a) << 8) | div255(b * a)


def expand(v: int, bit_depth: int) -> int:
    return {1: 255 if v else 0, 2: v * 85, 4: v * 17, 8: v, 16: v >> 8}[bit_depth]


CHANNELS = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}


def straight_color(color_type, bit_depth, sample, palette, trns):
    """`sample` is the raw per-pixel tuple of channel values (pre-expansion,
    except for palette indices). Returns (r, g, b, a), each 0-255."""
    if color_type == 0:
        (gray,) = sample
        a = 0 if (trns is not None and gray == trns) else 255
        g8 = expand(gray, bit_depth)
        return (g8, g8, g8, a)
    if color_type == 2:
        r, g, b = sample
        a = 0 if (trns is not None and (r, g, b) == trns) else 255
        return (expand(r, bit_depth), expand(g, bit_depth), expand(b, bit_depth), a)
    if color_type == 3:
        (idx,) = sample
        r, g, b = palette[idx]
        a = trns[idx] if (trns is not None and idx < len(trns)) else 255
        return (r, g, b, a)
    if color_type == 4:
        gray, a = sample
        g8 = expand(gray, bit_depth)
        return (g8, g8, g8, expand(a, bit_depth))
    r, g, b, a = sample
    return (expand(r, bit_depth), expand(g, bit_depth), expand(b, bit_depth), expand(a, bit_depth))


def add_png_fixture(name, width, height, bit_depth, color_type, pixel_rows, filters, palette=None, trns=None, interlace=0):
    """`pixel_rows[y][x]` is a tuple of raw channel values for that pixel (or a
    1-tuple palette index for colour type 3)."""
    channels = CHANNELS[color_type]
    bpp_bits = channels * bit_depth
    bpp_bytes = max(1, (bpp_bits + 7) // 8)

    raw_rows = []
    expected = []
    for row in pixel_rows:
        flat = [v for px in row for v in px]
        raw_rows.append(pack_bits(flat, bit_depth))
        for px in row:
            r, g, b, a = straight_color(color_type, bit_depth, px, palette, trns)
            expected.append(premul_argb(r, g, b, a))

    filtered = bytearray()
    prev = None
    for row, filt in zip(raw_rows, filters):
        filtered.append(filt)
        filtered += filter_row(filt, row, prev, bpp_bytes)
        prev = row

    compressed = zlib.compress(bytes(filtered), 6)
    out = bytearray()
    out += SIGNATURE
    out += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, bit_depth, color_type, 0, 0, interlace))
    if palette is not None:
        out += chunk(b"PLTE", b"".join(struct.pack(">BBB", *c) for c in palette))
    if trns is not None:
        if color_type == 0:
            out += chunk(b"tRNS", struct.pack(">H", trns))
        elif color_type == 2:
            out += chunk(b"tRNS", struct.pack(">HHH", *trns))
        elif color_type == 3:
            out += chunk(b"tRNS", bytes(trns))
    out += chunk(b"IDAT", compressed)
    out += chunk(b"IEND", b"")

    with open(os.path.join(FIXTURES, f"{name}.png"), "wb") as f:
        f.write(bytes(out))
    with open(os.path.join(FIXTURES, f"{name}.json"), "w") as f:
        json.dump({"width": width, "height": height, "argb": expected}, f)
    return bytes(out)


def gen_png_fixtures():
    # Colour type 0 (grayscale), every allowed bit depth, a different filter
    # each time so all five filter types appear across the fixture set.
    add_png_fixture("gray_bd1", 4, 2, 1, 0, [[(1,), (0,), (1,), (0,)], [(0,), (1,), (0,), (1,)]], filters=[0, 1])
    add_png_fixture("gray_bd2", 4, 2, 2, 0, [[(0,), (1,), (2,), (3,)], [(3,), (2,), (1,), (0,)]], filters=[0, 2])
    add_png_fixture("gray_bd4", 4, 2, 4, 0, [[(0,), (5,), (10,), (15,)], [(15,), (10,), (5,), (0,)]], filters=[0, 3])
    add_png_fixture("gray_bd8", 3, 3, 8, 0, [[(10,), (128,), (250,)], [(0,), (60,), (200,)], [(30,), (90,), (255,)]], filters=[0, 4, 2])
    add_png_fixture("gray_bd16", 2, 2, 16, 0, [[(0x0100,), (0x80FF,)], [(0xFF00,), (0x4321,)]], filters=[0, 4])
    add_png_fixture(
        "gray_bd8_trns", 2, 2, 8, 0, [[(5,), (200,)], [(5,), (5,)]], filters=[0, 0], trns=5,
    )

    # Colour type 2 (truecolor), bit depths 8 and 16, mixed filters, plus tRNS.
    add_png_fixture(
        "rgb_bd8",
        3,
        2,
        8,
        2,
        [[(255, 0, 0), (0, 255, 0), (0, 0, 255)], [(10, 20, 30), (200, 150, 100), (1, 2, 3)]],
        filters=[0, 1],
    )
    add_png_fixture("rgb_bd16", 2, 2, 16, 2, [[(0x1234, 0x5678, 0x9ABC), (0xFFFF, 0, 0)], [(0, 0xFFFF, 0), (0, 0, 0xFFFF)]], filters=[0, 3])
    add_png_fixture(
        "rgb_bd8_trns",
        2,
        2,
        8,
        2,
        [[(1, 2, 3), (9, 9, 9)], [(1, 2, 3), (1, 2, 3)]],
        filters=[0, 0],
        trns=(1, 2, 3),
    )

    # Colour type 3 (palette), bit depth 4 with a tRNS alpha table.
    palette = [(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 0)]
    add_png_fixture(
        "palette_bd4",
        4,
        2,
        4,
        3,
        [[(0,), (1,), (2,), (3,)], [(3,), (2,), (1,), (0,)]],
        filters=[0, 4],
        palette=palette,
        trns=[255, 128, 0, 255],
    )
    add_png_fixture(
        "palette_bd8", 3, 1, 8, 3, [[(0,), (1,), (2,)]], filters=[0], palette=palette,
    )

    # Colour type 4 (grayscale + alpha), bit depths 8 and 16.
    add_png_fixture("gray_alpha_bd8", 2, 2, 8, 4, [[(10, 255), (200, 128)], [(0, 0), (255, 64)]], filters=[0, 2])
    add_png_fixture("gray_alpha_bd16", 2, 2, 16, 4, [[(0x1000, 0xFFFF), (0x8000, 0x8000)], [(0, 0), (0xFFFF, 0)]], filters=[0, 4])

    # Colour type 6 (truecolor + alpha), bit depths 8 and 16.
    add_png_fixture(
        "rgba_bd8",
        3,
        3,
        8,
        6,
        [
            [(255, 0, 0, 255), (0, 255, 0, 128), (0, 0, 255, 0)],
            [(10, 20, 30, 255), (200, 150, 100, 64), (1, 2, 3, 200)],
            [(0, 0, 0, 0), (255, 255, 255, 255), (128, 128, 128, 128)],
        ],
        filters=[0, 1, 4],
    )
    add_png_fixture(
        "rgba_bd16",
        2,
        2,
        16,
        6,
        [[(0xFFFF, 0, 0, 0xFFFF), (0, 0xFFFF, 0, 0x8000)], [(0, 0, 0xFFFF, 0), (0x1234, 0x5678, 0x9ABC, 0xFFFF)]],
        filters=[0, 3],
    )

    # Adam7-interlaced (must be rejected with a clear error, not decoded wrong
    # or panicked on): valid otherwise-plain IHDR/IDAT/IEND, interlace=1.
    add_png_fixture(
        "interlaced_adam7", 2, 2, 8, 2, [[(1, 2, 3), (4, 5, 6)], [(7, 8, 9), (10, 11, 12)]], filters=[0, 0], interlace=1,
    )

    # A structurally valid PNG with a deliberately corrupted chunk CRC.
    good = add_png_fixture("bad_crc_source", 2, 2, 8, 0, [[(1,), (2,)], [(3,), (4,)]], filters=[0, 0])
    corrupted = bytearray(good)
    corrupted[-1] ^= 0xFF  # last byte of IEND's CRC
    with open(os.path.join(FIXTURES, "bad_crc.png"), "wb") as f:
        f.write(bytes(corrupted))
    os.remove(os.path.join(FIXTURES, "bad_crc_source.png"))
    os.remove(os.path.join(FIXTURES, "bad_crc_source.json"))


def main():
    gen_zlib_fixtures()
    gen_png_fixtures()
    names = sorted(os.listdir(FIXTURES))
    print(f"gen-gfx-fixtures: wrote {len(names)} files to {FIXTURES}")


if __name__ == "__main__":
    main()
