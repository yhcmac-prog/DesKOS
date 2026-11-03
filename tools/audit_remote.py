#!/usr/bin/env python3
"""audit_remote.py — scan the public DesKOS GitHub repo for tAbLeOS leaks."""
import base64
import json
import os
import ssl
import sys
import urllib.error
import urllib.request

try:
    import certifi
    CTX = ssl.create_default_context(cafile=certifi.where())
except ImportError:
    CTX = ssl._create_unverified_context()

API = "https://api.github.com"
TOKEN_PATH = os.path.join(os.path.dirname(__file__), "..", "..", "token.txt")
TOKEN = open(TOKEN_PATH).read().strip()
OWNER, REPO = "yhcmac-prog", "DesKOS"


def api(method, path):
    req = urllib.request.Request(
        f"{API}{path}", method=method,
        headers={
            "Authorization": f"Bearer {TOKEN}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "DesKOS-audit/0.1",
        },
    )
    with urllib.request.urlopen(req, context=CTX) as r:
        raw = r.read().decode()
        return json.loads(raw) if raw else {}


def get_text(path):
    r = api("GET", f"/repos/{OWNER}/{REPO}/contents/{path}")
    return base64.b64decode(r["content"]).decode()


# ── 1. apps/ listing ──────────────────────────────────────
print("=== [1] apps/ directory (remote) ===")
r = api("GET", f"/repos/{OWNER}/{REPO}/contents/apps")
for item in r:
    sz = item.get("size", "-")
    print(f"  {item['type']:8}  {item['name']:20}  size={sz}")

# ── 2. full tree scan ─────────────────────────────────────
print("\n=== [2] Repo tree — table-exclusive leak hunt ===")
tree = api("GET", f"/repos/{OWNER}/{REPO}/git/trees/main?recursive=5")
LEAK_PATTERNS = [
    ("apps/e1apfs",            "APFS browser app (table exclusive)"),
    ("apps/e1vfsx",            "8086 VM app (table exclusive)"),
    ("e1apfs",                 "any e1apfs blob anywhere"),
    ("e1vfsx",                 "any e1vfsx blob anywhere"),
    ("activation-keys.txt",    "activation keys file in git"),
    ("license.cfg",            "license config"),
    (".perm",                  "permanent key files"),
]
leaks = []
for leaf in tree.get("tree", []):
    p = leaf["path"]
    if leaf["type"] != "blob":
        continue
    for pat, why in LEAK_PATTERNS:
        if pat in p:
            leaks.append((p, why))
            break

if leaks:
    print(f"  ⚠️  LEAKS FOUND ({len(leaks)}):")
    for p, why in leaks:
        print(f"    ✗ {p}  ({why})")
else:
    print("  ✅ CLEAN — no obvious table-exclusive paths")

# ── 3. release assets ────────────────────────────────────
print("\n=== [3] Release assets (v0.1-redorange) ===")
try:
    rel = api("GET", f"/repos/{OWNER}/{REPO}/releases/tags/v0.1-redorange")
    for a in rel.get("assets", []):
        kb = a["size"] // 1024
        print(f"  {a['name']:50s}  {kb:>8} KB  id={a['id']}")
except urllib.error.HTTPError as e:
    print(f"  HTTP {e.code} — {e.read().decode()[:100]}")

# ── 4. brand.rs contents ─────────────────────────────────
print("\n=== [4] kernel/src/brand.rs (remote) ===")
try:
    txt = get_text("kernel/src/brand.rs")
    for i, line in enumerate(txt.split("\n"), 1):
        tag = ""
        if "PERM_FULL" in line or "ANDFREE" in line or "PERMANDFREEAAMO" in line:
            tag = "  ⚠️ PERM-KEY"
        elif "brand_name" in line.lower() or "BRAND_NAME" in line:
            tag = "  ← brand"
        print(f"  {i:3}: {line}{tag}")
except urllib.error.HTTPError as e:
    print(f"  HTTP {e.code}")

# ── 5. build.sh contents (relevant section) ──────────────
print("\n=== [5] build.sh (remote, userspace build section) ===")
try:
    txt = get_text("build.sh")
    for i, line in enumerate(txt.split("\n"), 1):
        if 30 <= i <= 70:
            tag = ""
            if "e1vfsx" in line or "e1apfs" in line:
                tag = "  ⚠️ table-excl"
            print(f"  {i:3}: {line}{tag}")
except urllib.error.HTTPError as e:
    print(f"  HTTP {e.code}")

# ── 6. mke1fss.py contents (relevant section) ─────────────
print("\n=== [6] tools/mke1fss.py (remote, __main__ block) ===")
try:
    txt = get_text("tools/mke1fss.py")
    in_main = False
    for i, line in enumerate(txt.split("\n"), 1):
        if "__name__" in line and "__main__" in line:
            in_main = True
        if in_main:
            tag = ""
            if "e1vfsx" in line or "e1apfs" in line or "demo.com" in line:
                tag = "  ⚠️ unconditional"
            print(f"  {i:3}: {line}{tag}")
except urllib.error.HTTPError as e:
    print(f"  HTTP {e.code}")

print("\n=== DONE ===")
