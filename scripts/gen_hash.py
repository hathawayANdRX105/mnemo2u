#!/usr/bin/env python3
"""Generate cache-key golden values for `compute_args_hash` parity.

Python: `md5(str((model, messages)).encode())` — `_utils.py:216`.

Usage:
    .venv-ref/bin/python scripts/gen_hash.py > tests/fixtures/hash_golden.json
"""

import hashlib
import json
import sys

CASES = [
    ("gpt-4o", [{"role": "user", "content": "hi"}]),
    ("gpt-4o-mini", [{"role": "system", "content": "sys"}, {"role": "user", "content": "a\nb"}]),
    ("gpt-4o", [{"role": "user", "content": "it's \"quoted\""}]),
    ("gpt-4o", []),
]


def main():
    payload = [
        {
            "model": model,
            "messages": messages,
            "expected_md5": hashlib.md5(str((model, messages)).encode()).hexdigest(),
        }
        for model, messages in CASES
    ]
    json.dump(payload, sys.stdout, ensure_ascii=False, indent=1)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
