#!/usr/bin/env python3
"""push_release_fix.py — push the leak-fix commit + overwrite release assets.

1. Force-push main branch (token-embedded URL, then clean)
2. DELETE old release assets (deskos-workstation, deskos-server, activation-keys.txt)
3. Re-upload fresh assets from local build/
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
TAG = "v0.1-redorange"
BRANCH = "main"
REPO_ROOT = os.path.join(os.path.dirname(__file__), "..")
REPO_ROOT = os.path.normpath(REPO_ROOT)
BUILD_DIR = os.path.join(REPO_ROOT, "build")


def api(method, path, body=None, raw_body=None):
    if raw_body is not None:
        data = raw_body
    elif body is not None:
        data = json.dumps(body).encode()
    else:
        data = None
    req = urllib.request.Request(
        f"{API}{path}", data=data, method=method,
        headers={
            "Authorization": f"Bearer {TOKEN}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "DesKOS-release-fix/0.1",
        },
    )
    try:
        with urllib.request.urlopen(req, context=CTX) as r:
            raw = r.read().decode()
            return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as e:
        raw = e.read().decode()
        raise RuntimeError(f"HTTP {e.code} {method} {path}: {raw[:300]}")


def upload(url, filepath, content_type):
    data = open(filepath, "rb").read()
    req = urllib.request.Request(
        url, data=data, method="POST",
        headers={
            "Authorization": f"Bearer {TOKEN}",
            "Content-Type": content_type,
            "Content-Length": str(len(data)),
        },
    )
    try:
        with urllib.request.urlopen(req, context=CTX) as r:
            raw = r.read().decode()
            return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as e:
        raw = e.read().decode()
        raise RuntimeError(f"HTTP {e.code} upload {filepath}: {raw[:300]}")


# ── Step 1: git push ──────────────────────────────────────
print("=== Step 1: git push ===")
os.chdir(REPO_ROOT)
# commit any last-minute uncommitted changes
st = subprocess.run(["git", "status", "--porcelain"], capture_output=True, text=True)
if st.stdout.strip():
    print("  uncommitted changes — committing...")
    subprocess.run(["git", "add", "-A"], check=True)
    subprocess.run(["git", "commit", "-m", "chore: pre-push snapshot"],
                   capture_output=True)

# token-embedded URL
push_url = f"https://{TOKEN}@github.com/{OWNER}/{REPO}.git"
clean_url = f"https://github.com/{OWNER}/{REPO}.git"

subprocess.run(["git", "remote", "set-url", "origin", push_url],
               capture_output=True, cwd=REPO_ROOT)
try:
    r = subprocess.run(["git", "push", "-u", "origin", BRANCH],
                       capture_output=True, text=True, cwd=REPO_ROOT)
    if r.returncode != 0:
        print(f"  git push FAILED: {r.stderr}")
        sys.exit(1)
    print(f"  ✅ pushed {BRANCH}")
finally:
    subprocess.run(["git", "remote", "set-url", "origin", clean_url],
                   capture_output=True, cwd=REPO_ROOT)

# ── Step 2: DELETE old release assets, then re-upload ─────
print("\n=== Step 2: refresh release assets ===")
rel = api("GET", f"/repos/{OWNER}/{REPO}/releases/tags/{TAG}")
upload_url = rel["upload_url"].split("{")[0]

ASSETS_TO_REFRESH = [
    ("deskos-workstation-0.1-amd64.img", "application/octet-stream"),
    ("deskos-server-0.1-amd64.img",      "application/octet-stream"),
    ("activation-keys.txt",              "text/plain"),
]

# delete all matching old assets first
for a in rel.get("assets", []):
    for fname, _ in ASSETS_TO_REFRESH:
        if a["name"] == fname:
            try:
                api("DELETE", f"/repos/{OWNER}/{REPO}/releases/assets/{a['id']}")
                print(f"  ✗ deleted old asset: {fname} (id={a['id']})")
            except RuntimeError as e:
                print(f"  ✗ delete failed: {e}")
            break

# upload fresh
for fname, ct in ASSETS_TO_REFRESH:
    fpath = os.path.join(BUILD_DIR, fname)
    if not os.path.exists(fpath):
        print(f"  SKIP (not found): {fname}")
        continue
    sz_kb = os.path.getsize(fpath) // 1024
    print(f"  ↑ uploading {fname} ({sz_kb} KB)...")
    upload(f"{upload_url}?name={fname}", fpath, ct)

print("\n=== DONE ===")
print(f"  Repo:    https://github.com/{OWNER}/{REPO}")
print(f"  Release: https://github.com/{OWNER}/{REPO}/releases/tag/{TAG}")
