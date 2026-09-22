import re
from pathlib import Path

from flyrail.artifacts import AssetRef, Command, EnvRef, HttpTransport, McpArtifact
from flyrail.content import DocumentFormat, Key, Selector, StructuredContent
from flyrail.destinations import RenderContext, UnsupportedTranslation
from flyrail.targets import Agent, Platform, Surface
from flyrail.values import freeze_value


def _literal(value: str, context: RenderContext, field: str) -> str:
    agent = context.audience.agent
    patterns = (
        ("{env:", "{file:")
        if agent is Agent.OPENCODE
        else ("${", "$env:", "{env:")
        if agent is Agent.PI
        else ()
        if agent is Agent.CODEX
        else ("${",)
    )
    copilot_cli = agent is Agent.COPILOT and context.surface is Surface.CLI
    if any(pattern in value for pattern in patterns) or (
        copilot_cli and field == "env" and re.search(r"\$[A-Za-z_]", value) is not None
    ):
        if copilot_cli and field != "env":
            raise UnsupportedTranslation(
                "literal-interpolation-unverified",
                f"Literal {field} substitution-like syntax is unverified for Copilot CLI.",
            )
        raise UnsupportedTranslation(
            "literal-interpolation",
            f"{field} contains host interpolation syntax; use native content.",
        )
    return value


def _reference(value: EnvRef, context: RenderContext, field: str) -> str:
    agent = context.audience.agent
    if agent is Agent.CLAUDE or (
        agent is Agent.COPILOT and context.surface is Surface.CLI and field == "env"
    ):
        return "${" + value.name + "}"
    if agent is Agent.OPENCODE:
        return "{env:" + value.name + "}"
    if agent is Agent.CURSOR or context.surface is Surface.VSCODE:
        return "${env:" + value.name + "}"
    raise UnsupportedTranslation(
        "portable-env-reference-unverified",
        "This host cannot represent this environment reference.",
    )


def _asset(value: str | AssetRef, assets: dict[str, Path]) -> str:
    if isinstance(value, AssetRef):
        return str(assets[value.asset_id] / value.path if value.path else assets[value.asset_id])
    return value


def _stdio(command: Command, context: RenderContext, assets: dict[str, Path]) -> dict[str, object]:
    agent = context.audience.agent
    names = [key.casefold() for key, _ in command.env]
    if context.platform is Platform.WINDOWS and len(set(names)) != len(names):
        raise UnsupportedTranslation(
            "windows-env-collision",
            "Environment names collide under Windows case-insensitive lookup.",
        )
    argv = [_asset(value, assets) for value in command.argv]
    for index, argument in enumerate(argv):
        if agent is not Agent.PI or index:
            _literal(argument, context, "argv")
    env: dict[str, str] = {}
    forwarded: list[str] = []
    for key, value in command.env:
        if isinstance(value, EnvRef):
            if agent is Agent.CODEX:
                if key != value.name:
                    raise UnsupportedTranslation(
                        "environment-alias",
                        "Codex env_vars forwards a variable under its own name.",
                    )
                forwarded.append(key)
            else:
                env[key] = _reference(value, context, "env")
        else:
            env[key] = value if agent is Agent.PI else _literal(value, context, "env")
    record: dict[str, object] = (
        {"type": "local", "command": argv, "environment": env}
        if agent is Agent.OPENCODE
        else {"command": argv[0], "args": argv[1:], "env": env}
    )
    if agent is Agent.CURSOR or context.surface is Surface.VSCODE:
        record["type"] = "stdio"
    if agent is Agent.PI:
        record["literalEnv"] = True
    if forwarded:
        record["env_vars"] = forwarded
    if command.cwd is not None:
        if agent in {Agent.CLAUDE, Agent.CURSOR}:
            raise UnsupportedTranslation("command-cwd", "The selected MCP schema cannot honor cwd.")
        cwd = (
            _asset(command.cwd, assets)
            if isinstance(command.cwd, AssetRef)
            else str((context.execution_root or context.root) / command.cwd)
        )
        record["cwd"] = _literal(cwd, context, "cwd")
    return record


def _http(transport: HttpTransport, context: RenderContext) -> dict[str, object]:
    agent = context.audience.agent
    result: dict[str, object] = {"url": _literal(transport.url, context, "url")}
    if agent is Agent.OPENCODE:
        result["type"] = "remote"
    elif agent not in {Agent.CODEX, Agent.PI}:
        result["type"] = "http"
    if transport.bearer_token is not None:
        if agent is Agent.CODEX:
            result["bearer_token_env_var"] = transport.bearer_token.name
        elif agent is Agent.PI:
            result.update(auth="bearer", bearerTokenEnv=transport.bearer_token.name)
        elif agent is Agent.COPILOT and context.surface is Surface.CLI:
            raise UnsupportedTranslation(
                "portable-auth-env-unverified",
                "Copilot CLI bearer environment syntax is unverified.",
            )
        else:
            result["headers"] = {
                "Authorization": "Bearer " + _reference(transport.bearer_token, context, "headers")
            }
    return result


def mcp_content(
    artifact: McpArtifact, context: RenderContext, path: Path, assets: dict[str, Path]
) -> StructuredContent:
    agent = context.audience.agent
    if agent is Agent.CLAUDE and artifact.name in {"workspace", "claude-in-chrome", "computer-use"}:
        raise UnsupportedTranslation(
            "reserved-server-name", "Claude reserves this MCP server name."
        )
    record = (
        _stdio(artifact.transport, context, assets)
        if isinstance(artifact.transport, Command)
        else _http(artifact.transport, context)
    )
    format = (
        DocumentFormat.TOML
        if agent is Agent.CODEX
        else DocumentFormat.JSONC
        if agent is Agent.OPENCODE or context.surface is Surface.VSCODE or path.suffix == ".jsonc"
        else DocumentFormat.JSON
    )
    key = (
        "mcp_servers"
        if agent is Agent.CODEX
        else "mcp"
        if agent is Agent.OPENCODE
        else "servers"
        if context.surface is Surface.VSCODE
        else "mcpServers"
    )
    return StructuredContent(format, Selector([Key(key), Key(artifact.name)]), freeze_value(record))
