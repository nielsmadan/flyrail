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


def declared(manifest: Path) -> dict[str, str]:
    document = tomllib.loads(manifest.read_text(encoding="utf-8"))
    sections = [document.get("dependencies", {}), document.get("build-dependencies", {})]
    entries = [(name, spec) for section in sections for name, spec in section.items()]
    return {
        name: spec if isinstance(spec, str) else spec["version"]
        for name, spec in entries
        if isinstance(spec, str) or "version" in spec
    }


def pin_drift(wanted: dict[str, str], native: dict[str, frozenset[str]]) -> list[str]:
    return [
        f"{name}: native/Cargo.toml declares {version}; "
        f"native/Cargo.lock has {', '.join(sorted(native.get(name, frozenset()))) or 'nothing'}"
        for name, version in sorted(wanted.items())
        if native.get(name) != frozenset({version})
    ]


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Fail when a package shared with the Rust engine lockfile resolves to different "
            "versions, or a direct dependency of native/ does not resolve to its declared version."
        )
    )
    parser.add_argument("native", nargs="?", type=Path, default=PACKAGE / "native" / "Cargo.lock")
    parser.add_argument("engine", nargs="?", type=Path, default=PACKAGE.parent / "rust" / "Cargo.lock")
    parser.add_argument(
        "--manifest", type=Path, default=PACKAGE / "native" / "Cargo.toml"
    )
    arguments = parser.parse_args()
    native = versions(arguments.native)
    problems = [
        f"engine dependency drift: {problem}"
        for problem in drift(native, versions(arguments.engine))
    ] + [f"pinned dependency drift: {problem}" for problem in pin_drift(declared(arguments.manifest), native)]
    for problem in problems:
        print(problem, file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
