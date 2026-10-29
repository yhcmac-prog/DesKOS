#!/usr/bin/env python3
"""
sign_github.py — M9: publish DesKOS / tAbLeOS to GitHub.

Two auth paths:
  A) API + git push (default) — uses GH_TOKEN env, macOS keychain, or PAT
  B) Playwright cookie auto-login — legacy visual login check

Publish flow:
  1. Get auth token
  2. Create GitHub repo (default: yhcmac-prog/DesKOS) if missing
  3. git add remote + push current branch
  4. Create release v0.1-redorange with server/workstation images + 100 keys

Usage:
  python3 sign_github.py                      # full publish
  python3 sign_github.py --dry-run            # just show what would happen
  python3 sign_github.py --repo my-repo       # override repo name
  python3 sign_github.py --skip-push          # skip git push
  python3 sign_github.py --cookie-login       # only run playwright cookie test
"""

import argparse
import base64
import json
import os
import ssl
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

# ── SSL fix for macOS Python.org builds ─────────────────────────

try:
    import certifi
    _SSL_CTX = ssl.create_default_context(cafile=certifi.where())
except ImportError:
    _SSL_CTX = ssl._create_unverified_context()
    print("[warn] certifi not installed — SSL verification disabled",
          file=sys.stderr)

# ── Config ──────────────────────────────────────────────────────

GITHUB_API = "https://api.github.com"
DEFAULT_OWNER = "yhcmac-prog"
DEFAULT_REPO = "DesKOS"
DEFAULT_TAG = "v0.1-redorange"
DEFAULT_BRANCH = "main"

# paths — script lives in os/ or os/tAbLeOS/; find the git root
_SCRIPT_DIR = Path(__file__).resolve().parent
# walk up to find a .git dir (for tAbLeOS it's one level up)
def _find_repo_root() -> Path:
    for p in (_SCRIPT_DIR, _SCRIPT_DIR.parent):
        if (p / ".git").exists():
            return p
    # default: prefer tAbLeOS subdir if it exists
    if (_SCRIPT_DIR / "tAbLeOS" / ".git").exists():
        return _SCRIPT_DIR / "tAbLeOS"
    return _SCRIPT_DIR  # fallback

REPO_ROOT = _find_repo_root()
BUILD_DIR = REPO_ROOT / "build"
KEYS_FILE = BUILD_DIR / "activation-keys.txt"

RELEASE_IMAGES = [
    "deskos-workstation-0.1-amd64.img",
    "deskos-server-0.1-amd64.img",
]

# Playwright legacy
COOKIE_FILE = REPO_ROOT / "github_login_cookies.json"
LEGACY_COOKIE = "wYT8SNyOiTplFr_IKpuXq1WMTRI89CW-QB_IouaLENFQwvHB"


# ── Token acquisition ───────────────────────────────────────────

def get_token() -> str:
    """Try GH_TOKEN env → macOS keychain gh:github.com → ask user."""
    tok = os.environ.get("GH_TOKEN", "").strip()
    if tok:
        print(f"[auth] using GH_TOKEN from env (len={len(tok)})")
        return tok

    # macOS keychain (gh CLI stores here)
    try:
        r = subprocess.run(
            ["security", "find-generic-password",
             "-s", "gh:github.com", "-w"],
            capture_output=True, text=True, timeout=5,
        )
        if r.returncode == 0 and r.stdout.strip():
            raw = r.stdout.strip()
            if raw.startswith("go-keyring-base64:"):
                tok = base64.b64decode(raw.split(":", 1)[1]).decode().strip()
            else:
                tok = raw
            if tok.startswith("ghp_") or tok.startswith("github_pat_"):
                print(f"[auth] token from macOS keychain (len={len(tok)})")
                return tok
    except Exception as e:
        print(f"[auth] keychain lookup failed: {e}")

    # ask user
    print("\n[auth] No GitHub token found.")
    print("       Create one at https://github.com/settings/tokens")
    print("       Needs: repo (full control)")
    tok = input("Paste token (or set GH_TOKEN env): ").strip()
    if tok.startswith("ghp_") or tok.startswith("github_pat_"):
        return tok
    print("ERROR: token doesn't look like a GitHub PAT (ghp_...)")
    sys.exit(1)


# ── GitHub API helpers ──────────────────────────────────────────

def gh_api(method: str, path: str, token: str,
           body: dict | None = None) -> dict:
    data = json.dumps(body).encode() if body else None
    req = urllib.request.Request(
        f"{GITHUB_API}{path}",
        data=data, method=method,
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "DesKOS-sign-github/0.1",
        },
    )
    try:
        with urllib.request.urlopen(req, context=_SSL_CTX) as r:
            raw = r.read().decode()
            return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as e:
        raw = e.read().decode()
        raise RuntimeError(f"HTTP {e.code} {method} {path}: {raw}") from e


def gh_upload(url: str, token: str, filepath: Path, content_type: str) -> dict:
    data = filepath.read_bytes()
    req = urllib.request.Request(
        url, data=data, method="POST",
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": content_type,
            "Content-Length": str(len(data)),
        },
    )
    try:
        with urllib.request.urlopen(req, context=_SSL_CTX) as r:
            raw = r.read().decode()
            return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as e:
        raw = e.read().decode()
        raise RuntimeError(f"HTTP {e.code} upload {filepath.name}: {raw}") from e


