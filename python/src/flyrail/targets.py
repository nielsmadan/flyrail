import os
from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path


class Agent(StrEnum):
    CLAUDE = "claude"
    CODEX = "codex"
    OPENCODE = "opencode"
    PI = "pi"
    CURSOR = "cursor"
    COPILOT = "copilot"


class TargetScope(StrEnum):
    DIRECTORY = "directory"
    USER = "user"
    PROJECT = "project"


_PROJECT = {
    Agent.CLAUDE: (".claude", "skills"),
    Agent.CODEX: (".agents", "skills"),
    Agent.OPENCODE: (".opencode", "skills"),
    Agent.PI: (".pi", "skills"),
    Agent.CURSOR: (".cursor", "skills"),
    Agent.COPILOT: (".github", "skills"),
}
_USER = {
    **_PROJECT,
    Agent.OPENCODE: (".config", "opencode", "skills"),
    Agent.PI: (".pi", "agent", "skills"),
    Agent.COPILOT: (".copilot", "skills"),
}
_RELOCATIONS = {
    Agent.CLAUDE: ("CLAUDE_CONFIG_DIR", ("skills",)),
    Agent.OPENCODE: ("XDG_CONFIG_HOME", ("opencode", "skills")),
    Agent.PI: ("PI_CODING_AGENT_DIR", ("skills",)),
}


def _absolute(root: str | os.PathLike[str]) -> Path:
    value = os.fspath(root)
    if not isinstance(value, str):
        raise TypeError("target roots must be string paths")
    if not value or "\0" in value:
        raise ValueError("target roots must be nonempty paths without NUL")
    path = Path(value)
    return path if path.is_absolute() else Path.cwd() / path


def _agent(value: Agent | str) -> Agent:
    if not isinstance(value, str):
        raise TypeError("agent must be an Agent or string")
    return Agent(value)


def _environment(env: Mapping[str, str] | None) -> dict[str, str]:
    if env is None:
        return dict(os.environ)
    if not isinstance(env, Mapping):
        raise TypeError("env must be a mapping of strings")
    snapshot = dict(env)
    if any(
        not isinstance(key, str) or not isinstance(value, str) for key, value in snapshot.items()
    ):
        raise TypeError("env keys and values must be strings")
    return snapshot


def _relocation(value: str, home: Path) -> Path:
    if value == "~":
        return home
    if value.startswith("~/"):
        return _absolute(home / value[2:])
    if not Path(value).is_absolute():
        raise ValueError("environment paths must be absolute or start with ~/")
    return _absolute(value)


@dataclass(frozen=True, slots=True, init=False)
class Target:
    root: Path
    agent: Agent | None
    scope: TargetScope
    _anchor: Path
    _suffix: tuple[str, ...]

    def __init__(self) -> None:
        raise TypeError("use Target.directory, Target.user, or Target.project")

    @classmethod
    def directory(cls, root: str | os.PathLike[str]) -> "Target":
        return cls._create(_absolute(root), (), None, TargetScope.DIRECTORY)

    @classmethod
    def project(cls, agent: Agent | str, root: str | os.PathLike[str]) -> "Target":
        selected = _agent(agent)
        return cls._create(_absolute(root), _PROJECT[selected], selected, TargetScope.PROJECT)

    @classmethod
    def user(
        cls,
        agent: Agent | str,
        *,
        home: str | os.PathLike[str] | None = None,
        env: Mapping[str, str] | None = None,
    ) -> "Target":
        selected = _agent(agent)
        environment = _environment({} if home is not None and env is None else env)
        if home is not None:
            home_root = _absolute(home)
        elif env is None:
            home_root = _absolute(Path.home())
        else:
            value = environment.get("HOME") or environment.get("USERPROFILE")
            if not value:
                raise ValueError("an explicit env requires home, HOME, or USERPROFILE")
            if not Path(value).is_absolute():
                raise ValueError("environment home must be absolute")
            home_root = _absolute(value)
        relocation = _RELOCATIONS.get(selected)
        if relocation is not None and environment.get(relocation[0]):
            return cls._create(
                _relocation(environment[relocation[0]], home_root),
                relocation[1],
                selected,
                TargetScope.USER,
            )
        return cls._create(home_root, _USER[selected], selected, TargetScope.USER)

    @classmethod
    def _create(
        cls, anchor: Path, suffix: tuple[str, ...], agent: Agent | None, scope: TargetScope
    ) -> "Target":
        root = anchor.joinpath(*suffix)
        if root == root.parent:
            raise ValueError("a filesystem root cannot be a skill container")
        result = object.__new__(cls)
        for name, value in (
            ("root", root),
            ("agent", agent),
            ("scope", scope),
            ("_anchor", anchor),
            ("_suffix", suffix),
        ):
            object.__setattr__(result, name, value)
        return result
