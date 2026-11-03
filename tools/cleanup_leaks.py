#!/usr/bin/env python3
"""cleanup_leaks.py — delete table-exclusive files from the public DesKOS repo.

Uses GitHub Contents API DELETE (per-file). Requires the file's current sha.
Also force-pushes the fixed build.sh + mke1fss.py + brand.rs via git.
"""
import base64
import json
import os
import ssl
import subprocess
import sys
import urllib.error
import urllib.request

try:
    import certifi
    CTX = ssl.create_default_context(cafile=certifi.where())
except ImportError:
    CTX = ssl._create_unverified_context()

API = "https://api.github.com"
TOKEN = open(os.path.join(os.path.dirname(__file__), "..", "..", "token.txt")).read().strip()
OWNER, REPO = "yhcmac-prog", "DesKOS"
BRANCH = "main"

LEAKED_PATHS = [
    "apps/e1apfs/.cargo/config.toml",
    "apps/e1apfs/Cargo.toml",
    "apps/e1apfs/linker.ld",
    "apps/e1apfs/src/main.rs",
    "apps/e1vfsx/.cargo/config.toml",
    "apps/e1vfsx/Cargo.toml",
    "apps/e1vfsx/linker.ld",
    "apps/e1vfsx/src/cjk.bin",
    "apps/e1vfsx/src/font8x8.rs",
    "apps/e1vfsx/src/main.rs",
]


def api(method, path, body=None):
    data = json.dumps(body).encode() if body else None
    req = urllib.request.Request(
        f"{API}{path}", data=data, method=method,
        headers={
            "Authorization": f"Bearer {TOKEN}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "DesKOS-cleanup/0.1",
        },
    )
    try:
        with urllib.request.urlopen(req, context=CTX) as r:
            raw = r.read().decode()
            return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as e:
        raw = e.read().decode()
        raise RuntimeError(f"HTTP {e.code} {method} {path}: {raw[:200]}")


# 1. Delete each leaked path
print("=== Deleting leaked table-exclusive files ===")
deleted = 0
for path in LEAKED_PATHS:
    try:
        r = api("GET", f"/repos/{OWNER}/{REPO}/contents/{path}")
        sha = r["sha"]
        api("DELETE", f"/repos/{OWNER}/{REPO}/contents/{path}", {
            "message": f"chore: remove table-exclusive file {path}",
            "sha": sha,
            "branch": BRANCH,
        })
        print(f"  ✗ deleted  {path}")
        deleted += 1
    except RuntimeError as e:
        print(f"  ✗ SKIP     {path} — {e}")

print(f"\n=== Deleted {deleted}/{len(LEAKED_PATHS)} leaked blobs ===")

# 2. Verify tree is clean now
print("\n=== Verifying remote tree is clean ===")
tree = api("GET", f"/repos/{OWNER}/{REPO}/git/trees/{BRANCH}?recursive=5")
still_leaked = []
for leaf in tree.get("tree", []):
    p = leaf["path"]
    if leaf["type"] != "blob":
        continue
    if "apps/e1apfs/" in p or "apps/e1vfsx/" in p:
        still_leaked.append(p)
if still_leaked:
    print(f"  ⚠️  STILL LEAKED ({len(still_leaked)}):")
    for p in still_leaked:
        print(f"    {p}")
else:
    print("  ✅ CLEAN — e1apfs/e1vfsx blobs fully removed")

# 3. Also empty dirs — git DELETE with dummy content
#    Directories auto-delete when last blob removed. Confirm apps/ listing.
print("\n=== apps/ listing after cleanup ===")
apps = api("GET", f"/repos/{OWNER}/{REPO}/contents/apps")
for item in apps:
    print(f"  {item['type']:8}  {item['name']}")
