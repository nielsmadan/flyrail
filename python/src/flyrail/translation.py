import hashlib
import json
from collections.abc import Iterable
from dataclasses import replace

from flyrail._hook_translation import hook_artifacts
from flyrail._mcp_translation import mcp_content
from flyrail._resource_plan import artifact_claim
from flyrail.artifacts import (
    Family,
    HookArtifact,
    InstructionArtifact,
    McpArtifact,
    NativeArtifact,
    SkillArtifact,
)
from flyrail.bundle import Bundle
from flyrail.content import FileContent, Key, SectionContent, StructuredContent
from flyrail.destinations import (
    RenderContext,
    UnsupportedTranslation,
    instruction_destination,
    mcp_destination,
)
from flyrail.rendered import (
    Dependency,
    Notice,
    NoticeKind,
    RenderedArtifact,
    RenderedBundle,
    model_value,
)
from flyrail.targets import Agent, Surface, TargetScope
from flyrail.values import semantic_bytes


def _id(*values: str) -> str:
    return hashlib.sha256(
        json.dumps(values, ensure_ascii=False, separators=(",", ":")).encode()
    ).hexdigest()


def _instruction(
    bundle: Bundle, artifact: InstructionArtifact, context: RenderContext
) -> RenderedArtifact:
    path = instruction_destination(context)
    if path is None:
        raise UnsupportedTranslation(
            "instruction-destination", "Cursor user rules have no verified filesystem preset."
        )
    marker = _id(bundle.id, artifact.id)
    if context.surface is Surface.VSCODE and context.target.scope is TargetScope.USER:
        if context.instruction_path is None:
            path = path / (marker + ".instructions.md")
        if not path.name.endswith(".instructions.md"):
            raise UnsupportedTranslation(
                "instruction-extension",
                "VS Code user instructions require an .instructions.md file.",
            )
        content: FileContent | SectionContent = FileContent(
            ('---\napplyTo: "**"\n---\n\n' + artifact.text).encode()
        )
    else:
        if path.suffix == ".mdc" or path.name.endswith(".instructions.md"):
            raise UnsupportedTranslation(
                "native-instruction-required",
                "Use native content for authored modular rule metadata.",
            )
        content = SectionContent(marker, artifact.text)
    return RenderedArtifact(artifact.id, Family.INSTRUCTIONS, path, content)


def _notices(artifact_id: str, family: Family, context: RenderContext) -> tuple[Notice, ...]:
    result = []
    agent = context.audience.agent
    if family is Family.INSTRUCTIONS:
        result.append(
            Notice(
                artifact_id,
                NoticeKind.ACTIVATION,
                "Instruction discovery and loading are controlled by the host and current session.",
                "instruction-loading",
            )
        )
        if agent is Agent.CODEX:
            result.append(
                Notice(
                    artifact_id,
                    NoticeKind.ACTIVATION,
                    "AGENTS.override.md takes precedence; instructions default to a 32 KiB limit. "
                    "Start a new session after changes and check host discovery.",
                    "codex-instruction-discovery",
                )
            )
        if context.surface is Surface.VSCODE and context.target.scope is TargetScope.USER:
            result.append(
                Notice(
                    artifact_id,
                    NoticeKind.ACTIVATION,
                    "User instructions target VS Code Agent Host; local extension-host "
                    "sessions may use different discovery settings.",
                    "vscode-agent-host-instructions",
                )
            )
    if family is Family.MCP:
        result.append(
            Notice(
                artifact_id,
                NoticeKind.ACTIVATION,
                "Registration does not grant trust, approve tools, supply credentials "
                "or prove server activation.",
                "mcp-activation",
            )
        )
        if agent is Agent.PI:
            result.append(
                Notice(
                    artifact_id,
                    NoticeKind.PREREQUISITE,
                    "Requires externally installed pi-mcp-adapter; emitted schema targets v2.32.1.",
                    "pi-mcp-adapter-v2-32-1",
                )
            )
        if context.surface is Surface.VSCODE:
            result.append(
                Notice(
                    artifact_id,
                    NoticeKind.ACTIVATION,
                    "VS Code forwards these servers to Agent Host, which does not "
                    "read this servers document directly.",
                    "vscode-mcp-forwarding",
                )
            )
    return tuple(result)


