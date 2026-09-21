import os
import shutil
import sys
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path

from flyrail.targets import Agent

_EXECUTABLES = {
    Agent.CLAUDE: "claude",
    Agent.CODEX: "codex",
    Agent.OPENCODE: "opencode",
    Agent.PI: "pi",
    Agent.CURSOR: "cursor",
    Agent.COPILOT: "copilot",
}

_APPLICATIONS = {
    Agent.CURSOR: "Cursor.app",
    Agent.COPILOT: "Visual Studio Code.app",
}


@dataclass(frozen=True, slots=True)
class AgentPresence:
    agent: Agent
    executable: Path | None = None
    application: Path | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.agent, Agent):
            raise TypeError("agent presence requires an Agent")
        for value in (self.executable, self.application):
            if value is None:
                continue
            if not isinstance(value, Path):
                raise TypeError("agent presence evidence must be a Path")
            if not value.is_absolute():
                raise ValueError("agent presence evidence must be an absolute Path")
        if self.executable is None and self.application is None:
            raise ValueError("agent presence requires at least one piece of evidence")


def _application_directories(applications: Iterable[Path] | None) -> tuple[Path, ...]:
    directories: tuple[Path, ...]
    if applications is None:
        directories = (Path("/Applications"), Path.home() / "Applications")
    else:
        directories = tuple(applications)
        if any(not isinstance(directory, Path) for directory in directories):
            raise TypeError("application directories must be Path values")
    return directories if sys.platform == "darwin" else ()


def _application(agent: Agent, directories: tuple[Path, ...]) -> Path | None:
    name = _APPLICATIONS.get(agent)
    if name is None:
        return None
    for directory in directories:
        candidate = directory / name
        if candidate.is_dir():
            return candidate
    return None


def detect_agents(
    *, path: str | None = None, applications: Iterable[Path] | None = None
) -> tuple[AgentPresence, ...]:
    if path is not None and not isinstance(path, str):
        raise TypeError("path must be a string")
    directories = _application_directories(applications)
    search = os.environ.get("PATH", "") if path is None else path
    found: list[AgentPresence] = []
    for agent in Agent:
        located = shutil.which(_EXECUTABLES[agent], path=search)
        executable = None if located is None else Path(os.path.abspath(located))
        application = _application(agent, directories)
        if executable is None and application is None:
            continue
        found.append(AgentPresence(agent, executable, application))
    return tuple(found)
