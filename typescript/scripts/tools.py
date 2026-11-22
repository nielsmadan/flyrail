"""Download pinned TypeScript development tools into the repository cache, verifying checksums."""

import hashlib
import platform
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOOLS = ROOT / ".cache/tools"

BUN_VERSION = "1.4.2"
DOWNLOAD_DEADLINE_SECONDS = 600
BUN_SHA256 = {
    ("darwin", "aarch64"): "90987a3a16d7db556d886ac3d551e7b6d3edf0a1cf43acaed622e8676be1d12f",
    ("darwin", "x64"): "80520d7e17526308c9185d261679ac6d27798d3803a0e9f7ff9121ab8affb012",
    ("linux", "aarch64"): "54328bbc2d9c8e0c9f892c544d66c57a83b84139e34909e5ee81758f1ac8fda7",
    ("linux", "x64"): "36368faef7527875d5ffa52e53cd48021741f2a83eb6208a8dd64068d422a913",
}


def host() -> tuple[str, str]:
    system = platform.system().lower()
    machine = {"x86_64": "x64", "amd64": "x64", "arm64": "aarch64", "aarch64": "aarch64"}.get(
        platform.machine().lower(), platform.machine().lower()
    )
    return system, machine


def installed_version(executable: Path) -> str | None:
    if not executable.is_file():
        return None
    try:
        result = subprocess.run(  # noqa: S603
            [str(executable), "--version"],
            capture_output=True,
            text=True,
            check=False,
            timeout=30,
        )
    except subprocess.TimeoutExpired:
        return None
    return result.stdout.strip() if result.returncode == 0 else None


def download(url: str, destination: Path, expected: str) -> None:
    digest = hashlib.sha256()
    deadline = time.monotonic() + DOWNLOAD_DEADLINE_SECONDS
    with urllib.request.urlopen(url, timeout=60) as response, destination.open("wb") as output:  # noqa: S310
        while chunk := response.read(1 << 16):
            if time.monotonic() > deadline:
                raise RuntimeError(f"download of {url} exceeded {DOWNLOAD_DEADLINE_SECONDS} s")
            digest.update(chunk)
            output.write(chunk)
    if digest.hexdigest() != expected:
        raise RuntimeError(f"checksum mismatch for {url}: {digest.hexdigest()} != {expected}")


def bun() -> Path:
    directory = TOOLS / f"bun-{BUN_VERSION}"
    executable = directory / "bun"
    if installed_version(executable) == BUN_VERSION:
        return executable
    key = host()
    if key not in BUN_SHA256:
        raise RuntimeError(f"bun {BUN_VERSION} has no pinned archive for {key}")
    name = f"bun-{key[0]}-{key[1]}"
    url = f"https://github.com/oven-sh/bun/releases/download/bun-v{BUN_VERSION}/{name}.zip"
    directory.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=directory) as scratch:
        archive = Path(scratch) / f"{name}.zip"
        download(url, archive, BUN_SHA256[key])
        staged = Path(scratch) / "bun"
        with (
            zipfile.ZipFile(archive) as bundle,
            bundle.open(f"{name}/bun") as source,
            staged.open("wb") as output,
        ):
            shutil.copyfileobj(source, output)
        staged.chmod(0o755)
        staged.replace(executable)
    version = installed_version(executable)
    if version != BUN_VERSION:
        raise RuntimeError(f"expected bun {BUN_VERSION}, found {version}")
    return executable


TOOLS_BY_NAME = {"bun": bun}


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
