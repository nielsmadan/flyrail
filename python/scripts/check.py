import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = ROOT.parent
CACHE = REPOSITORY / ".cache"


def uv_executable() -> str | None:
    local = CACHE / "tools/uv" / ("uv.exe" if os.name == "nt" else "uv")
    executable = shutil.which(os.environ.get("FLYRAIL_UV", str(local) if local.is_file() else "uv"))
    return str(Path(executable).resolve()) if executable is not None else None


def tool_environment() -> dict[str, str]:
    environment = os.environ.copy()
    directories = {
        "UV_CACHE_DIR": CACHE / "uv",
        "UV_PYTHON_INSTALL_DIR": CACHE / "python",
        "UV_TOOL_DIR": CACHE / "uv-tools",
        "UV_TOOL_BIN_DIR": CACHE / "uv-bin",
        "XDG_CACHE_HOME": CACHE,
        "PYTHONPYCACHEPREFIX": CACHE / "pycache",
        "TMPDIR": CACHE / "tmp",
        "TMP": CACHE / "tmp",
        "TEMP": CACHE / "tmp",
    }
    for name, directory in directories.items():
        directory.mkdir(parents=True, exist_ok=True)
        environment[name] = str(directory)
    environment["UV_PROJECT_ENVIRONMENT"] = str(ROOT / ".venv")
    environment["UV_PYTHON"] = environment.get("UV_PYTHON", "3.11")
    environment["COVERAGE_FILE"] = str(ROOT / ".cache/coverage/.coverage")
    return environment


def main() -> int:
    parser = argparse.ArgumentParser(description="Run Flyrail's shared development checks.")
    parser.add_argument(
        "action",
        choices=(
            "check",
            "format",
            "sync",
            "lock",
            "build",
            "package-check",
            "dependency-audit",
            "workflow-check",
        ),
        default="check",
        nargs="?",
    )
    arguments = parser.parse_args()
    environment = tool_environment()
    if arguments.action == "workflow-check":
        return subprocess.run(
            [sys.executable, "scripts/workflow_check.py"], cwd=ROOT, env=environment, check=False
        ).returncode
    uv = uv_executable()
    if uv is None:
        print("uv >=0.12.13 is required; see docs/development.md for setup.", file=sys.stderr)
        return 1
    commands = [[uv, "sync", "--locked"]]
    if arguments.action == "lock":
        commands = [[uv, "lock"]]
    elif arguments.action == "format":
        commands += [[uv, "run", "--no-sync", "ruff", "format", "."]]
    elif arguments.action == "check":
        commands += [
            [uv, "run", "--no-sync", "ruff", "format", "--check", "."],
            [uv, "run", "--no-sync", "ruff", "check", "."],
            [uv, "run", "--no-sync", "mypy"],
            [uv, "run", "--no-sync", "pytest"],
        ]
    elif arguments.action == "dependency-audit":
        commands = [
            [uv, "sync", "--locked", "--all-groups"],
            [uv, "run", "--no-sync", "python", "scripts/dependency_audit.py"],
        ]
    elif arguments.action == "build":
        python = ROOT / ".venv" / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
        commands += [[uv, "build", "--no-build-isolation", "--python", str(python)]]
    if arguments.action in {"check", "package-check"}:
        commands += [[uv, "run", "--no-sync", "python", "scripts/package_check.py"]]
    for command in commands:
        print(f"+ {' '.join(command)}", flush=True)
        result = subprocess.run(command, cwd=ROOT, env=environment, check=False)  # noqa: S603
        if result.returncode:
            return result.returncode
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
