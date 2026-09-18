import base64
import json
import os
import subprocess
import time
from importlib.resources import files
from pathlib import Path
from typing import Any

import pytest
from test_hooks import BUN, install, invoke, make_hook, node

from flyrail import HookOutcome


@pytest.fixture(params=["node", "bun"])
def runtime_binary(request: pytest.FixtureRequest) -> str:
    binary = node() if request.param == "node" else BUN
    assert binary is not None, "Bun is required by the shared hook runtime gate"
    return binary


def run_child(
    root: Path,
    script: str,
    *,
    clock_step: int = 0,
    runtime_binary: str | None = None,
    **changes: Any,
) -> dict[str, Any]:
    runtime = root / "runner.mts"
    for name in ["runner.mts", "process.mts"]:
        (root / name).write_bytes(files("flyrail").joinpath("runtime", name).read_bytes())
    handler: dict[str, Any] = {
        "id": "handler",
        "event": "before-tool",
        "argv": [node(), "-e", script],
        "env": {},
        "cwd": str(root),
        "timeout_ms": 200,
        "outcomes": ["continue", "block", "ask", "context", "modify-input"],
        "tools": [],
    }
    handler.update(changes)
    request = {
        "runtime": str(runtime),
        "clockStep": clock_step,
        "config": {"host": "test", "handlers": [handler]},
        "input": {"version": 1, "event": "before-tool", "tool": "Bash", "input": {}, "context": {}},
    }
    result = subprocess.run(  # noqa: S603
        [runtime_binary or node(), str(Path(__file__).with_name("hook_runner.mjs"))],
        input=json.dumps(request),
        text=True,
        capture_output=True,
        timeout=5,
        check=False,
        cwd=root,
    )
    assert result.returncode == 0, result.stderr
    assert result.stderr == ""
    value = json.loads(result.stdout)
    assert isinstance(value, dict)
    return value


@pytest.mark.parametrize(
    ("script", "code"),
    [
        ("process.exit(7)", "exit"),
        ('process.kill(process.pid,"SIGTERM")', "exit"),
        ("setInterval(()=>{},1000)", "timeout"),
        ('process.stdout.write("x".repeat(65537))', "stdout-limit"),
        ('process.stderr.write("x".repeat(16385))', "stderr-limit"),
        ("process.stdout.write(Buffer.from([255]))", "invalid-output"),
        ('process.stdout.write("{")', "invalid-output"),
        (
            'process.stdout.write(\'{"version":1,"outcome":"continue","extra":true}\')',
            "output-fields",
        ),
        ('process.stdout.write(\'{"version":1,"outcome":"block"}\')', "output-fields"),
        ('process.stdout.write(\'{"version":1,"outcome":"block","reason":""}\')', "reason"),
        ('process.stdout.write(\'{"version":1,"outcome":"context","text":""}\')', "context"),
        (
            'process.stdout.write(\'{"version":1,"outcome":"modify-input","input":[]}\')',
            "replacement",
        ),
        ('process.stdout.write(\'{"version":2,"outcome":"continue"}\')', "undeclared-outcome"),
        ('process.stdout.write(\'{"version":1,"outcome":"permit"}\')', "undeclared-outcome"),
        (
            'process.stdout.write(\'{"version":1,"version":1,"outcome":"continue"}\')',
            "invalid-output",
        ),
        (
            'process.stdout.write(\'{"version":1,"outcome":"continue"}\');process.stderr.write(Buffer.from([255]))',
            "invalid-output",
        ),
    ],
)
def test_child_failure_bounds_are_distinct_from_decisions(
    tmp_path: Path, runtime_binary: str, script: str, code: str
) -> None:
    assert run_child(tmp_path, script, runtime_binary=runtime_binary) == {
        "error": f"flyrail-hook:{code}"
    }


