import hashlib
import json
import shlex
from pathlib import Path

from flyrail._mcp_translation import _asset
from flyrail.artifacts import AssetRef, EnvRef, Family, HookArtifact, HookEvent, HookOutcome
from flyrail.bundle import Bundle
from flyrail.content import (
    DocumentFormat,
    DocumentSchema,
    FileContent,
    Key,
    Member,
    Selector,
    StructuredContent,
    TreeContent,
)
from flyrail.destinations import (
    RenderContext,
    UnsupportedTranslation,
    _user_directory,
    require_translatable,
    untranslatable_message,
)
from flyrail.hooks import HookShell
from flyrail.models import BundleEntry
from flyrail.rendered import Dependency, DependencyMode, Notice, NoticeKind, RenderedArtifact
from flyrail.targets import Agent, Platform, Surface, TargetScope
from flyrail.values import Scalar, freeze_value

_EVENTS = {
    Agent.CLAUDE: ["SessionStart", "PreToolUse", "PostToolUse", "UserPromptSubmit", "Stop"],
    Agent.CODEX: ["SessionStart", "PreToolUse", "PostToolUse", "UserPromptSubmit", "Stop"],
    Agent.CURSOR: ["sessionStart", "preToolUse", "postToolUse", "beforeSubmitPrompt", "stop"],
    Agent.COPILOT: [
        "sessionStart",
        "preToolUse",
        "postToolUse",
        "userPromptSubmitted",
        "agentStop",
    ],
}


def _name(*values: str) -> str:
    return hashlib.sha256(json.dumps(values, separators=(",", ":")).encode()).hexdigest()


def _json(value: object) -> str:
    return json.dumps(value, ensure_ascii=True, separators=(",", ":"))


def _supported(artifact: HookArtifact, context: RenderContext) -> None:
    require_translatable(context)
    agent = context.audience.agent
    if context.surface is Surface.VSCODE:
        raise UnsupportedTranslation(
            "hook-surface", "Portable hooks target Copilot CLI, not VS Code."
        )
    if (
        artifact.event is HookEvent.BEFORE_TOOL
        and HookOutcome.MODIFY_INPUT in artifact.outcomes
        and agent in {Agent.CODEX, Agent.CURSOR}
    ):
        raise UnsupportedTranslation(
            "hook-modification-approval",
            "This host has no verified argument-replacement channel "
            "preserving normal approval flow.",
        )
    outcomes = {HookOutcome.CONTINUE}
    if artifact.event is HookEvent.BEFORE_TOOL:
        outcomes |= {HookOutcome.BLOCK, HookOutcome.MODIFY_INPUT}
        if agent in {Agent.CLAUDE, Agent.COPILOT}:
            outcomes.add(HookOutcome.ASK)
        if agent in {Agent.CLAUDE, Agent.CODEX}:
            outcomes.add(HookOutcome.CONTEXT)
    elif artifact.event is HookEvent.AFTER_TOOL:
        outcomes.add(HookOutcome.CONTEXT)
    elif artifact.event is HookEvent.SESSION_START:
        if agent not in {Agent.OPENCODE, Agent.PI}:
            outcomes.add(HookOutcome.CONTEXT)
    elif artifact.event is HookEvent.PROMPT:
        if agent is Agent.OPENCODE:
            outcomes.clear()
        if agent in {Agent.CLAUDE, Agent.CODEX, Agent.CURSOR}:
            outcomes.add(HookOutcome.BLOCK)
        if agent in {Agent.CLAUDE, Agent.CODEX}:
            outcomes.add(HookOutcome.CONTEXT)
        if agent is Agent.PI:
            outcomes.add(HookOutcome.MODIFY_INPUT)
    elif agent is Agent.OPENCODE:
        outcomes.clear()
    if not set(artifact.outcomes) <= outcomes:
        raise UnsupportedTranslation(
            "hook-outcome",
            f"{agent} cannot preserve the declared {artifact.event} outcomes: "
            + ", ".join(sorted(set(artifact.outcomes) - outcomes)),
        )
    names = [key.casefold() for key, _ in artifact.command.env]
    if context.platform is Platform.WINDOWS and len(names) != len(set(names)):
        raise UnsupportedTranslation(
            "windows-env-collision", "Hook environment names collide on Windows."
        )