# ── Steps ───────────────────────────────────────────────────────

def step_auth(token: str) -> dict:
    user = gh_api("GET", "/user", token)
    print(f"[auth] OK — logged in as '{user['login']}' "
          f"({user.get('name', '')})")
    return user


def step_create_repo(token: str, owner: str, repo: str, dry_run: bool) -> str:
    try:
        gh_api("GET", f"/repos/{owner}/{repo}", token)
        print(f"[repo] {owner}/{repo} already exists — reusing it")
    except RuntimeError:
        if dry_run:
            print(f"[repo] DRY-RUN: would create {owner}/{repo}")
        else:
            body = {
                "name": repo,
                "description": "DesKOS (tAbLeOS) — a tiny x86_64 hobby OS. MBR → stage2 → Rust kernel → ring3 userspace. Server + Workstation dual-branch.",
                "homepage": "https://github.com/yhcmac-prog",
                "private": False,
                "auto_init": False,
                "has_issues": True,
                "has_wiki": True,
                "has_projects": False,
            }
            r = gh_api("POST", "/user/repos", token, body)
            print(f"[repo] created: {r['html_url']}")
    return f"https://github.com/{owner}/{repo}.git"


def step_push(repo_url: str, branch: str, tag: str, token: str, dry_run: bool) -> None:
    if dry_run:
        print(f"[push] DRY-RUN: would configure origin + push {branch} + tag {tag}")
        return

    # ensure git repo
    try:
        subprocess.run(["git", "rev-parse", "--git-dir"],
                       check=True, capture_output=True, cwd=REPO_ROOT)
    except subprocess.CalledProcessError:
        print(f"[push] ERROR: no git repo at {REPO_ROOT}")
        print("       Run: cd tAbLeOS && git init -b main && git add -A && git commit")
        sys.exit(1)

    # git identity
    subprocess.run(["git", "config", "user.name", "redorange"],
                   capture_output=True, cwd=REPO_ROOT)
    subprocess.run(["git", "config", "user.email", "yhc_mac@126.com"],
                   capture_output=True, cwd=REPO_ROOT)

    # token-embed URL (kept for entire push session)
    owner_slash_repo = repo_url.replace("https://github.com/", "")
    push_url = f"https://{token}@github.com/{owner_slash_repo}"

    existing = subprocess.run(
        ["git", "remote", "get-url", "origin"],
        capture_output=True, text=True, cwd=REPO_ROOT,
    )
    if existing.returncode == 0:
        subprocess.run(["git", "remote", "set-url", "origin", push_url],
                       check=True, cwd=REPO_ROOT)
    else:
        subprocess.run(["git", "remote", "add", "origin", push_url],
                       check=True, cwd=REPO_ROOT)
    print("[push] origin configured (token embedded)")

    # commit uncommitted changes
    status = subprocess.run(
        ["git", "status", "--porcelain"],
        capture_output=True, text=True, cwd=REPO_ROOT,
    )
    if status.stdout.strip():
        print("[push] uncommitted changes — committing...")
        subprocess.run(["git", "add", "-A"], check=True, cwd=REPO_ROOT)
        subprocess.run(
            ["git", "commit", "-m", "chore: pre-push snapshot"],
            capture_output=True, cwd=REPO_ROOT,
        )

    # push branch
    print(f"[push] pushing {branch} → origin...")
    result = subprocess.run(
        ["git", "push", "-u", "origin", branch],
        capture_output=True, text=True, cwd=REPO_ROOT,
    )
    if result.returncode != 0:
        print(f"[push] FAILED: {result.stderr}")
        # clean up before exit
        subprocess.run(["git", "remote", "set-url", "origin", repo_url],
                       capture_output=True, cwd=REPO_ROOT)
        sys.exit(1)

    # push tag (if it exists locally)
    tag_exists = subprocess.run(
        ["git", "tag", "-l", tag],
        capture_output=True, text=True, cwd=REPO_ROOT,
    )
    if tag in tag_exists.stdout.split():
        print(f"[push] pushing tag {tag} → origin...")
        result2 = subprocess.run(
            ["git", "push", "origin", tag],
            capture_output=True, text=True, cwd=REPO_ROOT,
        )
        if result2.returncode != 0:
            print(f"[push] tag {tag} FAILED: {result2.stderr}")

    # always strip token at the very end
    subprocess.run(["git", "remote", "set-url", "origin", repo_url],
                   capture_output=True, cwd=REPO_ROOT)
    print("[push] OK — origin cleaned to unauthenticated URL")