@pytest.mark.parametrize("field", ["argv", "cwd", "env"])
def test_missing_child_prerequisites_do_not_disclose_values(
    tmp_path: Path, runtime_binary: str, field: str
) -> None:
    changes: dict[str, Any] = (
        {"argv": [str(tmp_path / "missing-private-command")]}
        if field == "argv"
        else {"cwd": str(tmp_path / "missing-private-cwd")}
        if field == "cwd"
        else {"env": {"CHILD_SECRET": {"ref": "FLYRAIL_TEST_DEFINITELY_ABSENT"}}}
    )
    assert run_child(tmp_path, "", runtime_binary=runtime_binary, **changes) == {
        "error": "flyrail-hook:missing-env" if field == "env" else "flyrail-hook:spawn"
    }


def test_argv_env_and_cwd_are_literal_at_execution(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, runtime_binary: str
) -> None:
    value = " ü \" ' $PATH; &|<>%VAR%!NO!"
    monkeypatch.setenv("FLYRAIL_TEST_DECLARED", value)
    script = (
        'const r={version:1,outcome:"context",text:JSON.stringify({'
        "arg:process.argv[1],env:process.env.CHILD_VALUE,literal:process.env.LITERAL,"
        "cwd:process.cwd()})};process.stdout.write(JSON.stringify(r))"
    )
    result = run_child(
        tmp_path,
        script,
        runtime_binary=runtime_binary,
        argv=[node(), "-e", script, value],
        env={"CHILD_VALUE": {"ref": "FLYRAIL_TEST_DECLARED"}, "LITERAL": "${UNCHANGED}"},
    )
    assert json.loads(result["result"]["contexts"][0]) == {
        "arg": value,
        "env": value,
        "literal": "${UNCHANGED}",
        "cwd": str(tmp_path),
    }


@pytest.mark.skipif(os.name != "nt", reason="Windows environment identity")
def test_windows_environment_overrides_and_references_use_case_insensitive_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, runtime_binary: str
) -> None:
    monkeypatch.setenv("FLYRAIL_TEST_Mixed", "ambient")
    script = (
        'process.stdout.write(JSON.stringify({version:1,outcome:"context",text:JSON.stringify({'
        'names:Object.keys(process.env).filter(name=>name.toLowerCase()==="flyrail_test_mixed"),'
        "value:process.env.flyrail_test_mixed,reference:process.env.REFERENCED})}))"
    )
    result = run_child(
        tmp_path,
        script,
        runtime_binary=runtime_binary,
        env={"flyrail_test_mixed": "literal", "REFERENCED": {"ref": "flyrail_test_mixed"}},
    )
    assert json.loads(result["result"]["contexts"][0]) == {
        "names": ["flyrail_test_mixed"],
        "value": "literal",
        "reference": "ambient",
    }


@pytest.mark.parametrize(
    "raw",
    [b'{"a":1,"a":2}', b'{"__proto__":{}}', b'{"x":1e400}', b"[" * 65 + b"0" + b"]" * 65, b"\xff"],
)
def test_protocol_rejects_ambiguous_json(tmp_path: Path, raw: bytes) -> None:
    runtime = tmp_path / "runner.mts"
    for name in ["runner.mts", "process.mts"]:
        (tmp_path / name).write_bytes(files("flyrail").joinpath("runtime", name).read_bytes())
    request = {"runtime": str(runtime), "parse": base64.b64encode(raw).decode()}
    result = subprocess.run(  # noqa: S603
        [node(), str(Path(__file__).with_name("hook_runner.mjs"))],
        input=json.dumps(request),
        text=True,
        capture_output=True,
        timeout=5,
        check=True,
        cwd=tmp_path,
    )
    assert json.loads(result.stdout)["error"].startswith("flyrail-hook:")


@pytest.mark.skipif(os.name == "nt", reason="POSIX process group cleanup guarantee")
def test_runner_kills_descendant_pipe_holders_after_parent_exit(
    tmp_path: Path, runtime_binary: str
) -> None:
    marker = tmp_path / "descendant-ran"
    child = (
        'setTimeout(()=>require("node:fs").writeFileSync('
        + json.dumps(str(marker))
        + ',"ran"),500)'
    )
    script = (
        'require("node:child_process").spawn(process.execPath,["-e",'
        + json.dumps(child)
        + '],{stdio:"inherit"});process.stdout.write(\'{"version":1,"outcome":"continue"}\');'
        "process.exit(0)"
    )
    result = run_child(tmp_path, script, runtime_binary=runtime_binary)
    assert result == {"result": {"contexts": []}}
    time.sleep(0.6)
    assert list(tmp_path.glob("descendant-ran")) == []


