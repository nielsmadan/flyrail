import subprocess
import sys
import tomllib

from check import ROOT, tool_environment


def main() -> int:
    environment = tool_environment()
    lock = tomllib.loads((ROOT / "uv.lock").read_text(encoding="utf-8"))
    requirements = sorted(
        {
            f"{package['name']}=={package['version']}"
            for package in lock["package"]
            if "registry" in package["source"]
        }
    )
    if not requirements:
        raise RuntimeError("No registry dependencies found in uv.lock")
    directory = ROOT / ".cache/audit"
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / "requirements.txt"
    path.write_text("\n".join(requirements) + "\n", encoding="utf-8")
    print(f"Auditing all {len(requirements)} locked registry package versions: {path}", flush=True)
    return subprocess.run(  # noqa: S603
        [
            sys.executable,
            "-m",
            "pip_audit",
            "--requirement",
            str(path),
            "--no-deps",
            "--disable-pip",
            "--cache-dir",
            str(directory / "http"),
            "--progress-spinner",
            "off",
            "--strict",
        ],
        cwd=ROOT,
        env=environment,
        check=False,
    ).returncode


if __name__ == "__main__":
    raise SystemExit(main())
