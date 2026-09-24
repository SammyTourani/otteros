#!/usr/bin/env python3
"""
Convert BDF bitmap font to Rust source (brief M2-T4a).
Generates `kernel/src/font8x16.rs` from a BDF file.
Usage: python3 scripts/bdf-to-rust.py <input.bdf> <output.rs>
"""

import sys
import re

def parse_bdf(bdf_path):
    """Parse BDF file and extract glyphs for ASCII 0x20-0x7E."""
    glyphs = {}

    with open(bdf_path, 'r', encoding='utf-8', errors='ignore') as f:
        lines = f.readlines()

    i = 0
    while i < len(lines):
        line = lines[i].strip()

        # Look for character definitions
        if line.startswith('STARTCHAR'):
            char_name = line.split(None, 1)[1] if len(line.split()) > 1 else ""
            i += 1

            encoding = None
            width = None
            height = None
            bitmap = []

            while i < len(lines):
                line = lines[i].strip()

                if line.startswith('ENCODING'):
                    encoding = int(line.split()[1])
                elif line.startswith('BBX'):
                    parts = line.split()
                    width = int(parts[1])
                    height = int(parts[2])
                elif line == 'BITMAP':
                    i += 1
                    while i < len(lines) and not lines[i].strip().startswith('ENDCHAR'):
                        hex_str = lines[i].strip()
                        if hex_str and hex_str != 'BITMAP':
                            bitmap.append(int(hex_str, 16))
                        i += 1
                    break

                i += 1

            # Store glyphs for printable ASCII (0x20-0x7E)
            if encoding and 0x20 <= encoding <= 0x7e and bitmap:
                glyphs[encoding] = bitmap

        i += 1

    return glyphs

def reverse_bits(byte):
    """Reverse the bit order of a byte (BDF uses MSB=left, console expects LSB=left)."""
    result = 0
    for i in range(8):
        if byte & (1 << (7 - i)):
            result |= (1 << i)
    return result

def bitmap_to_bytes(bitmap, width, height):
    """Convert bitmap list to 16-byte array (8x16 format)."""
    # Pad to 16 bytes and reverse bit order (BDF MSB->left, console LSB->left)
    result = [0] * 16
    for i, val in enumerate(bitmap[:min(len(bitmap), 16)]):
        result[i] = reverse_bits(val)
    return result

def generate_rust(glyphs):
    """Generate Rust source code."""
    rust_code = '''//! Embedded 8x16 bitmap font (Spleen, BSD-2-Clause; see THIRD_PARTY_NOTICES.md).
//! Covers printable ASCII 0x20–0x7E with uppercase, lowercase, digits and punctuation.
//! Each glyph is 16 bytes, one per row top-to-bottom; within a row byte, bit N
//! (0 = leftmost) is one pixel.

/// Width and height in pixels of every glyph in this font.
pub const GLYPH_WIDTH: u64 = 8;
pub const GLYPH_HEIGHT: u64 = 16;

const BLANK: [u8; 16] = [0; 16];

/// Returns the 8x16 bitmap for `ch`. Characters outside 0x20–0x7E render
/// as a blank cell rather than panicking.
pub fn glyph(ch: u8) -> [u8; 16] {
    match ch {
'''

    # Generate match arms for each glyph
    for code in sorted(glyphs.keys()):
        if 0x20 <= code <= 0x7e:
            bitmap = glyphs[code]
            # Reverse bit order for each byte (BDF MSB->left, console LSB->left)
            reversed_bitmap = [reverse_bits(b) for b in bitmap]
            bytes_str = ', '.join(f'0x{b:02x}' for b in reversed_bitmap)
            # Escape special characters in comments
            ch_display = chr(code) if code >= 32 and code < 127 else '?'
            rust_code += f"        0x{code:02x} => [{bytes_str}], // {ch_display}\n"

    rust_code += '''        _ => BLANK,
    }
}
'''

    return rust_code

def main():
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <input.bdf> [output.rs]")
        sys.exit(1)

    bdf_path = sys.argv[1]
    out_path = sys.argv[2] if len(sys.argv) > 2 else "kernel/src/font8x16.rs"

    print(f"Parsing {bdf_path}...", file=sys.stderr)
    glyphs = parse_bdf(bdf_path)
    print(f"Found {len(glyphs)} glyphs", file=sys.stderr)

    rust_code = generate_rust(glyphs)

    with open(out_path, 'w') as f:
        f.write(rust_code)

    print(f"✓ Wrote {out_path}", file=sys.stderr)

if __name__ == '__main__':
    main()