@pytest.mark.parametrize("host", ["claude", "copilot", "pi", "opencode"])
def test_runtime_failure_does_not_become_native_permission_decision(
    tmp_path: Path, host: str
) -> None:
    _, _, rendered, _ = install(tmp_path, host, (make_hook(script="process.exit(2)"),))
    event = {
        "claude": "PreToolUse",
        "copilot": "preToolUse",
        "pi": "tool_call",
        "opencode": "tool.execute.before",
    }[host]
    raw = (
        {"tool_name": "Bash", "tool_input": {}}
        if host == "claude"
        else {"toolName": "bash", "toolArgs": {}}
        if host == "copilot"
        else {"toolName": "bash", "input": {}}
        if host == "pi"
        else {"tool": "bash", "sessionID": "s", "callID": "c"}
    )
    result, stderr = invoke(tmp_path, rendered, host, event, raw, {"args": {}})
    assert stderr == "flyrail-hook:exit\n"
    assert (
        result == {}
        if host in {"claude", "copilot"}
        else result["result"] is None
        if host == "pi"
        else result["blocked"] is None
    )


def test_exact_tool_match_and_ordered_short_circuit(tmp_path: Path) -> None:
    marker = tmp_path / "unexpected"
    verify = (
        'let s="";process.stdin.on("data",c=>s+=c);process.stdin.on("end",()=>'
        'process.stdout.write(JSON.stringify({version:1,outcome:"context",'
        "text:JSON.parse(s).input.command})))"
    )
    hooks = (
        make_hook(
            "z-final",
            script='require("node:fs").writeFileSync(' + json.dumps(str(marker)) + ',"bad")',
        ),
        make_hook(
            "handler",
            output={"version": 1, "outcome": "modify-input", "input": {"command": "changed"}},
            tools=("Bash",),
        ),
        make_hook("n-context", script=verify, outcomes=(HookOutcome.CONTEXT,), tools=("Bash",)),
        make_hook(
            "p-block", output={"version": 1, "outcome": "block", "reason": "stop"}, tools=("Bash",)
        ),
    )
    _, _, rendered, _ = install(tmp_path, "claude", hooks)
    output, error = invoke(
        tmp_path,
        rendered,
        "claude",
        "PreToolUse",
        {"tool_name": "Bash", "tool_input": {"command": "old"}},
    )
    assert error == ""
    assert output == {
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "additionalContext": "changed",
            "permissionDecision": "deny",
            "permissionDecisionReason": "stop",
        }
    }
    assert list(tmp_path.glob("unexpected")) == []
    guarded = make_hook(
        tools=("Bash",), output={"version": 1, "outcome": "block", "reason": "exact"}
    )
    other = tmp_path / "exact"
    other.mkdir()
    _, _, rendered, _ = install(other, "claude", (guarded,))
    output, error = invoke(
        other, rendered, "claude", "PreToolUse", {"tool_name": "Bashful", "tool_input": {}}
    )
    assert output == {} and error == ""


@pytest.mark.parametrize("host", ["claude", "copilot"])
def test_modified_arguments_survive_later_ask(tmp_path: Path, host: str) -> None:
    first = make_hook(
        output={"version": 1, "outcome": "modify-input", "input": {"command": "changed"}}
    )
    second = make_hook(
        "later", output={"version": 1, "outcome": "ask", "reason": "confirm changed"}
    )
    _, _, rendered, _ = install(tmp_path, host, (second, first))
    raw = (
        {"tool_name": "Bash", "tool_input": {"command": "old"}}
        if host == "claude"
        else {"toolName": "bash", "toolArgs": {"command": "old"}}
    )
    value, error = invoke(
        tmp_path, rendered, host, "PreToolUse" if host == "claude" else "preToolUse", raw
    )
    assert error == ""
    result = value["hookSpecificOutput"] if host == "claude" else value
    assert result["permissionDecision"] == "ask"
    assert result["updatedInput" if host == "claude" else "modifiedArgs"] == {"command": "changed"}


