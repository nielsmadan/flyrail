from dataclasses import dataclass
from pathlib import Path

from flyrail.artifacts import Audience, Family
from flyrail.configuration import InstallationTarget
from flyrail.hooks import HookRuntime
from flyrail.targets import Agent, Platform, Surface, Target, TargetScope, _relocation


class UnsupportedTranslation(Exception):
    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def _path(value: Path) -> Path:
    if not isinstance(value, Path) or not value.is_absolute():
        raise ValueError("render routes must be absolute Paths")
    if ".." in value.parts or "\0" in str(value):
        raise ValueError("render routes must be unambiguous paths without NUL")
    return value


@dataclass(frozen=True, slots=True)
class RenderContext:
    target: Target
    platform: Platform
    surface: Surface = Surface.CLI
    execution_root: Path | None = None
    instruction_path: Path | None = None
    mcp_path: Path | None = None
    native_root: Path | None = None
    asset_root: Path | None = None
    hook_runtime: HookRuntime | None = None

    def __post_init__(self) -> None:
        if type(self.target) is not Target or self.target.agent is None:
            raise ValueError("render context requires a project or user agent Target")
        if not isinstance(self.platform, Platform):
            raise TypeError("render platform must be a Platform")
        Audience(self.target.agent, self.target.scope, self.surface, self.platform)
        if self.hook_runtime is not None and type(self.hook_runtime) is not HookRuntime:
            raise TypeError("hook_runtime must be a HookRuntime")
        for value in (
            self.target.root,
            self.root,
            self.execution_root,
            self.instruction_path,
            self.mcp_path,
            self.native_root,
            self.asset_root,
        ):
            if value is not None:
                _path(value)

    @property
    def root(self) -> Path:
        return self.target.home if self.target.home is not None else self.target._anchor

    @property
    def audience(self) -> Audience:
        agent = self.target.agent
        if agent is None:
            raise ValueError("render context requires an agent")
        return Audience(agent, self.target.scope, self.surface, self.platform)

    @property
    def skills(self) -> Path:
        if self.surface is Surface.VSCODE and self.target.scope is TargetScope.USER:
            return self.root / ".copilot/skills"
        return self.target.root

    @property
    def routing_context(self) -> tuple[tuple[str, str], ...]:
        return tuple(
            sorted(
                (
                    ("agent", str(self.audience.agent)),
                    ("scope", str(self.target.scope)),
                    ("surface", str(self.surface)),
                    ("platform", str(self.platform)),
                    ("root", str(self.root)),
                    ("skills", str(self.skills)),
                    ("execution", str(self.execution_root or self.root)),
                    ("instructions", str(self.instruction_path or "")),
                    ("mcp", str(self.mcp_path or "")),
                    ("native", str(self.native_root or self.root)),
                    ("assets", str(self.asset_root or self.root / ".flyrail-assets")),
                    ("hook-node", str(self.hook_runtime.node or "") if self.hook_runtime else ""),
                    ("hook-shell", str(self.hook_runtime.shell or "") if self.hook_runtime else ""),
                    *(("env:" + key, value) for key, value in self.target.environment),
                )
            )
        )

    def installation(self, index_root: Path) -> InstallationTarget:
        return InstallationTarget(index_root, routing_context=self.routing_context)


def _user_directory(context: RenderContext) -> Path | None:
    agent = context.audience.agent
    paths = {
        Agent.CLAUDE: ("CLAUDE_CONFIG_DIR", ".claude"),
        Agent.CODEX: ("CODEX_HOME", ".codex"),
        Agent.OPENCODE: ("XDG_CONFIG_HOME", ".config"),
        Agent.PI: ("PI_CODING_AGENT_DIR", ".pi/agent"),
        Agent.CURSOR: ("", ".cursor"),
        Agent.COPILOT: ("COPILOT_HOME", ".copilot"),
    }
    entry = paths.get(agent)
    if entry is None:
        return None
    variable, suffix = entry
    if context.surface is Surface.VSCODE:
        variable = ""
    value = dict(context.target.environment).get(variable)
    root = _relocation(value, context.root) if value else context.root / suffix
    return root / "opencode" if agent is Agent.OPENCODE else root


