import re
import sys
from pathlib import Path


def replacement(path: Path, pattern: str, value: str) -> str:
    content, count = re.subn(pattern, value, path.read_text(), flags=re.MULTILINE)
    if count != 1:
        raise SystemExit(f"Expected one version reference in {path}.")
    return content


if len(sys.argv) != 2 or re.fullmatch(r"(0|[1-9][0-9]*)(\.(0|[1-9][0-9]*)){2}", sys.argv[1]) is None:
    raise SystemExit("usage: prepare_release.py MAJOR.MINOR.PATCH")
version = sys.argv[1]
changes = [
    (
        Path("rust/Cargo.toml"),
        r'(^\[workspace\.package\]\n(?:[^\[\n].*\n)*?)version = "[^"]+"$',
        rf'\g<1>version = "{version}"',
    ),
    *(
        (
            Path("rust/examples") / name / "Cargo.toml",
            r'^flyrail = \{ version = "[^"]+"',
            f'flyrail = {{ version = "{version}"',
        )
        for name in ("filesystem", "packaged")
    ),
]
prepared = [(path, replacement(path, pattern, value)) for path, pattern, value in changes]
for path, content in prepared:
    path.write_text(content)
