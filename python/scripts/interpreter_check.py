import os
import subprocess
import sys
from pathlib import Path

from check import CACHE, ROOT, uv_executable


def main() -> int:
    requested = os.environ["UV_PYTHON"]
    print(f"Python requested={requested} selected={sys.version} executable={sys.executable}")
    version = sys.version_info[:3]
    if os.name == "nt" and (version < (3, 11, 10) or (3, 12, 0) <= version < (3, 12, 4)):
        print("Windows checks require Python 3.11.10+, 3.12.4+ or 3.13+.", file=sys.stderr)
        return 1
    if os.environ.get("UV_MANAGED_PYTHON") == "1":
        uv = uv_executable()
        if uv is None:
            return 1
        selected = subprocess.run(  # noqa: S603
            [uv, "python", "find", "--system", "--managed-python", requested],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=True,
        )
        managed = Path(selected.stdout.strip()).resolve()
        base = Path(sys.base_prefix).resolve()
        if not managed.is_relative_to((CACHE / "python").resolve()) or not managed.is_relative_to(
            base
        ):
            print(
                f"Unexpected managed interpreter: {managed}; running base={base}", file=sys.stderr
            )
            return 1
        print(f"Verified uv-managed Python: {managed}")
    shell_mode = os.environ.get("FLYRAIL_TEST_CODEX_LOGIN_SHELL") == "1"
    print(
        "Codex native probes: "
        + (
            "Windows cmd /C"
            if os.name == "nt"
            else "POSIX /bin/sh -lc"
            if shell_mode
            else "POSIX non-login /bin/sh -c"
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
