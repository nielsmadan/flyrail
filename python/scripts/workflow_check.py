import os
import shutil
import subprocess
import sys
from pathlib import Path

from check import CACHE, REPOSITORY, tool_environment

VERSION = "1.7.12"


def main() -> int:
    local = CACHE / "tools/actionlint" / ("actionlint.exe" if os.name == "nt" else "actionlint")
    executable = shutil.which(
        os.environ.get("FLYRAIL_ACTIONLINT", str(local) if local.is_file() else "actionlint")
    )
    if executable is None:
        print(
            f"actionlint {VERSION} is required; see docs/development.md for setup.", file=sys.stderr
        )
        return 1
    executable = str(Path(executable).resolve())
    environment = tool_environment()
    version = subprocess.run(  # noqa: S603
        [executable, "-version"], env=environment, capture_output=True, text=True, check=False
    )
    print(version.stdout, end="", flush=True)
    if version.returncode or version.stdout.splitlines()[:1] != [VERSION]:
        print(f"Expected actionlint {VERSION}. {version.stderr}", file=sys.stderr)
        return 1
    workflows = sorted((REPOSITORY / ".github/workflows").glob("*.y*ml"))
    if not workflows:
        raise RuntimeError("No GitHub Actions workflows found")
    return subprocess.run(  # noqa: S603
        [executable, "-shellcheck=", "-pyflakes=", *map(str, workflows)],
        cwd=REPOSITORY,
        env=environment,
        check=False,
    ).returncode


if __name__ == "__main__":
    raise SystemExit(main())
