#!/usr/bin/env python3
"""push_via_api.py — push local files to DesKOS remote via GitHub Contents API.

Used when git push to github.com is blocked but api.github.com works.
"""
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
TOKEN = open(os.path.join(os.path.dirname(__file__), "..", "..", "token.txt")).read().strip()
OWNER, REPO = "yhcmac-prog", "DesKOS"
BRANCH = "main"
REPO_ROOT = os.path.join(os.path.dirname(__file__), "..")
REPO_ROOT = os.path.normpath(REPO_ROOT)


def api(method, path, body=None):
    data = json.dumps(body).encode() if body else None
    req = urllib.request.Request(
        f"{API}{path}", data=data, method=method,
        headers={
            "Authorization": f"Bearer {TOKEN}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "DesKOS-api-push/0.1",
        },
    )
    try:
        with urllib.request.urlopen(req, context=CTX) as r:
            raw = r.read().decode()
            return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as e:
        raw = e.read().decode()
        raise RuntimeError(f"HTTP {e.code} {method} {path}: {raw[:300]}")


def push_file(local_path, remote_path, message):
    """Push (update or create) a file to the remote repo."""
    content = open(local_path, "rb").read()
    encoded = base64.b64encode(content).decode()

    body = {
        "message": message,
        "content": encoded,
        "branch": BRANCH,
    }

    # GET current sha (if file exists)
    try:
        r = api("GET", f"/repos/{OWNER}/{REPO}/contents/{remote_path}")
        sha = r["sha"]
        body["sha"] = sha
        verb = "update"
    except RuntimeError as e:
        if "404" in str(e):
            verb = "create"
        else:
            raise

    r = api("PUT", f"/repos/{OWNER}/{REPO}/contents/{remote_path}", body)
    print(f"  ✅ {verb}d  {remote_path}  ({len(content)} bytes)")


FILES_TO_PUSH = [
    ("build.sh",                       "build.sh",                       "fix: exclude table-exclusive apps from DesKOS builds"),
    ("tools/mke1fss.py",               "tools/mke1fss.py",               "fix: exclude table-exclusive apps from DesKOS rootfs"),
    ("kernel/src/brand.rs",            "kernel/src/brand.rs",            "fix: zero PERM_FULL placeholder (real value in private tree)"),
    ("userspace/ush/src/main.rs",      "userspace/ush/src/main.rs",      "fix: cfg-brand banner split (desk=MIT, table=Closed source)"),
    ("tools/mkkeys.py",                "tools/mkkeys.py",                "fix: drop PERM_BODY, keep one-time keys only"),
    (".gitignore",                     ".gitignore",                     "add activation-keys.txt / license.cfg / *.key / *.perm"),
]

print("=== Pushing fixes via GitHub Contents API ===")
for local_rel, remote_path, msg in FILES_TO_PUSH:
    local_full = os.path.join(REPO_ROOT, local_rel)
    if not os.path.exists(local_full):
        print(f"  SKIP (not found): {local_rel}")
        continue
    try:
        push_file(local_full, remote_path, msg)
    except RuntimeError as e:
        print(f"  ✗ FAILED {remote_path}: {e}")

print("\n=== DONE ===")