def _shell(argv: list[str], context: RenderContext) -> str:
    runtime = context.hook_runtime
    if runtime is None:
        raise UnsupportedTranslation(
            "hook-runtime", "Native hooks require an explicit Node 24+ executable."
        )
    if context.platform is Platform.WINDOWS:
        if context.audience.agent is Agent.CURSOR:
            raise UnsupportedTranslation(
                "cursor-windows-shell",
                "Cursor Windows hook shell selection is unverified; use native content.",
            )
        if runtime.shell is not HookShell.CMD:
            raise UnsupportedTranslation(
                "hook-shell", "Windows Codex requires a caller-confirmed cmd.exe /C hook shell."
            )
        if any(any(c in value for c in '%!&|<>^()"\r\n') for value in argv):
            raise UnsupportedTranslation(
                "hook-shell-literal",
                "These native launcher paths cannot be represented by the cmd hook bridge.",
            )
        return " ".join('"' + value + '"' for value in argv)
    if runtime.shell is not HookShell.POSIX:
        raise UnsupportedTranslation(
            "hook-shell",
            "This native hook requires a caller-confirmed POSIX-compatible hook shell.",
        )
    return shlex.join(argv)


def _config(
    artifact: HookArtifact, context: RenderContext, assets: dict[str, Path]
) -> dict[str, object]:
    command = artifact.command
    cwd = (
        _asset(command.cwd, assets)
        if isinstance(command.cwd, AssetRef)
        else str((context.execution_root or context.root) / command.cwd)
        if command.cwd
        else str(context.execution_root or context.root)
    )
    return {
        "id": artifact.id,
        "event": str(artifact.event),
        "argv": [_asset(value, assets) for value in command.argv],
        "cwd": cwd,
        "env": {
            key: {"ref": value.name} if isinstance(value, EnvRef) else value
            for key, value in command.env
        },
        "timeout_ms": artifact.timeout_ms,
        "outcomes": list(artifact.outcomes),
        "tools": list(artifact.tools),
    }


