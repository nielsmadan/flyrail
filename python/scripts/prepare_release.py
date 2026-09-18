import re
import sys
from pathlib import Path


def replacement(path: Path, pattern: str, value: str) -> str:
    content, count = re.subn(pattern, value, path.read_text(), flags=re.MULTILINE)
    if count != 1:
        raise SystemExit(f"Expected one version reference in {path}.")
    return content


version = sys.argv[1]
changes = [
    (
        Path("python/pyproject.toml"),
        r'^version = "[^"]+"$',
        f'version = "{version}"',
    )
]
for name in ("filesystem", "packaged"):
    changes.append(
        (
            Path("python/examples") / name / "pyproject.toml",
            r'pyflyrail==[^"]+',
            f"pyflyrail=={version}",
        )
    )
prepared = [(path, replacement(path, pattern, value)) for path, pattern, value in changes]
for path, content in prepared:
    path.write_text(content)
