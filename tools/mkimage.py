#!/usr/bin/env python3
"""mkimage.py - assemble the tAbLeOS raw disk image.

Layout:
  sector 0       bootsect.bin (MBR)
  sectors 1..62  stage2.bin (padded to 62 sectors)
  sector 64..    kernel.bin (sector count patched into stage2 at offset 4)
  sector 2048..  e1fss.img (optional)
"""

import struct
import sys
import os

SECTOR = 512
STAGE2_SECTORS = 62
KERNEL_LBA = 64
E1FSS_LBA = 2048


def read(path):
    with open(path, "rb") as f:
        return f.read()


def main(bootsect_p, stage2_p, kernel_p, e1fss_p, output, total_sectors):
    bootsect = read(bootsect_p)
    stage2 = bytearray(read(stage2_p))
    kernel = read(kernel_p)
    e1fss = read(e1fss_p) if e1fss_p and os.path.exists(e1fss_p) else b""

    kernel_sectors = (len(kernel) + SECTOR - 1) // SECTOR
    if KERNEL_LBA + kernel_sectors > E1FSS_LBA:
        sys.exit("kernel overlaps e1fss area")

    # Patch kernel_sectors into stage2 at offset 4
    struct.pack_into("<I", stage2, 4, kernel_sectors)

    img = bytearray(total_sectors * SECTOR)
    pos = 0

    def put(data):
        nonlocal pos
        img[pos:pos + len(data)] = data
        pos += len(data)

    put(bootsect)
    pos = 1 * SECTOR
    put(stage2)
    pos = KERNEL_LBA * SECTOR
    put(kernel)
    if e1fss:
        pos = E1FSS_LBA * SECTOR
        put(e1fss)

    os.makedirs(os.path.dirname(output) or ".", exist_ok=True)
    with open(output, "wb") as f:
        f.write(img)
    print(f"mkimage: {output} ({len(img)} bytes, kernel {kernel_sectors} sectors)")


if __name__ == "__main__":
    # args: output total_sectors [e1fss]
    output = sys.argv[1] if len(sys.argv) > 1 else "build/tableos-0.1-amd64.img"
    total = int(sys.argv[2]) if len(sys.argv) > 2 else 16384
    e1fss = sys.argv[3] if len(sys.argv) > 3 else None
    main("build/bootsect.bin", "build/stage2.bin", "build/kernel.bin", e1fss, output, total)