def _collision_notices(artifacts: Iterable[RenderedArtifact]) -> tuple[Notice, ...]:
    selected: list[RenderedArtifact] = []
    notices: list[Notice] = []
    for artifact in artifacts:
        for previous in selected:
            if artifact.destination != previous.destination:
                continue
            if (
                isinstance(artifact.content, StructuredContent)
                and isinstance(previous.content, StructuredContent)
                and artifact.content.format is not previous.content.format
            ):
                notices.append(
                    Notice(
                        artifact.id,
                        NoticeKind.UNSUPPORTED,
                        f"Incompatible document formats from {previous.id} and {artifact.id} "
                        f"at {artifact.destination}.",
                        "document-format-collision",
                    )
                )
                continue
            if artifact_claim(artifact).overlaps(artifact_claim(previous)) and (
                artifact_claim(artifact) != artifact_claim(previous)
                or artifact.content != previous.content
                or artifact.schema_requirements != previous.schema_requirements
                or artifact.family != previous.family
            ):
                notices.append(
                    Notice(
                        artifact.id,
                        NoticeKind.UNSUPPORTED,
                        f"Incompatible claims from {previous.id} and {artifact.id} "
                        f"at {artifact.destination}.",
                        "destination-collision",
                    )
                )
        selected.append(artifact)
    return tuple(notices)


def _copilot_precedence_notices(
    artifacts: Iterable[RenderedArtifact], contexts: Iterable[RenderContext]
) -> tuple[Notice, ...]:
    roots = {
        context.root
        for context in contexts
        if context.audience.agent is Agent.COPILOT
        and context.surface is Surface.CLI
        and context.target.scope is TargetScope.PROJECT
    }
    selected = tuple(artifacts)
    notices = []
    for root in sorted(roots):
        higher = [item for item in selected if item.destination == root / ".mcp.json"]
        lower = [item for item in selected if item.destination == root / ".github/mcp.json"]
        for artifact in lower:
            for previous in higher:
                if (
                    isinstance(artifact.content, StructuredContent)
                    and isinstance(previous.content, StructuredContent)
                    and len(artifact.content.selector.parts) == 2
                    and len(previous.content.selector.parts) == 2
                    and artifact.content.selector.parts[0] == Key("mcpServers")
                    and previous.content.selector.parts[0] == Key("mcpServers")
                    and isinstance(artifact.content.selector.parts[1], Key)
                    and isinstance(previous.content.selector.parts[1], Key)
                ):
                    if (
                        artifact.content.selector == previous.content.selector
                        and artifact.content.value != previous.content.value
                    ):
                        notices.append(
                            Notice(
                                artifact.id,
                                NoticeKind.UNSUPPORTED,
                                f"Copilot CLI selects {previous.destination} over "
                                f"{artifact.destination} for server "
                                f"{artifact.content.selector.parts[1].name}; "
                                "the selected records disagree.",
                                "copilot-mcp-precedence-collision",
                            )
                        )
                else:
                    notices.append(
                        Notice(
                            artifact.id,
                            NoticeKind.ACTIVATION,
                            f"Copilot CLI gives {previous.destination} precedence over "
                            f"{artifact.destination}; native content requires caller review "
                            "of the effective server definitions.",
                            "copilot-mcp-precedence-unverified",
                        )
                    )
    return tuple(notices)


