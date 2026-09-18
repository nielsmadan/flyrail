import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest
from conformance import FIXTURES

from flyrail import (
    AssetRef,
    Bundle,
    BundleIdentity,
    Command,
    EnvRef,
    HookArtifact,
    HookEvent,
    HookOutcome,
    HookRuntime,
    HookShell,
    InstallationTarget,
    OperationStatus,
    Platform,
    RenderContext,
    RenderedBundle,
    Target,
    TreeContent,
    inspect_installation,
    remove,
    render,
    sync,
)

ROOT = Path(__file__).resolve().parents[2]
HOST = Path(__file__).with_name("hook_host.mjs")
NODE = shutil.which("node")
BUN = shutil.which("bun")
VECTORS = json.loads((FIXTURES / "hooks.json").read_text(encoding="utf-8"))


def platform() -> Platform:
    return (
        Platform.WINDOWS
        if os.name == "nt"
        else Platform.MACOS
        if sys.platform == "darwin"
        else Platform.LINUX
    )


def node() -> str:
    assert NODE is not None, "Node 24+ is required by the shared hook runtime gate"
    return NODE


def context(root: Path, agent: str, *, scope: str = "project") -> RenderContext:
    target = (
        Target.project(agent, root) if scope == "project" else Target.user(agent, home=root, env={})
    )
    return RenderContext(
        target,
        platform(),
        hook_runtime=HookRuntime(
            Path(node()), HookShell.CMD if os.name == "nt" else HookShell.POSIX
        ),
    )


def make_hook(
    identifier: str = "handler",
    event: str = "before-tool",
    output: object = None,
    *,
    script: str | None = None,
    tools: tuple[str, ...] = (),
    timeout: int = 1000,
    env: dict[str, str | EnvRef] | None = None,
    cwd: str | AssetRef | None = None,
    outcomes: tuple[HookOutcome, ...] | None = None,
) -> HookArtifact:
    response = output if output is not None else {"version": 1, "outcome": "continue"}
    source = (
        script
        if script is not None
        else 'process.stdin.resume();process.stdin.on("end",()=>process.stdout.write('
        + json.dumps(json.dumps(response))
        + "))"
    )
    selected = (
        outcomes or (HookOutcome(response["outcome"]),)
        if isinstance(response, dict)
        else (HookOutcome.CONTINUE,)
    )
    return HookArtifact(
        identifier,
        HookEvent(event),
        Command([node(), "-e", source], env=env or {}, cwd=cwd),
        timeout_ms=timeout,
        outcomes=selected,
        tools=tools,
    )


def bundle_of(*hooks: HookArtifact, version: str = "same") -> Bundle:
    return Bundle.from_artifacts(BundleIdentity("hook-demo", version), hooks)


def install(
    root: Path, agent: str, hooks: tuple[HookArtifact, ...], *, scope: str = "project"
) -> tuple[Bundle, RenderContext, RenderedBundle, InstallationTarget]:
    bundle = bundle_of(*hooks)
    ctx = context(root, agent, scope=scope)
    rendered = render(bundle, ctx)
    assert rendered.supported, rendered.notices
    target = ctx.installation(root / "index")
    result = sync(bundle, rendered, target)
    assert result.status is OperationStatus.APPLIED, result
    return bundle, ctx, rendered, target


