"""Download pinned Go development tools into the repository cache with checksum verification."""

import hashlib
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOOLS = ROOT / ".cache/tools"

GOLANGCI_LINT_VERSION = "2.15.0"
GOLANGCI_LINT_SHA256 = {
    ("darwin", "amd64"): "767b899a28285e88ded2823d5dcc4bc5ef681173eb2d80f029a7b17ee4ece3b8",
    ("darwin", "arm64"): "d1e409481a8ca846c4bc97e781eb5cb1bd5c23353a0b3977a1cbda1b1b6447d5",
    ("linux", "amd64"): "9b7fe6a6bf6334b98decf8d86b11415ea7889afdabdd1959fdce23af2a9ebcea",
    ("linux", "arm64"): "b4d8f17068b35ee6060cc2f69a16fb48740d706fbba81661c318649c114a6998",
}


def host() -> tuple[str, str]:
    system = platform.system().lower()
    machine = {"x86_64": "amd64", "amd64": "amd64", "arm64": "arm64", "aarch64": "arm64"}.get(
        platform.machine().lower(), platform.machine().lower()
    )
    return system, machine


def installed_version(executable: Path) -> str | None:
    if not executable.is_file():
        return None
    result = subprocess.run(  # noqa: S603
        [str(executable), "version", "--short"], capture_output=True, text=True, check=False
    )
    return result.stdout.strip() if result.returncode == 0 else None


def download(url: str, destination: Path, expected: str) -> None:
    digest = hashlib.sha256()
    with urllib.request.urlopen(url, timeout=60) as response, destination.open("wb") as output:  # noqa: S310
        while chunk := response.read(1 << 16):
            digest.update(chunk)
            output.write(chunk)
    if digest.hexdigest() != expected:
        raise RuntimeError(f"checksum mismatch for {url}: {digest.hexdigest()} != {expected}")


def golangci_lint() -> Path:
    directory = TOOLS / "golangci-lint"
    executable = directory / "golangci-lint"
    if installed_version(executable) == GOLANGCI_LINT_VERSION:
        return executable
    key = host()
    if key not in GOLANGCI_LINT_SHA256:
        raise RuntimeError(f"golangci-lint {GOLANGCI_LINT_VERSION} has no pinned archive for {key}")
    name = f"golangci-lint-{GOLANGCI_LINT_VERSION}-{key[0]}-{key[1]}"
    url = (
        "https://github.com/golangci/golangci-lint/releases/download/"
        f"v{GOLANGCI_LINT_VERSION}/{name}.tar.gz"
    )
    directory.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=directory) as scratch:
        archive = Path(scratch) / f"{name}.tar.gz"
        download(url, archive, GOLANGCI_LINT_SHA256[key])
        with tarfile.open(archive) as bundle:
            member = bundle.getmember(f"{name}/golangci-lint")
            source = bundle.extractfile(member)
            if source is None:
                raise RuntimeError(f"{name}.tar.gz has no golangci-lint executable")
            staged = Path(scratch) / "golangci-lint"
            with source, staged.open("wb") as output:
                shutil.copyfileobj(source, output)
        staged.chmod(0o755)
        staged.replace(executable)
    version = installed_version(executable)
    if version != GOLANGCI_LINT_VERSION:
        raise RuntimeError(f"expected golangci-lint {GOLANGCI_LINT_VERSION}, found {version}")
    return executable


TOOLS_BY_NAME = {"golangci-lint": golangci_lint}


def main(arguments: list[str]) -> int:
    names = arguments or sorted(TOOLS_BY_NAME)
    unknown = [name for name in names if name not in TOOLS_BY_NAME]
    if unknown:
        print(f"unknown tool: {', '.join(unknown)}", file=sys.stderr)
        return 2
    for name in names:
        print(TOOLS_BY_NAME[name]())
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
