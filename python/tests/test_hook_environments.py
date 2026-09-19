import json
from pathlib import Path

import pytest
from test_hook_execution import run_child
from test_hooks import install, invoke, make_hook

from flyrail import EnvRef

pytestmark = pytest.mark.integration

NAMES = ["__proto__", "constructor", "toString", "FLYRAIL_TEST_ORDINARY"]


def environment_capture() -> str:
    return (
        "const names="
        + json.dumps(NAMES)
        + ";process.stdout.write(JSON.stringify({version:1,outcome:'context',"
        "text:JSON.stringify(names.map(name=>[name,Object.hasOwn(process.env,name),"
        "process.env[name]]))}))"
    )


@pytest.mark.parametrize("references", [False, True])
def test_runtime_environment_names_preserve_own_literal_and_reference_values(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, references: bool
) -> None:
    for name in NAMES:
        monkeypatch.setenv(name, "ambient-" + name)
    result = run_child(
        tmp_path,
        environment_capture(),
        env={name: {"ref": name} if references else "literal-" + name for name in NAMES},
    )
    assert json.loads(result["result"]["contexts"][0]) == [
        [name, True, ("ambient-" if references else "literal-") + name] for name in NAMES
    ]


@pytest.mark.parametrize("name", NAMES)
def test_runtime_missing_environment_reference_stops_before_child_execution(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, name: str
) -> None:
    monkeypatch.delenv(name, raising=False)
    marker = tmp_path / "executed"
    script = 'require("node:fs").writeFileSync(' + json.dumps(str(marker)) + ',"ran")'
    assert run_child(tmp_path, script, env={"CHILD_VALUE": {"ref": name}}) == {
        "error": "flyrail-hook:missing-env"
    }
    assert not marker.exists()


@pytest.mark.parametrize("host", ["claude", "pi", "opencode"])
@pytest.mark.parametrize("references", [False, True])
def test_emitted_host_environment_preserves_every_declared_name(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, host: str, references: bool
) -> None:
    for name in NAMES:
        monkeypatch.setenv(name, "ambient-" + name)
    capture = tmp_path / "environment.json"
    script = (
        'require("node:fs").writeFileSync('
        + json.dumps(str(capture))
        + ",JSON.stringify("
        + json.dumps(NAMES)
        + ".map(name=>[name,Object.hasOwn(process.env,name),process.env[name]])));"
        'process.stdout.write(\'{"version":1,"outcome":"continue"}\')'
    )
    _, _, rendered, _ = install(
        tmp_path,
        host,
        (
            make_hook(
                script=script,
                env={name: EnvRef(name) if references else "literal-" + name for name in NAMES},
            ),
        ),
    )
    event = {"claude": "PreToolUse", "pi": "tool_call", "opencode": "tool.execute.before"}[host]
    raw = (
        {"tool_name": "Bash", "tool_input": {}}
        if host == "claude"
        else {"toolName": "bash", "input": {}}
        if host == "pi"
        else {"tool": "bash", "sessionID": "s", "callID": "c"}
    )
    _, stderr = invoke(tmp_path, rendered, host, event, raw, {"args": {}})
    assert stderr == ""
    assert json.loads(capture.read_text()) == [
        [name, True, ("ambient-" if references else "literal-") + name] for name in NAMES
    ]


@pytest.mark.parametrize("host", ["claude", "copilot", "pi", "opencode"])
@pytest.mark.parametrize("name", NAMES)
def test_missing_environment_reference_discards_accumulated_host_effects(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, host: str, name: str
) -> None:
    monkeypatch.delenv(name, raising=False)
    marker = tmp_path / "executed"
    first = make_hook(
        output={"version": 1, "outcome": "modify-input", "input": {"command": "changed"}}
    )
    second = make_hook(
        "later",
        script='require("node:fs").writeFileSync(' + json.dumps(str(marker)) + ',"ran")',
        env={"CHILD_VALUE": EnvRef(name)},
    )
    _, _, rendered, _ = install(tmp_path, host, (first, second))
    event = {
        "claude": "PreToolUse",
        "copilot": "preToolUse",
        "pi": "tool_call",
        "opencode": "tool.execute.before",
    }[host]
    original = {"command": "original"}
    raw = (
        {"tool_name": "Bash", "tool_input": original}
        if host == "claude"
        else {"toolName": "bash", "toolArgs": original}
        if host == "copilot"
        else {"toolName": "bash", "input": original}
        if host == "pi"
        else {"tool": "bash", "sessionID": "s", "callID": "c"}
    )
    result, stderr = invoke(tmp_path, rendered, host, event, raw, {"args": original})
    assert stderr == "flyrail-hook:missing-env\n"
    if host in {"claude", "copilot"}:
        assert result == {}
    elif host == "pi":
        assert result["result"] is None
        assert result["input"]["input"] == original
    else:
        assert result["blocked"] is None
        assert result["output"] == {"args": original}
    assert not marker.exists()
