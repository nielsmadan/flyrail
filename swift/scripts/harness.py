import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SWIFT = [str(ROOT / ".cache/swift/debug/FlyrailInterop")]
PYTHON = [sys.executable, "-B", str(ROOT / "python/scripts/interop_driver.py")]
RUST = [str(ROOT / "rust/target/debug/examples/interop")]
ENV = os.environ | {"PYTHONPATH": str(ROOT / "python/src"), "PYTHONDONTWRITEBYTECODE": "1"}


def use_python_paths(*directories):
    for directory in reversed(directories):
        path = str(ROOT / directory)
        if path not in sys.path:
            sys.path.insert(0, path)


def run(driver, *arguments, input=None, timeout=60):
    result = subprocess.run(
        driver + list(map(str, arguments)),
        input=input,
        capture_output=True,
        env=ENV,
        cwd=ROOT,
        timeout=timeout,
    )
    assert result.returncode == 0, (result.returncode, result.stdout, result.stderr)
    return result.stdout


def driver_error(driver, result):
    if driver == SWIFT:
        return result.stderr.decode().strip()
    return json.loads(result.stdout)["error"]["message"]


def build_rust_examples(*names):
    subprocess.run(
        ["cargo", "build", "--locked", "--manifest-path", str(ROOT / "rust/Cargo.toml"),
         *[argument for name in names for argument in ["--example", name]],
         "--features", "interop-test-seams"],
        env=ENV | {"CARGO_HOME": str(ROOT / ".cache/cargo-home")}, cwd=ROOT, check=True, timeout=240,
    )


def tree_snapshot(root, modes=False):
    return {
        str(path): (path.stat().st_mode, path.read_bytes()) if modes else path.read_bytes()
        for path in Path(root).rglob("*") if path.is_file()
    }
