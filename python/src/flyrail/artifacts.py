import re
from collections.abc import Iterable, Mapping
from dataclasses import dataclass
from enum import StrEnum
from typing import TypeAlias
from urllib.parse import urlsplit

from flyrail._validation import validate_identifier, validate_relative_path
from flyrail.content import Content, TreeContent, validate_content
from flyrail.targets import Agent, Platform, Surface, TargetScope
from flyrail.values import scalar_text


class Family(StrEnum):
    SKILLS = "skills"
    INSTRUCTIONS = "instructions"
    MCP = "mcp"
    HOOKS = "hooks"


@dataclass(frozen=True, slots=True)
class Audience:
    agent: Agent
    scope: TargetScope
    surface: Surface = Surface.CLI
    platform: Platform | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.agent, Agent) or not isinstance(self.scope, TargetScope):
            raise TypeError("audience requires an Agent and TargetScope")
        if self.scope is TargetScope.DIRECTORY:
            raise ValueError("native audiences require project or user scope")
        if not isinstance(self.surface, Surface):
            raise TypeError("surface must be a Surface")
        if self.platform is not None and not isinstance(self.platform, Platform):
            raise TypeError("platform must be a Platform")
        if self.surface is Surface.IDE and self.agent is not Agent.CURSOR:
            raise ValueError("IDE surface is only defined for Cursor")
        if self.surface is Surface.VSCODE and self.agent is not Agent.COPILOT:
            raise ValueError("VSCode surface is only defined for Copilot")


@dataclass(frozen=True, slots=True)
class EnvRef:
    name: str

    def __post_init__(self) -> None:
        if not isinstance(self.name, str):
            raise TypeError("environment reference must be a string")
        if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", self.name) is None:
            raise ValueError("invalid environment reference")


@dataclass(frozen=True, slots=True)
class AssetRef:
    asset_id: str
    path: str | None = None

    def __post_init__(self) -> None:
        validate_identifier(self.asset_id, "referenced asset id")
        if self.path is not None:
            validate_relative_path(self.path, "asset reference path")


@dataclass(frozen=True, slots=True, init=False)
class Command:
    argv: tuple[str | AssetRef, ...]
    env: tuple[tuple[str, str | EnvRef], ...]
    cwd: str | AssetRef | None

    def __init__(
        self,
        argv: Iterable[str | AssetRef],
        env: Mapping[str, str | EnvRef] | Iterable[tuple[str, str | EnvRef]] = (),
        cwd: str | AssetRef | None = None,
    ) -> None:
        if isinstance(argv, str | bytes):
            raise TypeError("argv must be a sequence of arguments")
        arguments = tuple(argv)
        if not arguments:
            raise ValueError("argv requires a command")
        for argument in arguments:
            if isinstance(argument, AssetRef):
                continue
            scalar_text(argument, "command argument")
            if "\0" in argument:
                raise ValueError("command arguments cannot contain NUL")
        if not arguments[0]:
            raise ValueError("command cannot be empty")
        pairs = tuple(
            (key, value) for key, value in (env.items() if isinstance(env, Mapping) else env)
        )
        seen: set[str] = set()
        for key, value in pairs:
            EnvRef(key)
            if key in seen:
                raise ValueError("duplicate environment name")
            seen.add(key)
            if not isinstance(value, EnvRef):
                scalar_text(value, "environment value")
                if "\0" in value:
                    raise ValueError("environment values cannot contain NUL")
        if cwd is not None and not isinstance(cwd, AssetRef):
            validate_relative_path(cwd, "command cwd")
        object.__setattr__(self, "argv", arguments)
        object.__setattr__(self, "env", tuple(sorted(pairs)))
        object.__setattr__(self, "cwd", cwd)


@dataclass(frozen=True, slots=True)
class HttpTransport:
    url: str
    bearer_token: EnvRef | None = None

    def __post_init__(self) -> None:
        scalar_text(self.url, "MCP URL")
        parsed = urlsplit(self.url)
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            raise ValueError("MCP URL must be an absolute HTTP or HTTPS URL")
        if parsed.username is not None or parsed.password is not None or parsed.fragment:
            raise ValueError("MCP URL cannot embed credentials or a fragment")
        if any(character.isspace() or ord(character) < 32 for character in self.url):
            raise ValueError("MCP URL cannot contain whitespace or control characters")
        if parsed.port is not None and not 1 <= parsed.port <= 65535:
            raise ValueError("MCP URL port must be 1-65535")
        if self.bearer_token is not None and type(self.bearer_token) is not EnvRef:
            raise TypeError("bearer token must reference an environment variable")


