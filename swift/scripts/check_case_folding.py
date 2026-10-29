import sys
import tarfile
import tomllib
import unicodedata
from pathlib import Path

sys.dont_write_bytecode = True

from generate_case_folding import mappings, render


def expected_folding(entries: list[tuple[int, list[int]]]) -> dict[int, str]:
    return {
        code: "".join(chr(item) for item in values)
        for code, values in entries
    }


def python_folding() -> dict[int, str]:
    result: dict[int, str] = {}
    for code in range(sys.maxunicode + 1):
        if 0xD800 <= code <= 0xDFFF:
            continue
        source = chr(code)
        folded = source.casefold()
        if folded != source:
            result[code] = folded
    return result


def rust_snapshot(root: Path) -> bytes | None:
    archives = tuple((root / ".cache/cargo-home/registry/cache").glob("**/caseless-0.2.2.crate"))
    if not archives:
        return None
    with tarfile.open(archives[0], mode="r:gz") as archive:
        member = archive.getmember("caseless-0.2.2/CaseFolding.txt")
        extracted = archive.extractfile(member)
        if extracted is None:
            raise ValueError("Rust caseless archive lacks CaseFolding.txt")
        return extracted.read()


def main() -> None:
    swift = Path(__file__).resolve().parents[1]
    root = swift.parent
    snapshot = swift / "scripts/unicode/CaseFolding-16.0.0.txt"
    generated = swift / "Sources/Flyrail/UnicodeCaseFolding.generated.swift"
    source = snapshot.read_text(encoding="utf-8")
    version, entries = mappings(source)
    if unicodedata.unidata_version != version:
        raise ValueError(
            f"Python Unicode {unicodedata.unidata_version} does not match case folding {version}"
        )
    if python_folding() != expected_folding(entries):
        raise ValueError("Python full case folding differs from the authoritative snapshot")
    if generated.read_text(encoding="utf-8") != render(version, entries):
        raise ValueError("UnicodeCaseFolding.generated.swift is stale")
    lock = tomllib.loads((root / "rust/Cargo.lock").read_text(encoding="utf-8"))
    caseless = [package for package in lock["package"] if package["name"] == "caseless"]
    if [package["version"] for package in caseless] != ["0.2.2"]:
        raise ValueError("Rust must pin caseless 0.2.2 for Unicode 16.0.0 parity")
    rust = rust_snapshot(root)
    if rust is not None and rust != snapshot.read_bytes():
        raise ValueError("Rust caseless CaseFolding.txt differs from the Swift snapshot")
    print(f"Unicode {version} full case folding matches Python and Rust")


if __name__ == "__main__":
    main()
