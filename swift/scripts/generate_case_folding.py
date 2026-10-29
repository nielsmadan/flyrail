import argparse
import re
from pathlib import Path


def mappings(source: str) -> tuple[str, list[tuple[int, list[int]]]]:
    first = source.splitlines()[0]
    match = re.fullmatch(r"# CaseFolding-([0-9.]+)\.txt", first)
    if match is None:
        raise ValueError("CaseFolding.txt has an unrecognized version header")
    result: dict[int, list[int]] = {}
    for line in source.splitlines():
        body = line.split("#", 1)[0].strip()
        if not body:
            continue
        code, status, mapping, _ = [part.strip() for part in body.split(";")]
        if status in {"C", "F"}:
            result[int(code, 16)] = [int(item, 16) for item in mapping.split()]
    return match.group(1), sorted(result.items())


def render(version: str, entries: list[tuple[int, list[int]]]) -> str:
    rows = ",\n".join(
        f"        (0x{code:X}, [{', '.join(f'0x{item:X}' for item in values)}])"
        for code, values in entries
    )
    return f"""import Foundation

enum UnicodeCaseFolding {{
    static let version = "{version}"
    static let mappings: [(UInt32, [UInt32])] = [
{rows},
    ]
}}
"""


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    arguments = parser.parse_args()
    version, entries = mappings(arguments.source.read_text(encoding="utf-8"))
    arguments.output.write_text(render(version, entries), encoding="utf-8")


if __name__ == "__main__":
    main()
