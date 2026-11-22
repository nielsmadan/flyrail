import argparse
import sys
import tomllib
from collections import defaultdict
from pathlib import Path

PACKAGE = Path(__file__).resolve().parent.parent


def versions(lock: Path) -> dict[str, frozenset[str]]:
    packages = tomllib.loads(lock.read_text(encoding="utf-8")).get("package", [])
    found: defaultdict[str, set[str]] = defaultdict(set)
    for package in packages:
        found[package["name"]].add(package["version"])
    return {name: frozenset(versions) for name, versions in found.items()}


def drift(native: dict[str, frozenset[str]], engine: dict[str, frozenset[str]]) -> list[str]:
    return [
        f"{name}: native/Cargo.lock has {', '.join(sorted(native[name]))}; "
        f"rust/Cargo.lock has {', '.join(sorted(engine[name]))}"
        for name in sorted(native.keys() & engine.keys())
        if native[name] != engine[name]
    ]


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Fail when a package shared with the Rust engine lockfile resolves to different versions."
    )
    parser.add_argument("native", nargs="?", type=Path, default=PACKAGE / "native" / "Cargo.lock")
    parser.add_argument("engine", nargs="?", type=Path, default=PACKAGE.parent / "rust" / "Cargo.lock")
    arguments = parser.parse_args()
    problems = drift(versions(arguments.native), versions(arguments.engine))
    for problem in problems:
        print(f"engine dependency drift: {problem}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