@pytest.mark.parametrize("mode", ["tui", "rpc", "json", "print"])
def test_pi_execution_mode_is_not_a_permission_mode(tmp_path: Path, mode: str) -> None:
    capture = tmp_path / "context.json"
    source = (
        'let s="";process.stdin.on("data",c=>s+=c);process.stdin.on("end",()=>{'
        'require("node:fs").writeFileSync(' + json.dumps(str(capture)) + ",s);"
        'process.stdout.write(\'{"version":1,"outcome":"continue"}\')})'
    )
    _, _, rendered, _ = install(tmp_path, "pi", (make_hook(script=source),))
    _, error = invoke(
        tmp_path, rendered, "pi", "tool_call", {"toolName": "bash", "input": {}}, mode=mode
    )
    assert error == ""
    assert json.loads(capture.read_text())["context"] == {
        "cwd": str(tmp_path),
        "execution_mode": mode,
        "has_ui": mode in ["tui", "rpc"],
    }


def test_invalid_later_handler_discards_prior_modification(tmp_path: Path) -> None:
    first = make_hook(
        output={"version": 1, "outcome": "modify-input", "input": {"command": "changed"}}
    )
    second = make_hook("later", script='process.stdout.write("invalid")')
    _, _, rendered, _ = install(tmp_path, "pi", (first, second))
    value, error = invoke(
        tmp_path,
        rendered,
        "pi",
        "tool_call",
        {"toolName": "bash", "input": {"command": "original"}},
    )
    assert error == "flyrail-hook:invalid-output\n"
    assert value["input"]["input"] == {"command": "original"}
    assert value["result"] is None


def test_event_deadline_includes_orchestration_after_child_completion(
    tmp_path: Path, runtime_binary: str
) -> None:
    assert run_child(
        tmp_path,
        'process.stdout.write(\'{"version":1,"outcome":"continue"}\')',
        clock_step=150001,
        runtime_binary=runtime_binary,
        timeout_ms=1000,
    ) == {"error": "flyrail-hook:event-timeout"}


@pytest.mark.parametrize("held_open", [False, True])
def test_native_stdin_is_bounded_before_child_execution(tmp_path: Path, held_open: bool) -> None:
    marker = tmp_path / "unexpected-child"
    script = 'require("node:fs").writeFileSync(' + json.dumps(str(marker)) + ',"ran")'
    _, _, rendered, _ = install(tmp_path, "claude", (make_hook(script=script),))
    path = next(item.destination for item in rendered.artifacts if item.id == "handler")
    command = json.loads(path.read_text())["hooks"]["PreToolUse"][0]["hooks"][0]
    with subprocess.Popen(  # noqa: S603
        [command["command"], *command["args"]],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        cwd=tmp_path,
    ) as process:
        if held_open:
            assert process.stdout is not None and process.stderr is not None
            process.wait(timeout=6)
            stdout, stderr = process.stdout.read(), process.stderr.read()
        else:
            stdout, stderr = process.communicate(b"x" * 1048577, timeout=6)
        assert process.returncode == 0
    assert json.loads(stdout) == {}
    assert stderr == (
        b"flyrail-hook:stdin-timeout\n" if held_open else b"flyrail-hook:input-limit\n"
    )
    assert list(tmp_path.glob("unexpected-child")) == []


def test_copilot_block_discards_pending_argument_replacement(tmp_path: Path) -> None:
    first = make_hook(
        output={"version": 1, "outcome": "modify-input", "input": {"command": "changed"}}
    )
    second = make_hook("later", output={"version": 1, "outcome": "block", "reason": "reject"})
    _, _, rendered, _ = install(tmp_path, "copilot", (first, second))
    value, error = invoke(
        tmp_path,
        rendered,
        "copilot",
        "preToolUse",
        {"toolName": "bash", "toolArgs": {"command": "old"}},
    )
    assert error == ""
    assert value == {"permissionDecision": "deny", "permissionDecisionReason": "reject"}
