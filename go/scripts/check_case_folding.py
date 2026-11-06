"""Compare Go's generated full case folding with Python 3.14 str.casefold for every Unicode scalar."""

import subprocess
import sys
import tempfile
import unicodedata
from pathlib import Path

EXPECTED_UNICODE = "16.0.0"


def python_folding() -> list[str]:
    lines = []
    for code in range(sys.maxunicode + 1):
        if 0xD800 <= code <= 0xDFFF:
            continue
        folded = chr(code).casefold()
        if folded != chr(code):
            lines.append(f"{code:X}\t{' '.join(f'{ord(item):X}' for item in folded)}")
    return lines


def main() -> int:
    if unicodedata.unidata_version != EXPECTED_UNICODE:
        print(
            f"Python {sys.version.split()[0]} has Unicode {unicodedata.unidata_version}; "
            f"the case folding check needs Unicode {EXPECTED_UNICODE} (Python 3.14)",
            file=sys.stderr,
        )
        return 1
    module = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory() as directory:
        reference = Path(directory) / "casefold.tsv"
        reference.write_text("\n".join(python_folding()) + "\n", encoding="utf-8")
        result = subprocess.run(  # noqa: S603
            [
                "go",
                "test",
                "-count=1",
                "-v",
                "./internal/unicodex",
                "-run",
                "TestCaseFoldMatchesReference",
                "-args",
                f"-reference={reference}",
            ],
            cwd=module,
            check=False,
            capture_output=True,
            text=True,
        )
    sys.stdout.write(result.stdout)
    sys.stderr.write(result.stderr)
    if result.returncode != 0:
        return result.returncode
    if "--- PASS: TestCaseFoldMatchesReference" not in result.stdout:
        print("TestCaseFoldMatchesReference did not run to a pass", file=sys.stderr)
        return 1
    print(f"Unicode {EXPECTED_UNICODE} full case folding matches Python str.casefold")
    return 0


if __name__ == "__main__":
    sys.exit(main())