def invoke(
    root: Path,
    rendered: RenderedBundle,
    host: str,
    event: str,
    raw: dict[str, Any],
    output: dict[str, Any] | None = None,
    *,
    mode: str = "print",
) -> tuple[dict[str, Any], str]:
    selected = next(item for item in rendered.artifacts if item.id == "handler")
    environment = os.environ.copy()
    environment.setdefault("FLYRAIL_HOOK_TOOLS", str(ROOT / ".cache/hook-tooling"))
    command: list[str] | str
    if host in {"pi", "opencode"}:
        binary = node() if host == "pi" else BUN
        assert binary, "Bun is required by the shared runtime gate"
        request = {
            "host": host,
            "entry": str(selected.destination),
            "event": event,
            "input": raw,
            "output": output or {},
            "context": {"cwd": str(root), "mode": mode, "hasUI": mode in {"tui", "rpc"}},
        }
        command = [binary, str(HOST)]
    else:
        document = json.loads(selected.destination.read_text())
        native = document["hooks"][event][0]
        if host in {"claude", "codex"}:
            native = native["hooks"][0]
        if host == "copilot":
            command = [native["exec"], *native["args"]]
        elif host == "claude":
            command = [native["command"], *native["args"]]
        elif os.name == "nt":
            command = (
                subprocess.list2cmdline([os.environ.get("COMSPEC", "cmd.exe"), "/C"])
                + ' "'
                + native["command"]
                + '"'
            )
        else:
            login = host == "codex" and os.environ.get("FLYRAIL_TEST_CODEX_LOGIN_SHELL") == "1"
            command = ["/bin/sh", "-lc" if login else "-c", native["command"]]
        request = raw
    result = subprocess.run(  # noqa: S603
        command,
        input=json.dumps(request),
        text=True,
        encoding="utf-8",
        capture_output=True,
        cwd=root,
        env=environment,
        timeout=15,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout), result.stderr


@pytest.mark.parametrize("case", VECTORS["cases"], ids=lambda value: value["name"])
def test_neutral_host_protocol_executes_installed_bridge(
    tmp_path: Path, case: dict[str, Any]
) -> None:
    if os.name == "nt" and case["host"] == "cursor":
        pytest.skip("Cursor Windows shell is not verified")
    capture = tmp_path / "observed.json"
    source = (
        'const fs=require("node:fs");let s="";process.stdin.on("data",c=>s+=c);'
        'process.stdin.on("end",()=>{fs.writeFileSync('
        + json.dumps(str(capture))
        + ",s);process.stdout.write("
        + json.dumps(json.dumps(case["response"]))
        + ")})"
    )
    hook = make_hook(
        event=case["event"],
        output=case["response"],
        script=source,
        tools=tuple(case.get("tools", [])),
    )
    bundle, _, rendered, target = install(tmp_path, case["host"], (hook,))
    observed, stderr = invoke(
        tmp_path,
        rendered,
        case["host"],
        case["native_event"],
        case["native_input"],
        case.get("native_output"),
    )
    assert stderr == ""
    if case["host"] in {"pi", "opencode"}:
        for key, value in case["expected"].items():
            assert observed.get(key) == value
    else:
        assert observed == case["expected"]
    packet = json.loads(capture.read_text())
    assert packet["version"] == 1 and packet["event"] == case["event"]
    for key, value in case.get("canonical", {}).items():
        assert packet.get(key) == value
    assert packet["context"] == case.get("context", {}) | (
        {"cwd": str(tmp_path), "execution_mode": "print", "has_ui": False}
        if case["host"] == "pi"
        else {"cwd": str(tmp_path)}
        if case["host"] == "opencode"
        else {}
    )
    assert remove(bundle.id, target).status is OperationStatus.APPLIED
    assert inspect_installation(bundle.id, target).resources == ()


