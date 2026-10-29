#!/usr/bin/env python3
"""mke1fss.py - create an e1FSS filesystem image for tAbLeOS."""

import struct
import sys
import os

BLOCK_SIZE = 4096
MAGIC = b"e1FSSv1\0"

# Layout constants
SUPERBLOCK = 0
BITMAP_START = 1
INODE_TABLE_START = 2  # bitmap is 1 block (4096 bytes = 32768 blocks = 128MB)
DATA_START = 34        # inode table = 32 blocks (32 * 4096 / 128 = 1024 inodes)
ROOT_INO = 0


def make_image(path, files=None, volume_name="tAbLeOS"):
    """Create an e1FSS image.

    files: list of (name, bytes) tuples
    """
    if files is None:
        files = []

    total_blocks = 1280   # 5MB fs area (disk image reserves 1MB..6MB for e1fss)
    inode_count = 1024

    # Allocate image
    img = bytearray(total_blocks * BLOCK_SIZE)

    # Superblock
    sb = struct.pack(
        "<8sIIIIIIIII32s",
        MAGIC,
        BLOCK_SIZE,
        total_blocks,
        inode_count,
        BITMAP_START,
        INODE_TABLE_START,
        DATA_START,
        ROOT_INO,
        total_blocks - DATA_START,  # free_blocks
        inode_count - 1,            # free_inodes (root uses 1)
        volume_name.encode()[:32].ljust(32, b"\0"),
    )
    sb = sb.ljust(BLOCK_SIZE, b"\0")  # pad to full block, else slice-assign shrinks img
    img[SUPERBLOCK * BLOCK_SIZE:(SUPERBLOCK + 1) * BLOCK_SIZE] = sb

    # Block bitmap: mark reserved blocks (superblock, bitmap, inode table)
    # and root dir block
    bitmap = bytearray(BLOCK_SIZE)
    reserved = [SUPERBLOCK, BITMAP_START] + list(range(INODE_TABLE_START, DATA_START))
    root_dir_block = DATA_START
    reserved.append(root_dir_block)
    free_blocks = total_blocks - len(reserved)

    for b in reserved:
        byte = b // 8
        bit = b % 8
        bitmap[byte] |= 1 << bit

    img[BITMAP_START * BLOCK_SIZE:(BITMAP_START + 1) * BLOCK_SIZE] = bitmap

    # Inode table: root inode (ino 0)
    # layout: type u16, nlinks u16, size u32, blocks[12] u32, indirect u32,
    #         ctime u32, mtime u32, pad 60  => 128 bytes
    inode_fmt = "<HHI" + "12I" + "III" + "60s"
    inode_table = bytearray(32 * BLOCK_SIZE)
    root_blocks = [root_dir_block] + [0] * 11
    root_inode = struct.pack(
        inode_fmt,
        2,          # type = dir
        1,          # nlinks
        BLOCK_SIZE, # size (one block of dirents)
        *root_blocks,
        0,          # indirect
        0, 0,       # ctime, mtime
        b"\0" * 60,
    )
    inode_table[0:128] = root_inode

    img[INODE_TABLE_START * BLOCK_SIZE:(INODE_TABLE_START + 32) * BLOCK_SIZE] = inode_table

    # Root directory block
    dir_block = bytearray(BLOCK_SIZE)
    img[root_dir_block * BLOCK_SIZE:(root_dir_block + 1) * BLOCK_SIZE] = dir_block

    # Write files
    next_ino = 1
    next_data_block = DATA_START + 1
    dir_offset = 0
    total_data_blocks = 0

    for name, data in files:
        if len(name) > 59:
            print(f"warning: name too long: {name}")
            continue

        # Allocate inode
        ino = next_ino
        next_ino += 1

        # Allocate data blocks
        blocks_needed = (len(data) + BLOCK_SIZE - 1) // BLOCK_SIZE
        block_list = []
        for _ in range(blocks_needed):
            block_list.append(next_data_block)
            next_data_block += 1
            if next_data_block >= total_blocks:
                raise RuntimeError("image full")

        # Pad to 12 direct blocks
        direct = block_list[:12]
        indirect_blocks = block_list[12:]
        while len(direct) < 12:
            direct.append(0)

        indirect = 0
        if indirect_blocks:
            indirect = next_data_block
            next_data_block += 1
            if next_data_block >= total_blocks:
                raise RuntimeError("image full")
            # write indirect block
            ind_data = struct.pack(f"<{len(indirect_blocks)}I", *indirect_blocks)
            img[indirect * BLOCK_SIZE:indirect * BLOCK_SIZE + len(ind_data)] = ind_data
            # mark indirect block in bitmap
            byte = indirect // 8
            bit = indirect % 8
            bitmap[byte] |= 1 << bit

        # Mark data blocks in bitmap
        for b in block_list:
            byte = b // 8
            bit = b % 8
            bitmap[byte] |= 1 << bit
        total_data_blocks += len(block_list)
        if indirect:
            total_data_blocks += 1

        # Write inode
        inode = struct.pack(
            inode_fmt,
            1,  # type = file
            1,
            len(data),
            *direct,
            indirect,
            0, 0,
            b"\0" * 60,
        )
        inode_table[ino * 128:(ino + 1) * 128] = inode

        # Write data
        for i, b in enumerate(block_list):
            start = i * BLOCK_SIZE
            end = min(start + BLOCK_SIZE, len(data))
            img[b * BLOCK_SIZE:b * BLOCK_SIZE + (end - start)] = data[start:end]

        # Write dirent
        dirent = struct.pack("<IB59s", ino, len(name), name.encode())
        dir_block[dir_offset:dir_offset + 64] = dirent
        dir_offset += 64

    # Update bitmap and inode table
    img[BITMAP_START * BLOCK_SIZE:(BITMAP_START + 1) * BLOCK_SIZE] = bitmap
    img[INODE_TABLE_START * BLOCK_SIZE:(INODE_TABLE_START + 32) * BLOCK_SIZE] = inode_table
    img[root_dir_block * BLOCK_SIZE:(root_dir_block + 1) * BLOCK_SIZE] = dir_block

    # Update superblock free counts
    used_blocks = len(reserved) + total_data_blocks
    used_inodes = next_ino
    sb = struct.pack(
        "<8sIIIIIIIII32s",
        MAGIC,
        BLOCK_SIZE,
        total_blocks,
        inode_count,
        BITMAP_START,
        INODE_TABLE_START,
        DATA_START,
        ROOT_INO,
        total_blocks - used_blocks,
        inode_count - used_inodes,
        volume_name.encode()[:32].ljust(32, b"\0"),
    )
    sb = sb.ljust(BLOCK_SIZE, b"\0")
    img[SUPERBLOCK * BLOCK_SIZE:(SUPERBLOCK + 1) * BLOCK_SIZE] = sb

    with open(path, "wb") as f:
        f.write(img)
    print(f"mke1fss: wrote {path} ({len(img)} bytes, {len(files)} files)")


