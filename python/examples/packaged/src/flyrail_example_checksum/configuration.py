import argparse
import json
import os
import sys
from dataclasses import asdict
from importlib import resources
from pathlib import Path

from flyrail import (
    Agent,
    AssetRef,
    Bundle,
    BundleEntry,
    BundleIdentity,
    Command,
    Dependency,
    EnvRef,
    Family,
    HookArtifact,
    HookEvent,
    HookOutcome,
    HookRuntime,
    HookShell,
    HttpTransport,
    InstallationObservation,
    InstallationResult,
    InstructionArtifact,
    McpArtifact,
    Notice,
    NoticeKind,
    OperationStatus,
    Platform,
    RenderContext,
    SupportAsset,
    Target,
    TreeContent,
    inspect_installation,
    remove,
    render,
    sync,
)

BUNDLE_ID = "example-checksum-config"
GUIDANCE = "Use flyrail-checksum digest FILE to verify downloaded files.\n"


def configure_parser(parser: argparse.ArgumentParser) -> None:
    actions = parser.add_subparsers(dest="action", required=True)
    for action in ("status", "update", "uninstall"):
        command = actions.add_parser(action)
        command.add_argument("--project", type=Path, required=True)
        command.add_argument("--agent", choices=[str(agent) for agent in Agent], required=True)
        if action != "uninstall":
            command.add_argument("--node", type=Path, help="explicit Node 24+ for native hosts")
            command.add_argument("--mcp-command", required=True, help="installed stdio MCP server")
            command.add_argument("--mcp-arg", action="append", default=[])
            command.add_argument("--mcp-url", required=True, help="HTTP MCP endpoint")
            command.add_argument("--guidance", default=GUIDANCE)
            command.add_argument("--asset-root", type=Path)
        if action != "status":
            command.add_argument("--replace-modified", action="store_true")


def bundled_configuration(guidance: str, argv: list[str], url: str) -> Bundle:
    skills = Bundle.from_package("flyrail_example_checksum")
    assets = resources.files("flyrail_example_checksum").joinpath("assets")
    return Bundle.from_artifacts(
        BundleIdentity(BUNDLE_ID, skills.version),
        [
            *skills.artifacts,
            InstructionArtifact("guidance", guidance),
            McpArtifact(
                "local",
                "checksum-local",
                Command(argv, {"CHECKSUM_MCP_TOKEN": EnvRef("CHECKSUM_MCP_TOKEN")}),
            ),
            McpArtifact(
                "remote", "checksum-remote", HttpTransport(url, EnvRef("CHECKSUM_HTTP_TOKEN"))
            ),
            HookArtifact(
                "reminder",
                HookEvent.BEFORE_TOOL,
                Command([sys.executable, AssetRef("reminder-assets", "reminder.py")]),
                outcomes=[HookOutcome.CONTEXT],
            ),
        ],
        assets=[
            SupportAsset(
                "reminder-assets",
                Family.HOOKS,
                TreeContent(
                    [
                        BundleEntry(name, assets.joinpath(name).read_bytes())
                        for name in ("reminder.py", "reminder.txt")
                    ]
                ),
            )
        ],
        dependencies=[Dependency("reminder", "reminder-assets")],
    )


def main(arguments: argparse.Namespace, app_version: str) -> int:
    root = arguments.project.resolve()
    platform = (
        Platform.WINDOWS
        if os.name == "nt"
        else Platform.MACOS
        if sys.platform == "darwin"
        else Platform.LINUX
    )
    context = RenderContext(Target.project(arguments.agent, root), platform)
    target = context.installation(root / ".cache" / f"{BUNDLE_ID}-{arguments.agent}")
    result: InstallationResult | None = None
    observation: InstallationObservation
    bundle_version: str | None = None
    matches: bool | None = None
    notices: tuple[Notice, ...] = ()
    if arguments.action == "uninstall":
        result = remove(BUNDLE_ID, target, replace_modified=arguments.replace_modified)
        observation = result.observation
    else:
        bundle = bundled_configuration(
            arguments.guidance, [arguments.mcp_command, *arguments.mcp_arg], arguments.mcp_url
        )
        bundle_version = bundle.version
        context = RenderContext(
            context.target,
            platform,
            asset_root=arguments.asset_root.resolve() if arguments.asset_root else None,
            hook_runtime=HookRuntime(
                arguments.node.resolve() if arguments.node else None,
                HookShell.CMD if os.name == "nt" else HookShell.POSIX,
            ),
        )
        target = context.installation(target.index_root)
        desired = render(bundle, context)
        if arguments.action == "update":
            result = sync(bundle, desired, target, replace_modified=arguments.replace_modified)
            observation = result.observation
        else:
            observation = inspect_installation(BUNDLE_ID, target)
        matches = observation.matches(bundle, desired)
        notices = desired.notices
    print(
        json.dumps(
            {
                "app_version": app_version,
                "bundle_id": BUNDLE_ID,
                "bundle_version": bundle_version,
                "agent": arguments.agent,
                "project": str(root),
                "configured_current": matches,
                "recorded_current": observation.is_current,
                "activation": "not-checked",
                "prerequisites": [
                    "Provide the selected stdio MCP server and HTTP endpoint.",
                    "Provide CHECKSUM_MCP_TOKEN and CHECKSUM_HTTP_TOKEN in the agent environment.",
                    "Keep this application's Python interpreter available for the bundled hook.",
                    *[
                        notice.message
                        for notice in notices
                        if notice.kind is NoticeKind.PREREQUISITE
                    ],
                ]
                if bundle_version
                else [],
                "notices": [asdict(notice) for notice in notices],
                "observation": asdict(observation),
                "status": result.status if result else None,
                "error": asdict(result.error) if result and result.error else None,
                "resources": [asdict(resource) for resource in result.resources] if result else [],
            },
            default=str,
        )
    )
    if result is None:
        return int(matches is not True)
    return int(
        result.status not in {OperationStatus.APPLIED, OperationStatus.UNCHANGED}
        or result.error is not None
        or observation.error is not None
        or any(
            resource.error is not None or resource.recovery_paths for resource in result.resources
        )
        or (arguments.action == "update" and matches is not True)
    )