def step_release(token: str, owner: str, repo: str, tag: str,
                 dry_run: bool) -> None:
    """Create GitHub release for an already-pushed tag + upload assets."""
    # release body
    release_body = f"""## DesKOS / tAbLeOS — {tag}

**Codename: redorange** (DesKOS 1.x)

### What is tAbLeOS?
A minimal x86_64 hobby operating system: MBR bootsector → stage2 → Rust kernel → ring3 userspace. Server + Workstation dual branch.

### Build
```bash
BRAND=desk EDITION=workstation ./build.sh
BRAND=desk EDITION=server      ./build.sh
```

### Run in VirtualBox
```bash
VBoxManage convertfromraw build/deskos-workstation-0.1-amd64.img deskos.vdi --format VDI
# attach to IDE controller, boot
```

### Included
| File | Description |
|------|-------------|
| `deskos-workstation-0.1-amd64.img` | GUI framebuffer + VGA console (8 MB) |
| `deskos-server-0.1-amd64.img`      | UART headless server (8 MB) |
| `activation-keys.txt`              | 100 one-time public test keys + 1 permanent key |

### Activation keys
Public one-time keys (00–99) — free for testing, each works once.
See `activation-keys.txt` attached.

### Repo
https://github.com/{owner}/{repo}
"""

    if dry_run:
        print(f"[release] DRY-RUN: would create release {tag}")
        return

    # reuse or create release
    try:
        existing = gh_api(
            "GET", f"/repos/{owner}/{repo}/releases/tags/{tag}", token)
        upload_url = existing["upload_url"].split("{")[0]
        print(f"[release] release {tag} exists — reusing for uploads")
    except RuntimeError:
        body = {
            "tag_name": tag,
            "name": f"DesKOS / tAbLeOS {tag} (redorange)",
            "body": release_body,
            "draft": False,
            "prerelease": False,
            "target_commitish": DEFAULT_BRANCH,
        }
        r = gh_api("POST", f"/repos/{owner}/{repo}/releases", token, body)
        upload_url = r["upload_url"].split("{")[0]
        print(f"[release] created: {r['html_url']}")

    # upload assets
    for fname in RELEASE_IMAGES + ["activation-keys.txt"]:
        fpath = BUILD_DIR / fname
        if not fpath.exists():
            print(f"[release] SKIP (not found): {fname}")
            continue
        ct = "application/octet-stream"
        if fname.endswith(".txt"):
            ct = "text/plain"
        url = f"{upload_url}?name={fname}"
        sz_kb = fpath.stat().st_size // 1024
        print(f"[release] uploading {fname} ({sz_kb} KB)...")
        gh_upload(url, token, fpath, ct)
    print("[release] assets uploaded — done!")


# ── Legacy: Playwright cookie login ────────────────────────────

def cookie_auto_login():
    """Visual check: inject a user_session cookie, open github.com."""
    from playwright.sync_api import sync_playwright

    with sync_playwright() as p:
        browser = p.chromium.launch(headless=False)
        context = browser.new_context()
        context.add_cookies([{
            "name": "user_session",
            "value": LEGACY_COOKIE,
            "domain": ".github.com",
            "path": "/",
        }])
        page = context.new_page()
        page.goto("https://github.com")
        print("✅ Cookie injected — check the browser window")
        page.wait_for_timeout(5000)
        browser.close()


# ── Main ────────────────────────────────────────────────────────

def main():
    ap = argparse.ArgumentParser(description="M9: publish DesKOS to GitHub")
    ap.add_argument("--repo", default=DEFAULT_REPO)
    ap.add_argument("--owner", default=DEFAULT_OWNER)
    ap.add_argument("--tag", default=DEFAULT_TAG)
    ap.add_argument("--branch", default=DEFAULT_BRANCH)
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--skip-push", action="store_true")
    ap.add_argument("--cookie-login", action="store_true",
                    help="only run playwright cookie login visual test")
    args = ap.parse_args()

    if args.cookie_login:
        cookie_auto_login()
        return

    print(f"\n=== M9 sign_github.py ===  dry_run={args.dry_run}")
    print(f"  repo root: {REPO_ROOT}")
    print(f"  build dir: {BUILD_DIR}\n")

    token = get_token()
    user = step_auth(token)

    repo_url = step_create_repo(token, args.owner, args.repo, args.dry_run)

    # create local tag BEFORE push so step_push can push both
    if not args.dry_run:
        existing_tags = subprocess.run(
            ["git", "tag", "-l", args.tag],
            capture_output=True, text=True, cwd=REPO_ROOT,
        )
        if args.tag not in existing_tags.stdout.split():
            subprocess.run(
                ["git", "tag", "-a", args.tag,
                 "-m", f"DesKOS / tAbLeOS release {args.tag} (codename: redorange)"],
                check=True, capture_output=True, cwd=REPO_ROOT,
            )
            print(f"[tag] local {args.tag} created")
        else:
            print(f"[tag] local {args.tag} already exists")

    if not args.skip_push:
        step_push(repo_url, args.branch, args.tag, token, args.dry_run)
    else:
        print("[push] skipped (--skip-push)")

    step_release(token, args.owner, args.repo, args.tag, args.dry_run)

    print("\n=== M9 DONE ===")
    print(f"  Repo:    https://github.com/{args.owner}/{args.repo}")
    print(f"  Release: https://github.com/{args.owner}/{args.repo}/releases/tag/{args.tag}")
    print(f"  User:    {user['login']}")


if __name__ == "__main__":
    main()
