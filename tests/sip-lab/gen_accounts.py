#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Generate throwaway SIP test accounts for the sip-lab PBX.

The passwords are deliberately obvious and non-sensitive: this is a loopback
test lab, not a production PBX. Never reuse these anywhere else.

Usage:
    python3 gen_accounts.py [--out accounts.json] [--count 3] [--force]
"""
from __future__ import annotations

import argparse
import json
import os
import sys

LAB = os.path.dirname(os.path.abspath(__file__))


def main() -> int:
    ap = argparse.ArgumentParser(description="Generate sip-lab test accounts")
    ap.add_argument("--out", default=os.path.join(LAB, "accounts.json"))
    ap.add_argument("--count", type=int, default=3,
                    help="number of extensions 1001..100N (default 3)")
    ap.add_argument("--realm", default="sip-lab")
    ap.add_argument("--force", action="store_true",
                    help="overwrite an existing accounts.json")
    args = ap.parse_args()

    if os.path.exists(args.out) and not args.force:
        print("REFUSED: %s already exists (use --force to overwrite)"
              % args.out, file=sys.stderr)
        return 2

    users = {}
    for i in range(1, args.count + 1):
        ext = str(1000 + i)
        users[ext] = {
            # Obvious, throwaway, loopback-only. NOT a real credential.
            "password": "lab-%s-test" % ext,
            "name": "test ext %s" % ext,
        }

    doc = {
        "_comment": ("sip-lab throwaway test accounts — loopback only, "
                     "digest MD5. NOT real credentials; do not reuse."),
        "realm": args.realm,
        "users": users,
    }
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(doc, f, indent=2, ensure_ascii=False)
        f.write("\n")
    os.chmod(args.out, 0o600)

    print("wrote %s (mode 600):" % args.out)
    for ext, v in users.items():
        print("  ext %s  password %s  (%s)" % (ext, v["password"], v["name"]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
