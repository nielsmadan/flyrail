import os
import stat
import sys
from pathlib import Path

import pytest

from flyrail import Agent, AgentPresence, detect_agents
from flyrail.discovery import _application_directories


def executable(directory: Path, name: str) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / name
    path.write_text("#!/bin/sh\nexit 0\n")
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return path


def bundle(directory: Path, name: str) -> Path:
    path = directory / name
    path.mkdir(parents=True, exist_ok=True)
    return path


def test_detects_agents_by_executable(tmp_path: Path) -> None:
    binaries = tmp_path / "bin"
    executable(binaries, "claude")
    executable(binaries, "codex")

    found = detect_agents(path=str(binaries), applications=())

    assert [item.agent for item in found] == [Agent.CLAUDE, Agent.CODEX]
    assert found[0].executable == binaries / "claude"
    assert found[0].application is None


def test_droid_is_detected_from_its_executable(tmp_path: Path) -> None:
    binary = tmp_path / "droid"
    binary.write_text("#!/bin/sh\n")
    binary.chmod(0o755)
    found = detect_agents(path=str(tmp_path), applications=())
    assert [presence.agent for presence in found] == [Agent.DROID]
    assert found[0].executable == binary
    assert found[0].application is None


def test_droid_is_absent_without_an_executable(tmp_path: Path) -> None:
    binaries = tmp_path / "bin"
    executable(binaries, "claude")
    (binaries / "droid").write_text("#!/bin/sh\nexit 0\n")

    found = detect_agents(path=str(binaries), applications=())

    assert [presence.agent for presence in found] == [Agent.CLAUDE]


def test_omits_agents_without_evidence(tmp_path: Path) -> None:
    binaries = tmp_path / "bin"
    executable(binaries, "claude")

    found = detect_agents(path=str(binaries), applications=())

    assert [item.agent for item in found] == [Agent.CLAUDE]


def test_results_follow_agent_declaration_order(tmp_path: Path) -> None:
    binaries = tmp_path / "bin"
    for name in ("pi", "claude", "opencode"):
        executable(binaries, name)

    found = detect_agents(path=str(binaries), applications=())

    assert [item.agent for item in found] == [Agent.CLAUDE, Agent.OPENCODE, Agent.PI]


@pytest.mark.skipif(sys.platform != "darwin", reason="macOS application bundles")
def test_detects_ide_agents_by_application(tmp_path: Path) -> None:
    directory = tmp_path / "Applications"
    cursor = bundle(directory, "Cursor.app")
    code = bundle(directory, "Visual Studio Code.app")

    found = detect_agents(path=str(tmp_path / "empty"), applications=[directory])

    assert [item.agent for item in found] == [Agent.CURSOR, Agent.COPILOT]
    assert found[0].application == cursor
    assert found[0].executable is None
    assert found[1].application == code


@pytest.mark.skipif(sys.platform != "darwin", reason="macOS application bundles")
def test_reports_both_evidence_kinds(tmp_path: Path) -> None:
    binaries = tmp_path / "bin"
    directory = tmp_path / "Applications"
    executable(binaries, "cursor")
    cursor = bundle(directory, "Cursor.app")

    found = detect_agents(path=str(binaries), applications=[directory])

    assert found[0].executable == binaries / "cursor"
    assert found[0].application == cursor


@pytest.mark.skipif(sys.platform == "darwin", reason="non-macOS ignores applications")
def test_application_directories_are_ignored_off_macos(tmp_path: Path) -> None:
    directory = tmp_path / "Applications"
    bundle(directory, "Cursor.app")

    assert detect_agents(path=str(tmp_path / "empty"), applications=[directory]) == ()


def test_claude_desktop_application_is_not_claude_code(tmp_path: Path) -> None:
    directory = tmp_path / "Applications"
    bundle(directory, "Claude.app")

    assert detect_agents(path=str(tmp_path / "empty"), applications=[directory]) == ()


def test_non_executable_files_are_not_evidence(tmp_path: Path) -> None:
    binaries = tmp_path / "bin"
    binaries.mkdir()
    candidate = binaries / "claude"
    candidate.write_text("#!/bin/sh\nexit 0\n")
    candidate.chmod(0o644)

    assert detect_agents(path=str(binaries), applications=()) == ()


def test_configuration_directories_are_not_evidence(tmp_path: Path) -> None:
    for name in (".claude", ".codex", ".agents", ".opencode", ".pi", ".cursor", ".copilot"):
        (tmp_path / name).mkdir()

    assert detect_agents(path=str(tmp_path / "empty"), applications=()) == ()


def test_presence_requires_evidence() -> None:
    with pytest.raises(ValueError, match="evidence"):
        AgentPresence(Agent.CLAUDE)


def test_presence_rejects_relative_evidence() -> None:
    with pytest.raises(ValueError, match="absolute"):
        AgentPresence(Agent.CLAUDE, Path("claude"))


def test_detect_rejects_invalid_arguments() -> None:
    with pytest.raises(TypeError, match="path"):
        detect_agents(path=os.sep.encode())  # type: ignore[arg-type]


def test_presence_rejects_a_non_agent() -> None:
    with pytest.raises(TypeError, match="requires an Agent"):
        AgentPresence("claude", Path(os.sep) / "claude")  # type: ignore[arg-type]


def test_presence_rejects_non_path_evidence() -> None:
    with pytest.raises(TypeError, match="must be a Path"):
        AgentPresence(Agent.CLAUDE, os.sep + "claude")  # type: ignore[arg-type]


def test_detect_rejects_non_path_application_directories() -> None:
    with pytest.raises(TypeError, match="must be Path values"):
        detect_agents(applications=[os.sep + "Applications"])  # type: ignore[list-item]


def test_default_application_directories_are_the_standard_pair() -> None:
    expected = (Path("/Applications"), Path.home() / "Applications")
    directories = _application_directories(None)
    assert directories == (expected if sys.platform == "darwin" else ())
