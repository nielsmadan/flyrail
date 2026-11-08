import argparse
import re
import sys
import tomllib
from pathlib import Path
from typing import Any, NamedTuple

PREFIXES = {"python-v": "python", "rust-v": "rust", "v": "swift"}
TAG = re.compile(r"(?P<prefix>python-v|rust-v|v)(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")


class WorkflowError(Exception):
    pass


class ReleaseTag(NamedTuple):
    component: str
    version: tuple[int, int, int]


def parse_tag(tag: str) -> ReleaseTag | None:
    match = TAG.fullmatch(tag)
    if match is None:
        return None
    prefix, major, minor, patch = match.groups()
    return ReleaseTag(PREFIXES[prefix], (int(major), int(minor), int(patch)))


def _require_tag(tag: str) -> ReleaseTag:
    parsed = parse_tag(tag)
    if parsed is None:
        raise WorkflowError(f"Invalid release tag: {tag!r}")
    return parsed


def _require_version(expected: str, actual: object) -> None:
    if actual != expected:
        raise WorkflowError(f"Tag version {expected!r} does not match project version {actual!r}.")


def _python_prerelease(expected: str, path: Path) -> bool:
    project = tomllib.loads(path.read_text(encoding="utf-8"))["project"]
    if project["name"] != "pyflyrail":
        raise WorkflowError(f"Expected pyflyrail distribution, got {project['name']!r}.")
    _require_version(expected, project["version"])
    return any(
        classifier in project.get("classifiers", [])
        for classifier in (
            "Development Status :: 2 - Pre-Alpha",
            "Development Status :: 3 - Alpha",
            "Development Status :: 4 - Beta",
        )
    )


def _crate_version(manifest: dict[str, Any]) -> object:
    version = manifest["package"]["version"]
    return (
        manifest["workspace"]["package"]["version"] if version == {"workspace": True} else version
    )


def _rust_prerelease(expected: str, path: Path) -> bool:
    manifest = tomllib.loads(path.read_text(encoding="utf-8"))
    name = manifest["package"]["name"]
    if name != "flyrail":
        raise WorkflowError(f"Expected flyrail crate, got {name!r}.")
    _require_version(expected, _crate_version(manifest))
    return expected.startswith("0.")


def _swift_prerelease(expected: str, path: Path) -> bool:
    if re.search(r'Package\(\s*name:\s*"Flyrail"\s*,', path.read_text(encoding="utf-8")) is None:
        raise WorkflowError(f"Expected the Flyrail Swift package in {path}.")
    return expected.startswith("0.")


PRERELEASE = {
    "python": _python_prerelease,
    "rust": _rust_prerelease,
    "swift": _swift_prerelease,
}


def project_metadata(tag: str, path: Path) -> tuple[str, bool]:
    component, version = _require_tag(tag)
    expected = ".".join(map(str, version))
    try:
        return expected, PRERELEASE[component](expected, path)
    except (KeyError, TypeError, tomllib.TOMLDecodeError) as error:
        raise WorkflowError(f"{path} is not a valid {component} manifest: {error!r}") from error


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="operation", required=True)
    metadata = subparsers.add_parser("project-metadata")
    metadata.add_argument("tag")
    metadata.add_argument("project", type=Path)
    args = parser.parse_args()
    project_version, prerelease = project_metadata(args.tag, args.project)
    print(f"version={project_version}")
    print(f"prerelease={str(prerelease).lower()}")


if __name__ == "__main__":
    try:
        main()
    except (WorkflowError, OSError, UnicodeError, KeyboardInterrupt) as error:
        print(f"Release workflow stopped: {error}", file=sys.stderr)
        sys.exit(1)
