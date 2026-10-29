#!/usr/bin/env python3
"""mkrit.py - pack a directory into a .rit (DBPF-style) package."""

import struct
import sys
import os

MAGIC = b"RIFFrit\0"

TYPE_ELF = 0x0001
TYPE_LIB = 0x0002
TYPE_DATA = 0x0010
TYPE_META = 0x0100


def pack_rit(name, files, out_path):
    """Pack files into a .rit archive.

    files: list of (type_id, instance_id, bytes)
    """
    entries = []
    offset = 32  # after header

    # Meta entry first (name)
    meta = name.encode()
    entries.append((TYPE_META, 0, 0, offset, len(meta), 0))
    offset += len(meta)

    data_blobs = [meta]

    for type_id, instance, data in files:
        entries.append((type_id, 0, instance, offset, len(data), 0))
        offset += len(data)
        data_blobs.append(data)

    # Align the index table to 8 bytes (consumers read u32 fields)
    pad = (-offset) % 8
    if pad:
        data_blobs.append(b"\0" * pad)
        offset += pad

    # Build index
    index = b""
    for type_id, group, inst, off, size, flags in entries:
        index += struct.pack("<IIIIII", type_id, group, inst, off, size, flags)

    # Build header
    index_off = offset
    header = struct.pack(
        "<8sIIII8s",
        MAGIC,
        1,
        len(entries),
        index_off,
        len(index),
        b"\0" * 8,
    )

    # Write file
    with open(out_path, "wb") as f:
        f.write(header)
        for blob in data_blobs:
            f.write(blob)
        f.write(index)
    print(f"mkrit: wrote {out_path} ({os.path.getsize(out_path)} bytes, {len(entries)} entries)")


if __name__ == "__main__":
    if len(sys.argv) < 3:
        print("usage: mkrit.py <name> <out.rit> [file1 file2 ...]")
        sys.exit(1)
    name = sys.argv[1]
    out = sys.argv[2]
    files = []
    for i, path in enumerate(sys.argv[3:]):
        with open(path, "rb") as f:
            data = f.read()
        # heuristic: ELF files are executables
        type_id = TYPE_ELF if data[:4] == b"\x7FELF" else TYPE_DATA
        files.append((type_id, i + 1, data))
    pack_rit(name, files, out)
