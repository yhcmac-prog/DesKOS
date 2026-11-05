#!/usr/bin/env python3
"""compare_builds.py — desk vs table e1fss file list."""
import struct, os, subprocess, sys

ROOT = "/Users/macbook/Desktop/os/tAbLeOS"

def run(brand, edition):
    subprocess.run(
        ["python3", "tools/mke1fss.py", "build/e1fss.img", brand, edition],
        check=True, cwd=ROOT, capture_output=True, text=True)
    with open(os.path.join(ROOT, "build/e1fss.img"), "rb") as f:
        data = f.read()
    dir_block = data[34*4096 : 34*4096+4096]
    files = []
    for i in range(0, 4096, 64):
        ino, nlen, name = struct.unpack("<IB59s", dir_block[i:i+64])
        if ino == 0 and nlen == 0:
            break
        n = name[:nlen].rstrip(b"\x00").decode()
        files.append(n)
    return files

print("=" * 55)
print("  DesKOS  vs  tAbLeOS  —  rootfs 比较")
print("=" * 55)

for brand, edition in [("desk", "workstation"), ("table", "workstation"),
                       ("desk", "server"),     ("table", "server")]:
    files = run(brand, edition)
    print(f"\n[{brand:5s} / {edition:12s}]  {len(files)} files:")
    exclusive = {"e1apfs", "e1vfsx", "demo.com"}
    for f in files:
        tag = "  🔒 table only" if f in exclusive else ""
        print(f"    {f:12}{tag}")
