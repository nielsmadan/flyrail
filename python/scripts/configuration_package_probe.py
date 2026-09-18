import importlib.resources
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any


def command(arguments: list[str], expected: int = 0) -> str:
    result = subprocess.run(  # noqa: S603
        arguments,
        capture_output=True,
        text=True,
        check=False,
        timeout=90,
    )
    assert result.returncode == expected, (
        arguments,
        result.returncode,
        result.stdout,
        result.stderr,
    )
    assert result.stderr == "", result.stderr
    return result.stdout


def snapshot(root: Path) -> dict[str, bytes]:
    return {
        str(path.relative_to(root)): path.read_bytes() for path in root.rglob("*") if path.is_file()
    }


def main() -> None:
    executable = Path(sys.executable).with_name(
        "flyrail-checksum.exe" if os.name == "nt" else "flyrail-checksum"
    )
    node = shutil.which("node")
    assert node is not None
    root = Path.cwd() / "combined ü project"
    root.mkdir()
    guidance = root / "CLAUDE.md"
    original = b"# Project instructions\nPreserve this user guidance.\n"
    guidance.write_bytes(original)
    arguments = ["--project", str(root), "--agent", "claude"]
    content_arguments = [
        "--node",
        node,
        "--mcp-command",
        "checksum-mcp",
        "--mcp-url",
        "https://checksum.example.invalid/mcp",
    ]

    def call(action: str, *extra: str, expected: int = 0) -> dict[str, Any]:
        report: dict[str, Any] = json.loads(
            command(
                [
                    str(executable),
                    "config",
                    action,
                    *arguments,
                    *(content_arguments if action != "uninstall" else []),
                    *extra,
                ],
                expected,
            )
        )
        assert report["app_version"] == "2.1.0"
        assert report["bundle_id"] == "example-checksum-config"
        assert report["activation"] == "not-checked"
        return report

    before = snapshot(root)
    assert call("status", expected=1)["configured_current"] is False
    assert snapshot(root) == before
    installed = call("update")
    assert installed["status"] == "applied" and installed["configured_current"] is True
    assert installed["prerequisites"]
    assert any(notice["kind"] == "activation" for notice in installed["notices"])
    assert (root / ".claude/skills/checksum-review/SKILL.md").is_file()
    assert guidance.read_bytes().startswith(original)
    mcp = json.loads((root / ".mcp.json").read_text())["mcpServers"]
    assert mcp["checksum-local"]["command"] == "checksum-mcp"
    assert mcp["checksum-local"]["env"] == {"CHECKSUM_MCP_TOKEN": "${CHECKSUM_MCP_TOKEN}"}
    assert mcp["checksum-remote"]["headers"] == {"Authorization": "Bearer ${CHECKSUM_HTTP_TOKEN}"}
    hook = json.loads((root / ".claude/settings.json").read_text())["hooks"]["PreToolUse"][0][
        "hooks"
    ][0]
    result = subprocess.run(  # noqa: S603
        [hook["command"], *hook["args"]],
        input='{"tool_name":"Bash","tool_input":{}}',
        capture_output=True,
        text=True,
        check=True,
        cwd=root,
        timeout=10,
    )
    assert result.stderr == ""
    assert json.loads(result.stdout) == {
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "additionalContext": (
                "Verify a downloaded file with flyrail-checksum digest FILE before using it."
            ),
        }
    }
    before = snapshot(root)
    assert call("status")["configured_current"] is True
    assert snapshot(root) == before
    mtimes = {path: path.stat().st_mtime_ns for path in (guidance, root / ".mcp.json")}
    assert call("update")["status"] == "unchanged"
    assert {path: path.stat().st_mtime_ns for path in mtimes} == mtimes

    changed = ["--guidance", "Generated guidance from the next app configuration."]
    report = call("status", *changed, expected=1)
    assert report["recorded_current"] is True and report["configured_current"] is False
    assert report["bundle_version"] == installed["bundle_version"]
    assert call("update", *changed)["configured_current"] is True
    moved = [*changed, "--asset-root", str(root / "relocated-assets")]
    report = call("status", *moved, expected=1)
    assert report["recorded_current"] is True and report["configured_current"] is False
    assert call("update", *moved)["configured_current"] is True
    guidance.write_bytes(guidance.read_bytes().replace(b"Generated guidance", b"User edit"))
    assert call("status", *moved, expected=1)["recorded_current"] is False
    report = call("update", *moved, expected=1)
    assert report["status"] == "failed" and report["error"]["code"] == "modified"
    assert call("update", *moved, "--replace-modified")["configured_current"] is True

    blocked = Path.cwd() / "unsupported combined"
    report = call("update", "--project", str(blocked), "--agent", "copilot", expected=1)
    assert report["configured_current"] is False and report["error"]["code"] == "unsupported"
    assert snapshot(blocked) == {}
    assert any(notice["kind"] == "unsupported" for notice in report["notices"])

    package = Path(str(importlib.resources.files("flyrail_example_checksum")))
    manifest_path = package / "flyrail/flyrail.json"
    manifest_bytes = manifest_path.read_bytes()
    manifest = json.loads(manifest_bytes)
    manifest["version"] = "a-new-bundle-label"
    manifest_path.write_text(json.dumps(manifest))
    report = call("status", *moved, expected=1)
    assert report["recorded_current"] is True and report["configured_current"] is False
    assert report["bundle_version"] == "a-new-bundle-label"
    manifest_path.write_bytes(manifest_bytes)
    for name in ("flyrail", "assets"):
        (package / name).rename(package / f"hidden-{name}")
    try:
        assert call("uninstall")["status"] == "applied"
        assert call("uninstall")["status"] == "unchanged"
    finally:
        for name in ("flyrail", "assets"):
            (package / f"hidden-{name}").rename(package / name)
    assert guidance.read_bytes() == original
    print(
        "Installed combined CLI: four families, hook execution/assets, current/activation "
        "distinction, generation/render drift, edit protection, unsupported preflight "
        "and source-free removal passed."
    )


if __name__ == "__main__":
    main()
