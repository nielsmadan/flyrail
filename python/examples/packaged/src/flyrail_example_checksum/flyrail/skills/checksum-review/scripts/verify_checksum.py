#!/usr/bin/env python3
import argparse
import hashlib
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(description="Verify a file against a SHA-256 digest.")
    parser.add_argument("file", type=Path)
    parser.add_argument("expected")
    arguments = parser.parse_args()
    with arguments.file.open("rb") as stream:
        actual = hashlib.file_digest(stream, "sha256").hexdigest()
    print(actual)
    return int(actual != arguments.expected.lower())


if __name__ == "__main__":
    raise SystemExit(main())