@pytest.mark.parametrize("agent", ["claude", "codex", "cursor", "copilot", "pi", "opencode"])
@pytest.mark.parametrize("scope", ["project", "user"])
def test_hook_destinations_and_source_free_upgrade(tmp_path: Path, agent: str, scope: str) -> None:
    if os.name == "nt" and agent == "cursor":
        pytest.skip("Cursor Windows shell is not verified")
    root = tmp_path / "space ü quote" if os.name == "nt" else tmp_path / "space ü quote' $;&"
    root.mkdir()
    first, ctx, rendered, target = install(root, agent, (make_hook(),), scope=scope)
    second = bundle_of(make_hook(output={"version": 1, "outcome": "block", "reason": "second"}))
    updated = render(second, ctx)
    original_entry = next(item.destination for item in rendered.artifacts if item.id == "handler")
    assert (
        next(item.destination for item in updated.artifacts if item.id == "handler")
        == original_entry
    )
    assert sync(second, updated, target).status is OperationStatus.APPLIED
    event = {
        "claude": "PreToolUse",
        "codex": "PreToolUse",
        "cursor": "preToolUse",
        "copilot": "preToolUse",
        "pi": "tool_call",
        "opencode": "tool.execute.before",
    }[agent]
    raw = (
        {"tool_name": "Bash", "tool_input": {}}
        if agent in {"claude", "codex", "cursor"}
        else {"toolName": "bash", "toolArgs": {}}
        if agent == "copilot"
        else {"toolName": "bash", "input": {}}
        if agent == "pi"
        else {"tool": "bash", "sessionID": "s", "callID": "c"}
    )
    result, stderr = invoke(root, updated, agent, event, raw, {"args": {}})
    assert stderr == ""
    assert "second" in json.dumps(result)
    old_assets = {
        item.destination for item in rendered.artifacts if isinstance(item.content, TreeContent)
    }
    assert all(not path.exists() for path in old_assets)
    assert remove(first.id, target).status is OperationStatus.APPLIED
    assert not original_entry.exists()


@pytest.mark.parametrize("host", ["pi", "opencode"])
def test_real_extension_loader_works_in_commonjs_project(tmp_path: Path, host: str) -> None:
    (tmp_path / "package.json").write_text('{"type":"commonjs"}')
    response = {"version": 1, "outcome": "block", "reason": "CommonJS handler executed"}
    _, _, rendered, _ = install(tmp_path, host, (make_hook(output=response),))
    raw = (
        {"toolName": "bash", "input": {}}
        if host == "pi"
        else {"tool": "bash", "sessionID": "s", "callID": "c"}
    )
    result, stderr = invoke(
        tmp_path,
        rendered,
        host,
        "tool_call" if host == "pi" else "tool.execute.before",
        raw,
        {"args": {}},
    )
    assert stderr == ""
    assert result["registered"] == ["tool_call" if host == "pi" else "tool.execute.before"]
    if host == "pi":
        assert result["result"] == {"block": True, "reason": response["reason"]}
    else:
        assert result["blocked"] == response["reason"]


@pytest.mark.parametrize("host", ["pi", "opencode"])
def test_native_callback_values_are_validated_before_child_execution(
    tmp_path: Path, host: str
) -> None:
    capture = tmp_path / "packet.json"
    source = (
        'const fs=require("node:fs");let s="";process.stdin.on("data",c=>s+=c);'
        'process.stdin.on("end",()=>{fs.writeFileSync('
        + json.dumps(str(capture))
        + ",s);process.stdout.write(JSON.stringify("
        '{version:1,outcome:"context",text:"native context"}))})'
    )
    _, _, output, _ = install(
        tmp_path,
        host,
        (
            make_hook(
                event="after-tool",
                script=source,
                outcomes=(HookOutcome.CONTEXT,),
            ),
        ),
    )
    entry = next(item.destination for item in output.artifacts if item.id == "handler")
    binary = node() if host == "pi" else BUN
    assert binary is not None
    environment = os.environ.copy()
    environment.setdefault("FLYRAIL_HOOK_TOOLS", str(ROOT / ".cache/hook-tooling"))
    result = subprocess.run(  # noqa: S603
        [binary, str(HOST.with_name("hook_native_host.mjs"))],
        input=json.dumps(
            {"host": host, "entry": str(entry), "cwd": str(tmp_path), "capture": str(capture)}
        ),
        text=True,
        encoding="utf-8",
        capture_output=True,
        cwd=tmp_path,
        env=environment,
        timeout=15,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == [
        {"name": name, "executed": name in {"finite", "omitted"}}
        for name in [
            "nan",
            "infinity",
            "negative-infinity",
            "undefined",
            "function",
            "bigint",
            "symbol",
            "date",
            "cycle",
            "finite",
            "omitted",
        ]
    ]
    assert result.stderr.splitlines() == [
        *("flyrail-hook:json-number" for _ in range(3)),
        *("flyrail-hook:invalid-input" for _ in range(5)),
        "flyrail-hook:json-depth",
    ]