def hook_artifacts(
    bundle: Bundle, context: RenderContext, assets: dict[str, Path]
) -> tuple[list[RenderedArtifact], list[Dependency], list[Notice]]:
    hooks = [artifact for artifact in bundle.artifacts if isinstance(artifact, HookArtifact)]
    if not hooks:
        return [], [], []
    for artifact in hooks:
        _supported(artifact, context)
    for event in HookEvent:
        handlers = [artifact for artifact in hooks if artifact.event is event]
        if len(handlers) > 64 or sum(artifact.timeout_ms for artifact in handlers) > 300000:
            raise UnsupportedTranslation(
                "hook-event-budget",
                "Each event permits at most 64 handlers and 300000 ms total declared timeout.",
            )
    agent = context.audience.agent
    embedded = agent in {Agent.OPENCODE, Agent.PI}
    runtime = context.hook_runtime
    if runtime is None or (not embedded and runtime.node is None):
        raise UnsupportedTranslation(
            "hook-runtime", "Native hooks require an explicit Node 24+ executable."
        )
    if context.target.scope is TargetScope.USER:
        native_directory = _user_directory(context)
        if native_directory is None:
            raise UnsupportedTranslation("agent-unsupported", untranslatable_message(agent))
    else:
        native_directory = (
            context.root
            / {
                Agent.CLAUDE: ".claude",
                Agent.CODEX: ".codex",
                Agent.CURSOR: ".cursor",
                Agent.COPILOT: ".github",
                Agent.OPENCODE: ".opencode",
                Agent.PI: ".pi",
            }[agent]
        )
    base = (
        (context.asset_root or context.root / ".flyrail-assets") / bundle.id / ".hooks" / str(agent)
    )
    if any(part.casefold() == "node_modules" for part in base.parts):
        raise UnsupportedTranslation(
            "hook-node-modules",
            "Node cannot load stripped TypeScript from a node_modules asset root.",
        )
    config = {
        "host": str(agent),
        "handlers": [_config(artifact, context, assets) for artifact in hooks],
    }
    entries = list(runtime.resources)
    entries.append(
        BundleEntry(
            "config.mts",
            (
                "import type { Config } from './runner.mts';\nconst config: Config = JSON.parse("
                + _json(_json(config))
                + ");\nexport default config;\n"
            ).encode(),
        )
    )
    digest = hashlib.sha256(b"".join(entry.data or b"" for entry in entries)).hexdigest()
    revision = base / "revisions" / digest
    runtime_id = _name(bundle.id, str(agent), "runtime")
    launcher_id = _name(bundle.id, str(agent), "launcher")
    reserved = {runtime_id, launcher_id, _name(bundle.id, str(agent), "version")}
    if reserved & (
        {artifact.id for artifact in bundle.artifacts} | {asset.id for asset in bundle.assets}
    ):
        raise UnsupportedTranslation(
            "hook-generated-id", "An authored identifier collides with a generated hook identifier."
        )
    generated = [RenderedArtifact(runtime_id, Family.HOOKS, revision, TreeContent(entries))]
    edges = [
        Dependency(runtime_id, edge.required)
        for edge in bundle.dependencies
        if edge.dependent in {artifact.id for artifact in hooks}
    ]
    bridge_url = _json((revision / "bridge.mts").as_uri())
    config_url = _json((revision / "config.mts").as_uri())
    if embedded:
        launcher = (
            native_directory
            / ("plugins" if agent is Agent.OPENCODE else "extensions")
            / ("flyrail-" + _name(bundle.id) + ".ts")
        )
        code = (
            f"import {{ openCode, type OpenCodeInput }} from {bridge_url};\n"
            f"import config from {config_url};\n"
            "export default async function(host: OpenCodeInput) {\n"
            "  return openCode(config, host);\n}\n"
            if agent is Agent.OPENCODE
            else f"import {{ registerPi, type Pi }} from {bridge_url};\n"
            f"import config from {config_url};\n"
            "export default function(pi: Pi) { registerPi(config, pi); }\n"
        )
        for artifact in hooks:
            generated.append(
                RenderedArtifact(artifact.id, Family.HOOKS, launcher, FileContent(code.encode()))
            )
            edges.append(Dependency(artifact.id, runtime_id))
    else:
        launcher = base / "entry.mjs"
        code = (
            f"try {{ const {{ native }} = await import({bridge_url}); "
            f"const {{ default: config }} = await import({config_url}); "
            "await native(config, process.argv[2]); } "
            "catch { process.stderr.write('flyrail-hook:bootstrap\\n'); "
            "process.stdout.write('{}\\n'); }\n"
        )
        generated.append(
            RenderedArtifact(launcher_id, Family.HOOKS, launcher, FileContent(code.encode()))
        )
        edges.append(Dependency(launcher_id, runtime_id))
        node = str(runtime.node) if runtime is not None else ""
        if agent is Agent.CLAUDE and any("${" in argument for argument in (node, str(launcher))):
            raise UnsupportedTranslation(
                "hook-literal-interpolation",
                "Claude launcher paths cannot contain host placeholder syntax.",
            )
        if context.platform is Platform.WINDOWS and Path(node).suffix.lower() in {".cmd", ".bat"}:
            raise UnsupportedTranslation(
                "hook-executable",
                "Native hooks require a real executable, not a Windows command shim.",
            )
        native_entries: dict[str, list[dict[str, object]]] = {}
        for event in sorted({artifact.event for artifact in hooks}):
            name = _EVENTS[agent][list(HookEvent).index(event)]
            argv = [node, str(launcher), str(event)]
            if agent is Agent.COPILOT:
                entry = {"type": "command", "exec": node, "args": argv[1:], "timeoutSec": 310}
            elif agent is Agent.CLAUDE:
                entry = {
                    "hooks": [
                        {"type": "command", "command": node, "args": argv[1:], "timeout": 310}
                    ]
                }
            elif agent is Agent.CODEX:
                entry = {
                    "hooks": [{"type": "command", "command": _shell(argv, context), "timeout": 310}]
                }
            else:
                entry = {"command": _shell(argv, context), "timeout": 310}
            native_entries[name] = [entry]
        for artifact in hooks:
            name = _EVENTS[agent][list(HookEvent).index(artifact.event)]
            if agent is Agent.COPILOT:
                destination = native_directory / "hooks" / ("flyrail-" + _name(bundle.id) + ".json")
                content: FileContent | StructuredContent = FileContent(
                    (_json({"version": 1, "hooks": native_entries}) + "\n").encode()
                )
            else:
                destination = native_directory / (
                    "settings.json" if agent is Agent.CLAUDE else "hooks.json"
                )
                entries_for_event = native_entries[name]
                value = freeze_value(entries_for_event[0])
                member = Member(value)
                if agent is Agent.CURSOR:
                    member = Member(Scalar(str(entries_for_event[0]["command"])), ["command"])
                content = StructuredContent(
                    DocumentFormat.JSON if agent is Agent.CURSOR else DocumentFormat.JSONC,
                    Selector([Key("hooks"), Key(name), member]),
                    value,
                )
            generated.append(
                RenderedArtifact(
                    artifact.id,
                    Family.HOOKS,
                    destination,
                    content,
                    schema_requirements=(DocumentSchema(DocumentFormat.JSON, "version", Scalar(1)),)
                    if agent is Agent.CURSOR
                    else (),
                )
            )
            edges.append(Dependency(artifact.id, launcher_id, DependencyMode.STABLE_REFERENCE))
    notices = [
        Notice(
            hooks[0].id,
            NoticeKind.PREREQUISITE,
            "Requires Node 24+ for native/Pi bridges, Bun 1.4.2+ for OpenCode; "
            "host APIs documented in the hook protocol.",
            "hook-runtime-prerequisite",
        ),
        Notice(
            hooks[0].id,
            NoticeKind.ACTIVATION,
            "Installation only writes assets/configuration. Host trust, reload and actual "
            "activation remain host-controlled.",
            "hook-activation",
        ),
    ]
    return generated, list(set(edges)), notices