if __name__ == "__main__":
    brand = sys.argv[2] if len(sys.argv) > 2 else "table"
    edition = sys.argv[3] if len(sys.argv) > 3 else "workstation"
    if brand == "desk":
        brand_name, license_line = "DesKOS", "DesKOS is open source under the MIT License"
    else:
        brand_name, license_line = "tAbLeOS", "tAbLeOS is closed source, all rights reserved"
    files = []
    # user programs built by build.sh
    for name, path in [("ush", "build/ush"), ("hello", "build/hello"),
                       ("e1wm", "build/e1wm"), ("e1apfs", "build/e1apfs"),
                       ("exaltop", "build/exaltop"), ("e1dm", "build/e1dm"),
                       ("e1vfsx", "build/e1vfsx")]:
        if edition == "server" and name in ("e1wm", "e1apfs", "exaltop", "e1dm", "e1vfsx"):
            continue  # server edition has no GUI tools
        if os.path.exists(path):
            with open(path, "rb") as f:
                files.append((name, f.read()))
        else:
            print(f"warning: {path} missing, run ./build.sh userspace first")
    # Preload some files
    files.append(("README", f"{brand_name} - a from-scratch OS ({license_line})\n".encode()))
    files.append(("MOTD", f"Welcome to {brand_name}!\n".encode()))
    # demo.com for e1VFSx (8086 .com image: INT21h AH=09 print + AH=4C exit)
    # loads at DS:0x100; mov dx,0x10B; mov ah,09; int21; mov ah,4C; int21
    demo_com = bytearray()
    demo_com += bytes([0xBA, 0x0B, 0x01])  # mov dx, 0x10B
    demo_com += bytes([0xB4, 0x09])         # mov ah, 0x09
    demo_com += bytes([0xCD, 0x21])         # int 21h
    demo_com += bytes([0xB4, 0x4C])         # mov ah, 0x4C
    demo_com += bytes([0xCD, 0x21])         # int 21h
    demo_com += b"Hello from e1VFSx!$\0\0\0\0"  # $-terminated msg + pad
    files.append(("demo.com", bytes(demo_com)))
    # .rit package payload for the e1wine test (built by build.sh)
    if os.path.exists("build/busybox.rit"):
        with open("build/busybox.rit", "rb") as f:
            files.append(("busybox.rit", f.read()))
    else:
        print("warning: build/busybox.rit missing (e1wine test unavailable)")
    make_image(sys.argv[1] if len(sys.argv) > 1 else "build/e1fss.img", files,
               volume_name=brand_name)
