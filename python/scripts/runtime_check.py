import os
import shutil
import subprocess
import sys
from pathlib import Path

from check import CACHE, ROOT, tool_environment


def main() -> int:
    environment = tool_environment()
    node, npm, bun = (shutil.which(name) for name in ("node", "npm", "bun"))
    if node is None or npm is None or bun is None:
        print(
            "Node 24+, npm and Bun 1.4.2+ are required for hook development checks.",
            file=sys.stderr,
        )
        return 1
    npm_command = [npm]
    if os.name == "nt":
        npm_command = [node, str(Path(npm).resolve().parent / "node_modules/npm/bin/npm-cli.js")]
    tooling = CACHE / "hook-tooling"
    tooling.mkdir(parents=True, exist_ok=True)
    for name in ("package.json", "package-lock.json"):
        shutil.copyfile(ROOT / "runtime-tooling" / name, tooling / name)
    commands = [
        [node, "--version"],
        [bun, "--version"],
        [
            *npm_command,
            "ci",
            "--prefix",
            str(tooling),
            "--cache",
            str(CACHE / "npm"),
            "--ignore-scripts",
        ],
        [
            *npm_command,
            "audit",
            "--prefix",
            str(tooling),
            "--cache",
            str(CACHE / "npm"),
            "--audit-level=low",
        ],
        [
            node,
            str(tooling / "node_modules/typescript/bin/tsc"),
            "-p",
            str(ROOT / "runtime-tooling/tsconfig.json"),
            "--typeRoots",
            str(tooling / "node_modules/@types"),
        ],
        [node, "--check", str(ROOT / "tests/hook_host.mjs")],
        [node, "--check", str(ROOT / "tests/hook_runner.mjs")],
    ]
    commands.extend(
        [node, "--check", str(path)]
        for path in sorted((ROOT / "src/flyrail/runtime").glob("*.mts"))
    )
    for command in commands:
        print("+ " + " ".join(command), flush=True)
        result = subprocess.run(command, cwd=ROOT, env=environment, check=False)  # noqa: S603
        if result.returncode:
            return result.returncode
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