def instruction_destination(context: RenderContext) -> Path | None:
    if context.instruction_path is not None:
        return context.instruction_path
    agent = context.audience.agent
    if context.target.scope is TargetScope.PROJECT:
        name = {
            Agent.CLAUDE: "CLAUDE.md",
            Agent.CODEX: "AGENTS.md",
            Agent.OPENCODE: "AGENTS.md",
            Agent.PI: "AGENTS.md",
            Agent.CURSOR: "AGENTS.md",
            Agent.COPILOT: ".github/copilot-instructions.md",
        }.get(agent)
        return None if name is None else context.root / name
    if agent is Agent.CURSOR:
        return None
    if context.surface is Surface.VSCODE:
        directory = _user_directory(context)
        return None if directory is None else directory / "instructions"
    name = {
        Agent.CLAUDE: "CLAUDE.md",
        Agent.CODEX: "AGENTS.md",
        Agent.OPENCODE: "AGENTS.md",
        Agent.PI: "AGENTS.md",
        Agent.COPILOT: "copilot-instructions.md",
    }.get(agent)
    if name is None:
        return None
    directory = _user_directory(context)
    return None if directory is None else directory / name


def mcp_destination(context: RenderContext) -> Path | None:
    if context.mcp_path is not None:
        return context.mcp_path
    agent = context.audience.agent
    if context.target.scope is TargetScope.PROJECT:
        if context.surface is Surface.VSCODE:
            return context.root / ".vscode/mcp.json"
        name = {
            Agent.CLAUDE: ".mcp.json",
            Agent.CODEX: ".codex/config.toml",
            Agent.OPENCODE: "opencode.json",
            Agent.PI: ".pi/mcp.json",
            Agent.CURSOR: ".cursor/mcp.json",
            Agent.COPILOT: ".mcp.json",
        }.get(agent)
        return None if name is None else context.root / name
    if context.surface is Surface.VSCODE:
        return None
    if agent is Agent.CLAUDE:
        configured = dict(context.target.environment).get("CLAUDE_CONFIG_DIR")
        return None if configured else context.root / ".claude.json"
    if agent is Agent.OPENCODE:
        configured = dict(context.target.environment).get("OPENCODE_CONFIG")
        if configured:
            return _relocation(configured, context.root)
    name = {
        Agent.CODEX: "config.toml",
        Agent.OPENCODE: "opencode.json",
        Agent.PI: "mcp.json",
        Agent.CURSOR: "mcp.json",
        Agent.COPILOT: "mcp-config.json",
    }.get(agent)
    if name is None:
        return None
    directory = _user_directory(context)
    return None if directory is None else directory / name


@dataclass(frozen=True, slots=True)
class Capability:
    family: Family
    portable: bool
    native: bool
    limitations: tuple[str, ...] = ()
    prerequisites: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        if (
            not isinstance(self.family, Family)
            or type(self.portable) is not bool
            or type(self.native) is not bool
        ):
            raise TypeError("capabilities require a Family and boolean support flags")
        for name in ("limitations", "prerequisites"):
            values = getattr(self, name)
            if isinstance(values, str | bytes):
                raise TypeError("capability details require a sequence of strings")
            snapshot = tuple(values)
            if any(not isinstance(value, str) or not value.strip() for value in snapshot):
                raise ValueError("capability details must be nonblank strings")
            object.__setattr__(self, name, snapshot)


TRANSLATABLE_AGENTS = frozenset(
    {Agent.CLAUDE, Agent.CODEX, Agent.OPENCODE, Agent.PI, Agent.CURSOR, Agent.COPILOT}
)


def untranslatable_message(agent: Agent) -> str:
    return f"Flyrail detects {agent.value} but has no verified configuration translation."


def require_translatable(context: RenderContext) -> None:
    agent = context.audience.agent
    if agent not in TRANSLATABLE_AGENTS:
        raise UnsupportedTranslation("agent-unsupported", untranslatable_message(agent))


def capabilities(context: RenderContext) -> tuple[Capability, ...]:
    if type(context) is not RenderContext:
        raise TypeError("capabilities require a RenderContext")
    if context.audience.agent not in TRANSLATABLE_AGENTS:
        return (
            Capability(Family.SKILLS, False, False),
            Capability(Family.INSTRUCTIONS, False, False),
            Capability(Family.MCP, False, False),
            Capability(Family.HOOKS, False, False),
        )
    return (
        Capability(Family.SKILLS, True, True),
        Capability(
            Family.INSTRUCTIONS,
            instruction_destination(context) is not None,
            True,
            ("Discovery, overrides, byte limits and session loading remain host-controlled.",),
        ),
        Capability(
            Family.MCP,
            mcp_destination(context) is not None,
            True,
            ("Transport, interpolation, environment references and cwd are checked per request.",),
            ("pi-mcp-adapter@2.32.1",) if context.audience.agent is Agent.PI else (),
        ),
        Capability(
            Family.HOOKS,
            context.surface is not Surface.VSCODE and context.hook_runtime is not None,
            True,
            ("Event/outcome and native shell support are validated for each request.",),
            ("Node 24+; explicit executable for native hosts. See the hook protocol matrix.",),
        ),
    )
