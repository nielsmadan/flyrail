import os
from dataclasses import FrozenInstanceError
from pathlib import Path

import pytest

from flyrail import Agent, Target, TargetScope


@pytest.mark.parametrize(
    ("agent", "user", "project"),
    [
        ("claude", ".claude/skills", ".claude/skills"),
        ("codex", ".agents/skills", ".agents/skills"),
        ("opencode", ".config/opencode/skills", ".opencode/skills"),
        ("pi", ".pi/agent/skills", ".pi/skills"),
        ("cursor", ".cursor/skills", ".cursor/skills"),
        ("copilot", ".copilot/skills", ".github/skills"),
    ],
)
def test_documented_presets(tmp_path: Path, agent: str, user: str, project: str) -> None:
    target = Target.user(agent, home=tmp_path)

    assert target.root == tmp_path / user
    assert target.agent == Agent(agent)
    assert target.scope is TargetScope.USER
    local = Target.project(agent, tmp_path)
    assert local.root == tmp_path / project
    assert local.scope is TargetScope.PROJECT


@pytest.mark.parametrize(
    ("agent", "variable", "suffix"),
    [
        ("claude", "CLAUDE_CONFIG_DIR", "skills"),
        ("opencode", "XDG_CONFIG_HOME", "opencode/skills"),
        ("pi", "PI_CODING_AGENT_DIR", "skills"),
    ],
)
def test_relocations_are_explicit_and_home_override_isolated(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, agent: str, variable: str, suffix: str
) -> None:
    other = tmp_path / "other"
    monkeypatch.setenv(variable, str(other))
    normal = Target.user(agent, home=tmp_path)
    isolated = Target.user(agent, home=tmp_path, env={})
    environment = {variable: str(other)}
    relocated = Target.user(agent, home=tmp_path, env=environment)
    environment[variable] = str(tmp_path / "later")

    assert normal == isolated
    assert relocated.root == other / suffix
    assert Target.user(agent, home=tmp_path, env={variable: ""}) == normal
    assert (
        Target.user(agent, home=tmp_path, env={variable: "~/chosen"}).root
        == tmp_path / "chosen" / suffix
    )
    assert Target.user(agent, home=tmp_path, env={variable: "~"}).root == tmp_path / suffix
    with pytest.raises(ValueError, match="absolute"):
        Target.user(agent, home=tmp_path, env={variable: "relative"})


def test_unrelated_configuration_variables_do_not_relocate_skills(tmp_path: Path) -> None:
    env = {
        "CODEX_HOME": str(tmp_path / "codex"),
        "OPENCODE_CONFIG_DIR": str(tmp_path / "opencode"),
        "CURSOR_CONFIG_DIR": str(tmp_path / "cursor"),
        "COPILOT_CONFIG_DIR": str(tmp_path / "copilot"),
    }

    for agent in (Agent.CODEX, Agent.OPENCODE, Agent.CURSOR, Agent.COPILOT):
        assert Target.user(agent, home=tmp_path, env=env) == Target.user(
            agent, home=tmp_path, env={}
        )


def test_environment_home_replaces_ambient_and_ambient_defaults_are_supported(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(Path, "home", lambda: tmp_path / "ambient")
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path / "ambient-config"))

    assert Target.user("codex").root == tmp_path / "ambient/.agents/skills"
    assert Target.user("claude").root == tmp_path / "ambient-config/skills"
    assert (
        Target.user("claude", env={"HOME": str(tmp_path / "chosen")}).root
        == tmp_path / "chosen/.claude/skills"
    )
    assert (
        Target.user("codex", env={"USERPROFILE": str(tmp_path)}).root == tmp_path / ".agents/skills"
    )
    with pytest.raises(ValueError, match="explicit env requires"):
        Target.user("codex", env={})
    with pytest.raises(ValueError, match="home must be absolute"):
        Target.user("codex", env={"HOME": "relative"})


def test_target_inputs_and_immutability(tmp_path: Path) -> None:
    target = Target.directory(tmp_path)

    assert target.root == tmp_path
    assert target.agent is None
    assert target.scope is TargetScope.DIRECTORY
    assert {target: "custom"}[Target.directory(str(tmp_path))] == "custom"
    assert Target.directory("relative").root == Path.cwd() / "relative"
    with pytest.raises(FrozenInstanceError):
        target.root = tmp_path / "other"  # type: ignore[misc]
    with pytest.raises(TypeError, match="use Target"):
        Target()
    with pytest.raises(TypeError, match="string paths"):
        Target.directory(b"bytes")  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        Target.directory(7)  # type: ignore[arg-type]
    for invalid in ("", "a\0b"):
        with pytest.raises(ValueError, match="nonempty"):
            Target.directory(invalid)
    with pytest.raises(ValueError, match="filesystem root"):
        Target.directory(Path(tmp_path.anchor))
    with pytest.raises(ValueError):
        Target.user("unknown", home=tmp_path)
    with pytest.raises(TypeError, match="agent must"):
        Target.project(7, tmp_path)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="mapping"):
        Target.user("codex", home=tmp_path, env=[])  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="keys and values"):
        Target.user("codex", home=tmp_path, env={"HOME": 7})  # type: ignore[dict-item]
    with pytest.raises(ValueError, match="NUL"):
        Target.user(
            "claude", home=tmp_path, env={"CLAUDE_CONFIG_DIR": str(tmp_path) + os.sep + "a\0b"}
        )