@dataclass(frozen=True, slots=True)
class SkillArtifact:
    id: str
    name: str
    content: TreeContent

    def __post_init__(self) -> None:
        validate_identifier(self.id, "artifact id")
        validate_identifier(self.name, "skill name")
        if type(self.content) is not TreeContent:
            raise TypeError("skill content must be a TreeContent")
        if not any(e.path == "SKILL.md" and e.data is not None for e in self.content.entries):
            raise ValueError("skill must contain a SKILL.md file")


@dataclass(frozen=True, slots=True)
class InstructionArtifact:
    id: str
    text: str

    def __post_init__(self) -> None:
        validate_identifier(self.id, "artifact id")
        scalar_text(self.text, "instruction text")
        if "\0" in self.text:
            raise ValueError("instruction text cannot contain NUL")


@dataclass(frozen=True, slots=True)
class McpArtifact:
    id: str
    name: str
    transport: Command | HttpTransport

    def __post_init__(self) -> None:
        validate_identifier(self.id, "artifact id")
        validate_identifier(self.name, "MCP server name")
        if type(self.transport) not in {Command, HttpTransport}:
            raise TypeError("MCP transport must be a Command or HttpTransport")


class HookEvent(StrEnum):
    SESSION_START = "session-start"
    BEFORE_TOOL = "before-tool"
    AFTER_TOOL = "after-tool"
    PROMPT = "prompt"
    STOP = "stop"


class HookOutcome(StrEnum):
    CONTINUE = "continue"
    BLOCK = "block"
    ASK = "ask"
    MODIFY_INPUT = "modify-input"
    CONTEXT = "context"


@dataclass(frozen=True, slots=True, init=False)
class HookArtifact:
    id: str
    event: HookEvent
    command: Command
    timeout_ms: int
    outcomes: tuple[HookOutcome, ...]
    tools: tuple[str, ...]
    protocol_version: int = 1

    def __init__(
        self,
        id: str,
        event: HookEvent,
        command: Command,
        timeout_ms: int = 10000,
        outcomes: Iterable[HookOutcome] = (HookOutcome.CONTINUE,),
        tools: Iterable[str] = (),
    ) -> None:
        validate_identifier(id, "artifact id")
        if not isinstance(event, HookEvent) or type(command) is not Command:
            raise TypeError("hook requires a HookEvent and Command")
        if type(timeout_ms) is not int or not 1 <= timeout_ms <= 300000:
            raise ValueError("hook timeout must be 1-300000 integer milliseconds")
        selected = tuple(outcomes)
        if not selected or any(not isinstance(outcome, HookOutcome) for outcome in selected):
            raise ValueError("hook requires declared HookOutcome values")
        if len(set(selected)) != len(selected):
            raise ValueError("hook outcomes must be unique")
        if isinstance(tools, str | bytes):
            raise TypeError("tools must be a sequence of exact tool names")
        names = tuple(tools)
        for name in names:
            scalar_text(name, "tool name")
            if not name or "\0" in name:
                raise ValueError("tool names must be nonempty and cannot contain NUL")
        if len(set(names)) != len(names):
            raise ValueError("tool names must be unique")
        if names and event not in {HookEvent.BEFORE_TOOL, HookEvent.AFTER_TOOL}:
            raise ValueError("tool selection requires a tool event")
        for name, value in (
            ("id", id),
            ("event", event),
            ("command", command),
            ("timeout_ms", timeout_ms),
            ("outcomes", tuple(sorted(selected))),
            ("tools", tuple(sorted(names))),
            ("protocol_version", 1),
        ):
            object.__setattr__(self, name, value)


@dataclass(frozen=True, slots=True)
class NativeArtifact:
    id: str
    family: Family
    audience: Audience
    destination: str
    content: Content

    def __post_init__(self) -> None:
        validate_identifier(self.id, "artifact id")
        if not isinstance(self.family, Family) or type(self.audience) is not Audience:
            raise TypeError("native artifact requires a Family and Audience")
        validate_relative_path(self.destination, "native destination")
        validate_content(self.content)


@dataclass(frozen=True, slots=True)
class SupportAsset:
    id: str
    family: Family
    content: TreeContent

    def __post_init__(self) -> None:
        validate_identifier(self.id, "asset id")
        if not isinstance(self.family, Family) or type(self.content) is not TreeContent:
            raise TypeError("support assets require a Family and TreeContent")


Artifact: TypeAlias = (
    SkillArtifact | InstructionArtifact | McpArtifact | HookArtifact | NativeArtifact
)


def validate_artifact(artifact: Artifact) -> None:
    if type(artifact) not in {
        SkillArtifact,
        InstructionArtifact,
        McpArtifact,
        HookArtifact,
        NativeArtifact,
    }:
        raise TypeError("unsupported artifact type")