def render(bundle: Bundle, context: RenderContext) -> RenderedBundle:
    if not isinstance(bundle, Bundle) or type(context) is not RenderContext:
        raise TypeError("render requires a Bundle snapshot and RenderContext")
    assets = {
        asset.id: (context.asset_root or context.root / ".flyrail-assets")
        / bundle.id
        / asset.id
        / hashlib.sha256(semantic_bytes(model_value(asset.content))).hexdigest()
        for asset in bundle.assets
    }
    artifacts: list[RenderedArtifact] = []
    notices: list[Notice] = []
    hook_dependencies: list[Dependency] = []
    try:
        hook_outputs, hook_dependencies, hook_notices = hook_artifacts(bundle, context, assets)
        artifacts.extend(hook_outputs)
        notices.extend(hook_notices)
    except UnsupportedTranslation as error:
        for hook in bundle.artifacts:
            if isinstance(hook, HookArtifact):
                notices.append(Notice(hook.id, NoticeKind.UNSUPPORTED, str(error), error.code))
    selected = set()
    for artifact in bundle.artifacts:
        if isinstance(artifact, NativeArtifact) and (
            artifact.audience.agent is not context.audience.agent
            or artifact.audience.scope is not context.target.scope
            or artifact.audience.surface is not context.surface
            or (
                artifact.audience.platform is not None
                and artifact.audience.platform is not context.platform
            )
        ):
            continue
        selected.add(artifact.id)
        try:
            if isinstance(artifact, SkillArtifact):
                artifacts.append(
                    RenderedArtifact(
                        artifact.id,
                        Family.SKILLS,
                        context.skills,
                        artifact.content,
                        artifact.name,
                    )
                )
            elif isinstance(artifact, InstructionArtifact):
                artifacts.append(_instruction(bundle, artifact, context))
            elif isinstance(artifact, McpArtifact):
                path = mcp_destination(context)
                if path is None:
                    raise UnsupportedTranslation(
                        "mcp-profile-path"
                        if context.surface is Surface.VSCODE
                        else "mcp-explicit-path",
                        "VS Code user MCP requires an explicit profile mcp.json path."
                        if context.surface is Surface.VSCODE
                        else "Relocated Claude user MCP requires an explicit mcp_path.",
                    )
                if context.surface is Surface.VSCODE and path.name != "mcp.json":
                    raise UnsupportedTranslation(
                        "mcp-profile-schema",
                        "VS Code routes require mcp.json with the servers schema; "
                        "use the CLI surface for Agent Host native mcpServers files.",
                    )
                artifacts.append(
                    RenderedArtifact(
                        artifact.id, Family.MCP, path, mcp_content(artifact, context, path, assets)
                    )
                )
            elif isinstance(artifact, HookArtifact):
                continue
            else:
                artifacts.append(
                    RenderedArtifact(
                        artifact.id,
                        artifact.family,
                        (context.native_root or context.root) / artifact.destination,
                        artifact.content,
                    )
                )
        except UnsupportedTranslation as error:
            notices.append(Notice(artifact.id, NoticeKind.UNSUPPORTED, str(error), error.code))
    pending = set(selected)
    while pending:
        required = {edge.required for edge in bundle.dependencies if edge.dependent in pending}
        pending = required - selected
        selected.update(required)
    for asset in bundle.assets:
        if asset.id in selected:
            artifacts.append(
                RenderedArtifact(asset.id, asset.family, assets[asset.id], asset.content)
            )
    emitted = {artifact.id for artifact in artifacts}
    for name in sorted(selected - emitted):
        if not any(
            notice.artifact_id == name and notice.kind is NoticeKind.UNSUPPORTED
            for notice in notices
        ):
            notices.append(
                Notice(
                    name,
                    NoticeKind.UNSUPPORTED,
                    "A selected dependency is unavailable for this audience.",
                    "dependency-audience",
                )
            )
    for item in artifacts:
        if item.id not in assets:
            notices.extend(_notices(item.id, item.family, context))
    notices.extend(_collision_notices(artifacts))
    notices.extend(_copilot_precedence_notices(artifacts, [context]))
    blocked = any(notice.kind is NoticeKind.UNSUPPORTED for notice in notices)
    return RenderedBundle(
        () if blocked else artifacts,
        ()
        if blocked
        else [
            *(edge for edge in bundle.dependencies if edge.dependent in emitted),
            *hook_dependencies,
        ],
        notices,
        routing_context=context.routing_context,
    )


def render_many(bundle: Bundle, contexts: Iterable[RenderContext]) -> RenderedBundle:
    selected = tuple(contexts)
    if not selected:
        raise ValueError("render_many requires at least one explicit context")
    rendered = [render(bundle, context) for context in selected]
    unique = {json.dumps(item.routing_context): item for item in rendered}
    artifacts: list[RenderedArtifact] = []
    dependencies: list[Dependency] = []
    notices: list[Notice] = []
    routes = []
    for route, item in sorted(unique.items()):
        routes.append((_id(route), route))
        names = {artifact.id: _id(route, artifact.id) for artifact in item.artifacts}
        artifacts.extend(replace(artifact, id=names[artifact.id]) for artifact in item.artifacts)
        dependencies.extend(
            Dependency(names[edge.dependent], names[edge.required], edge.mode)
            for edge in item.dependencies
        )
        notices.extend(
            replace(notice, artifact_id=_id(route, notice.artifact_id)) for notice in item.notices
        )
    notices.extend(_collision_notices(artifacts))
    notices.extend(_copilot_precedence_notices(artifacts, selected))
    blocked = any(notice.kind is NoticeKind.UNSUPPORTED for notice in notices)
    return RenderedBundle(
        () if blocked else artifacts,
        () if blocked else dependencies,
        notices,
        routing_context=routes,
    )
