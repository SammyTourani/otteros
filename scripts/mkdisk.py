#!/usr/bin/env python3
"""
Build the test disk image (brief M3-T1 §6).
64 MiB raw disk image where sector n starts with ASCII "OTTERDISK n"
followed by a deterministic byte pattern filling the 512-byte sector.
"""

import struct
import sys

def make_disk_image(output_path: str, size_mib: int = 64):
    """Create a test disk image with a known pattern."""
    size_bytes = size_mib * 1024 * 1024
    sector_size = 512
    num_sectors = size_bytes // sector_size

    with open(output_path, 'wb') as f:
        for sector_num in range(num_sectors):
            # Sector starts with "OTTERDISK N" where N is the sector number.
            header = f"OTTERDISK {sector_num}".encode('ascii')
            # Pad the rest of the sector with a deterministic pattern.
            # Use (sector_num + byte_offset) % 256 for each byte.
            remaining = sector_size - len(header)
            pattern = bytes((sector_num + i) & 0xff for i in range(remaining))
            sector = header + pattern
            assert len(sector) == sector_size
            f.write(sector)

    print(f"[mkdisk] created {output_path}: {size_mib} MiB ({num_sectors} sectors)")

if __name__ == '__main__':
    output = 'build/data.img'
    make_disk_image(output, 64)
