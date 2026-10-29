#!/usr/bin/env python3
"""mkkeys.py - generate activation keys for tAbLeOS.

Algorithm (must match kernel brand.rs verify_key exactly):
  1. Strip dashes: "XXXX-XXXX-XXXX-XXXX" → 16 chars
  2. Map each char to 0..35 (A-Z=0..25, 0-9=26..35)
  3. Polynomial rolling hash: acc = (acc * 36 + val) mod 65521, starting acc=0
  4. VALID iff hash of all 16 chars == 0

Key layout (valid=hash==0):
  Permanent: payload[0..3] = 'PERM', payload[4..11] = fixed ("ANDFREE"),
              payload[12..15] = checksum → 16 chars total
  One-time:  payload[0..1] = 2-digit serial (00..99), payload[2..11] = random,
              payload[12..15] = checksum → 16 chars total

Verification for one-time also checks serial < 100 AND serial not consumed
(recorded in license.cfg on the e1fss root after activation).
"""
import secrets
import sys

MOD = 65521  # largest prime < 2^16
CHARSET = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
PERM_PREFIX = "PERM"
PERM_BODY = "ANDFREE"


def char_val(c):
    i = CHARSET.index(c)
    return i


def rolling_hash(chars):
    acc = 0
    for c in chars:
        acc = (acc * 36 + char_val(c)) % MOD
    return acc


def make_key(payload12):
    """Given the first 12 chars (prefix+serial+random), compute 4 checksum
    chars ck such that rolling_hash(payload12 + ck) == 0.

    Since rolling_hash(payload12 + ck) = (rolling_hash(payload12) * 36^4 +
    rolling_hash(ck)) mod MOD, we need rolling_hash(ck) == target where
    target = (MOD - rolling_hash(payload12) * 36^4) mod MOD.
    We brute-force 4-char combinations to find one whose hash equals target.
    """
    h12 = rolling_hash(payload12)
    power = pow(36, 4, MOD)
    target = (MOD - (h12 * power) % MOD) % MOD
    # brute-force ck (36^4 = 1679616, fast in Python)
    for c3 in range(36):
        for c2 in range(36):
            for c1 in range(36):
                # for each single c0, rolling_hash of 4 chars is:
                #   (((0*36 + c3)*36 + c2)*36 + c1)*36 + c0  = c3*36^3 + c2*36^2 + c1*36 + c0
                # we need this == target
                head = (((0 * 36 + c3) * 36 + c2) * 36 + c1) % MOD
                c0 = (target - head * 36 + MOD) % MOD
                if c0 < 36:
                    ck = CHARSET[c3] + CHARSET[c2] + CHARSET[c1] + CHARSET[c0]
                    full = payload12 + ck
                    assert rolling_hash(full) == 0, "hash mismatch in verify"
                    return "-".join([full[0:4], full[4:8], full[8:12], full[12:16]])
    raise RuntimeError("no ck found — should never happen")


def gen_perm_key():
    payload12 = (PERM_PREFIX + PERM_BODY).ljust(12, "A")
    return make_key(payload12)


def gen_one_time_key(serial):
    assert 0 <= serial < 100
    sn = f"{serial:02d}"
    rand = "".join(secrets.choice(CHARSET) for _ in range(10))
    payload12 = sn + rand
    return make_key(payload12)


def main():
    out_path = sys.argv[1] if len(sys.argv) > 1 else "build/activation-keys.txt"
    perm = gen_perm_key()
    ones = [gen_one_time_key(i) for i in range(100)]
    with open(out_path, "w") as f:
        f.write(f"# tAbLeOS activation keys - {out_path}\n")
        f.write(f"# permanent (personal use):\n")
        f.write(f"PERM {perm}\n")
        f.write(f"# one-time (public test, 100 keys, serial 00..99):\n")
        for i, k in enumerate(ones):
            f.write(f"{i:02d} {k}\n")
    print(f"mkkeys: wrote {out_path} (1 perm + 100 one-time)")
    # sanity verify
    assert verify(perm), f"perm key verify fail {perm}"
    for i, k in enumerate(ones):
        assert verify(k, serial=i), f"serial {i} verify fail {k}"
    print("all keys self-verified ✓")


def verify(key_str, serial=None):
    """Python-side verify (mirrors kernel brand.rs). Used for self-check."""
    cleaned = key_str.replace("-", "").upper()
    if len(cleaned) != 16 or any(c not in CHARSET for c in cleaned):
        return False
    h = rolling_hash(cleaned)
    if h != 0:
        return False
    # perm: payload[0..4] == "PERM"
    if cleaned[0:4] == PERM_PREFIX:
        return True
    # one-time: payload[0..2] must be serial
    if serial is not None:
        sn_char = f"{serial:02d}"
        if cleaned[0:2] != sn_char:
            return False
    return True


if __name__ == "__main__":
    main()
