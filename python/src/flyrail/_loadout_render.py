import hashlib
import json
import re
from dataclasses import dataclass, replace
from pathlib import Path

from flyrail._hook_translation import _EVENTS, hook_artifacts
from flyrail._loadout_manifest import (
    Inputs,
    LoadoutFailure,
    LoadoutSelector,
    LoadoutSource,
    Manifest,
    strings,
)
from flyrail._mcp_translation import _asset, _literal
from flyrail._validation import portable_path_key
from flyrail.artifacts import (
    Artifact,
    EnvRef,
    Family,
    HookArtifact,
    HttpTransport,
    InstructionArtifact,
    McpArtifact,
    NativeArtifact,
    SkillArtifact,
)
from flyrail.bundle import Bundle
from flyrail.content import (
    DocumentFormat,
    FileContent,
    Key,
    Member,
    Selector,
    StructuredContent,
    TreeContent,
)
from flyrail.destinations import RenderContext, UnsupportedTranslation
from flyrail.editors import _native as plain_value
from flyrail.hooks import HookRuntime
from flyrail.rendered import (
    Dependency,
    DependencyMode,
    Notice,
    RenderedArtifact,
    RenderedBundle,
    model_value,
)
from flyrail.targets import Agent, Platform, Surface, Target, TargetScope
from flyrail.values import Scalar, freeze_value, semantic_bytes

_MODULE_HARNESSES = {
    Agent.OPENCODE: ("plugins", ".config/opencode"),
    Agent.PI: ("extensions", ".pi/agent"),
}
_HOOK_HARNESSES = {Agent.CLAUDE, Agent.CODEX, Agent.CURSOR, *_MODULE_HARNESSES}
_NATIVE_PREFIX = {
    agent: f"{directory}/{module}/" for agent, (module, directory) in _MODULE_HARNESSES.items()
}
_FILTERED_DIRECTORIES = {
    ".git",
    ".hg",
    "node_modules",
    ".venv",
    "__pycache__",
    ".mypy_cache",
    ".ruff_cache",
    ".pytest_cache",
}
_FILTERED_FILES = {".DS_Store", ".loadout-bundle.toml"}
_FILTERED_SUFFIXES = {".pyc", ".pyo", ".swp"}


@dataclass(frozen=True, slots=True)
class LoadoutImpact:
    category: str
    path: Path
    harnesses: tuple[Agent, ...]
    shared: bool


def _id(*values: str) -> str:
    return hashlib.sha256("\0".join(values).encode()).hexdigest()


def _structured(
    keys: tuple[str, ...], value: object, *, member: str | None = None
) -> StructuredContent:
    parts: list[Key | Member] = [Key(key) for key in keys]
    if member is not None:
        parts.append(Member(Scalar(member)))
    return StructuredContent(DocumentFormat.TOML, Selector(parts), freeze_value(value))


def required_slices(artifact: Artifact, agents: tuple[Agent, ...]) -> tuple[tuple[Agent, str], ...]:
    if isinstance(artifact, NativeArtifact):
        return ((artifact.audience.agent, "module-config"),)
    if isinstance(artifact, HookArtifact):
        return tuple(
            (agent, "module-config" if agent in _MODULE_HARNESSES else "hooks") for agent in agents
        )
    category = (
        "skills"
        if isinstance(artifact, SkillArtifact)
        else "instructions"
        if isinstance(artifact, InstructionArtifact)
        else "mcp"
    )
    return tuple((agent, category) for agent in agents)


def hooks_supported(bundle: Bundle, agents: tuple[Agent, ...]) -> bool:
    return all(agent in _HOOK_HARNESSES for agent in agents) or not any(
        isinstance(artifact, HookArtifact) for artifact in bundle.artifacts
    )


def _filtered(path: Path) -> bool:
    return (
        bool(_FILTERED_DIRECTORIES.intersection(path.parts))
        or path.name in _FILTERED_FILES
        or path.suffix in _FILTERED_SUFFIXES
    )


def _permission_overlaps(rule: str, server: str, *, pi: bool) -> bool:
    other, separator, tool = rule.partition("/")
    if not separator or not other or not tool:
        raise ValueError("MCP permissions must use server/tool entries")
    if "*" in other or "?" in other:
        return False
    allowed = r"[^a-zA-Z0-9_*?]" if pi else r"[^a-zA-Z0-9_*?-]"
    marker = "__" if pi else "_"
    namespace = re.sub(allowed, "_", server) + marker
    other_namespace = re.sub(allowed, "_", other) + marker
    if other_namespace == namespace:
        return other != server
    key = other_namespace + re.sub(allowed, "_", tool)
    for expected, actual in zip(namespace, key, strict=False):
        if actual == "*":
            return True
        if actual not in (expected, "?"):
            return False
    return len(key) > len(namespace)


class SourceRender:
    def __init__(
        self,
        bundle: Bundle,
        manifest: Manifest,
        source: LoadoutSource,
        agents: tuple[Agent, ...],
        accept_shared_scope: bool,
        home: Path,
        platform: Platform,
        inputs: Inputs,
        hook_runtime: HookRuntime | None,
        execution_root: Path | None,
    ) -> None:
        self.bundle, self.manifest, self.source, self.agents = bundle, manifest, source, agents
        self.accept_shared_scope = accept_shared_scope
        self.inputs = inputs
        self.contexts = {
            agent: RenderContext(
                Target.user(agent, home=home),
                platform,
                asset_root=source.path / ".flyrail-assets",
                hook_runtime=hook_runtime,
                execution_root=execution_root,
            )
            for agent in agents
        }
        self.artifacts: list[RenderedArtifact] = []
        self.dependencies: set[Dependency] = set()
        self.notices: list[Notice] = []
        self.impacts: list[LoadoutImpact] = []
        self.assets = {
            asset.id: source.path
            / ".flyrail-assets"
            / bundle.id
            / asset.id
            / hashlib.sha256(semantic_bytes(model_value(asset.content))).hexdigest()
            for asset in bundle.assets
        }

    def fail(self, code: str, message: str, path: Path | None = None) -> LoadoutFailure:
        return LoadoutFailure(code, message, path or self.manifest.chain[0])

    def _gate(self) -> None:
        active = {harness.agent: harness.slices for harness in self.manifest.harnesses}
        for agent in self.agents:
            if agent is Agent.COPILOT:
                raise self.fail("unsupported-harness", "Copilot is outside the Loadout harness set")
            if agent not in active:
                raise self.fail("disabled-harness", f"{agent} is absent from the active profile")
        if not hooks_supported(self.bundle, self.agents):
            raise self.fail(
                "unsupported-hook", "This selection includes an unverified Loadout hook harness"
            )
        for artifact in self.bundle.artifacts:
            for agent, category in required_slices(artifact, self.agents):
                if category not in self.source.use:
                    raise self.fail(
                        "source-use",
                        f"Source {self.source.name} does not offer {category}",
                        self.source.declared_in,
                    )
                if agent not in self.agents:
                    raise self.fail(
                        "native-audience",
                        f"Native artifact {artifact.id} targets {agent} outside this selection",
                    )
                if category not in active[agent]:
                    raise self.fail(
                        "disabled-slice", f"{agent}.{category} is disabled in this profile"
                    )
                if (str(agent), category) in self.manifest.custom_routes:
                    raise self.fail(
                        "unsupported-slice", f"{agent}.{category} uses a custom legacy renderer"
                    )

    def _destination(self, path: Path) -> Path:
        if path.resolve() != path or not path.is_relative_to(self.source.path):
            raise self.fail(
                "escaped-path",
                f"Source destination must stay within the selected source without aliases: {path}",
                path,
            )
        return path

    def _impact(
        self,
        category: str,
        path: Path,
        agents: tuple[Agent, ...],
        *,
        shared: bool = False,
    ) -> None:
        broader = shared or bool(set(agents) - set(self.agents))
        self.impacts.append(LoadoutImpact(category, path, agents, broader))

    def _read_source_file(self, source: LoadoutSource, relative: str) -> bytes | None:
        path = source.path / relative
        seen, data = self.inputs.read(path, "invalid-source", within=source.path)
        if seen.identity is not None and data is None:
            raise self.fail("source-collision", f"Expected a source file at {path}", path)
        return data

    def _write(
        self,
        identifier: str,
        family: Family,
        path: Path,
        content: FileContent | TreeContent | StructuredContent,
        *,
        subtree: str | None = None,
    ) -> None:
        self.artifacts.append(RenderedArtifact(identifier, family, path, content, subtree))

    def _skill(self, artifact: SkillArtifact) -> None:
        root = self._destination(self.source.path / "skills")
        if _filtered(Path(artifact.name)) or any(
            _filtered(Path(entry.path)) for entry in artifact.content.entries
        ):
            raise self.fail("skill-filter", f"Loadout excludes part of skill {artifact.name}", root)
        for source in (item for item in self.manifest.sources if "skills" in item.use):
            name = artifact.name
            paths = [f"skills/{name}.local/SKILL.md"]
            if source != self.source:
                paths.append(f"skills/{name}/SKILL.md")
            if name in dict(source.overrides).get("skills", ()):
                raise self.fail(
                    "source-collision",
                    f"An override controls skill {name} in {source.name}",
                    source.declared_in,
                )
            for relative in paths:
                if self._read_source_file(source, relative) is not None:
                    raise self.fail(
                        "source-collision",
                        f"Skill {name} also resolves from {source.name}",
                        source.path / relative,
                    )
        active = self.manifest.chain[0]
        policy = next((item for item in self.manifest.policies if item.name == artifact.name), None)
        declarations = (active,) if policy is None else policy.declarations
        if len(declarations) != 1:
            raise self.fail(
                "skill-policy", f"Split declarations control skill {artifact.name}", active
            )
        manifest = declarations[0]
        self._impact("skills", manifest, self.agents, shared=manifest != active)
        policy_id = _id("skill-policy", artifact.id)
        self._write(
            policy_id,
            Family.SKILLS,
            manifest,
            _structured(
                ("skills", artifact.name),
                {"agents": [str(agent) for agent in self.agents], "render": False},
            ),
        )
        self._write(artifact.id, Family.SKILLS, root, artifact.content, subtree=artifact.name)
        self.dependencies.add(Dependency(artifact.id, policy_id))

    def _routes(self, category: str) -> tuple[tuple[LoadoutSelector, ...], ...]:
        all_routes: dict[tuple[Path, tuple[str, ...]], list[LoadoutSelector]] = {}
        for route in self.manifest.selectors:
            if route.category == category:
                all_routes.setdefault((route.manifest, route.key), []).append(route)
        return tuple(
            tuple(routes)
            for routes in all_routes.values()
            if any(route.harness in self.agents for route in routes)
        )

    def _entry(
        self, routes: tuple[LoadoutSelector, ...], name: str, family: Family, required: str
    ) -> str:
        route = routes[0]
        path, key = route.manifest, route.key
        self._impact(
            route.category,
            path,
            tuple(sorted({item.harness for item in routes})),
            shared=path != self.manifest.chain[0]
            or any(item.destinations > 1 or item.all_profiles for item in routes),
        )
        if not route.array:
            raise self.fail(
                "selector-not-array",
                f"Write {'.'.join(key)} as an array before selecting another fragment",
                path,
            )
        entry = f"{self.source.name}/{name}"
        if any(entry in dict(item.substitutes) for item in routes):
            raise self.fail(
                "selector-shadowed", f"A substitution changes the meaning of {entry}", path
            )
        if any(
            dict(item.substitutes).get(existing, existing) in {name, entry}
            for item in routes
            for existing in item.entries
            if existing != entry
        ):
            raise self.fail(
                "selector-shadowed", f"Another selector entry already names {entry}", path
            )
        identifier = _id("selector", str(path), *key, entry)
        self._write(identifier, family, path, _structured(key, entry, member=entry))
        self.dependencies.add(Dependency(identifier, required, DependencyMode.STABLE_REFERENCE))
        return identifier

    def _check_unselected(
        self, category: str, name: str, groups: tuple[tuple[LoadoutSelector, ...], ...]
    ) -> None:
        target = f"{self.source.name}/{name}"
        selected = {route for routes in groups for route in routes}
        for route in self.manifest.selectors:
            swaps = dict(route.substitutes)
            if (
                route not in selected
                and route.category == category
                and any(swaps.get(item, item) in {name, target} for item in route.entries)
            ):
                raise self.fail(
                    "selector-shadowed",
                    f"An unselected {category} target already refers to {target}",
                    route.manifest,
                )

    def _instruction(self, artifact: InstructionArtifact) -> None:
        name = "flyrail-" + _id("instructions", self.bundle.id, artifact.id)
        relative = f"instructions/{name}.md"
        path = self._destination(self.source.path / relative)
        for source in self.manifest.sources:
            if (
                source != self.source
                and "instructions" in source.use
                and self._read_source_file(source, relative) is not None
            ):
                raise self.fail(
                    "source-collision",
                    f"Instruction fragment {name} already exists in {source.name}",
                    source.path / relative,
                )
        selected_routes = self._routes("instructions")
        self._check_unselected("instructions", name, selected_routes)
        self._write(artifact.id, Family.INSTRUCTIONS, path, FileContent(artifact.text.encode()))
        for routes in selected_routes:
            self._entry(routes, name, Family.INSTRUCTIONS, artifact.id)

    def _mcp(self, artifact: McpArtifact, existing: dict[LoadoutSource, dict[str, object]]) -> None:
        affected = tuple(item.agent for item in self.manifest.harnesses if "mcp" in item.slices)
        path = self._destination(self.source.path / "mcp.toml")
        self._impact("mcp", path, affected)
        if any((str(agent), "mcp") in self.manifest.custom_routes for agent in affected):
            raise self.fail("unsupported-slice", "MCP also reaches a custom legacy renderer", path)
        name = artifact.name
        for source, table in existing.items():
            for other in table:
                same = name == other and source == self.source
                pi_alias = Agent.PI in affected and name.replace("-", "_") == other.replace(
                    "-", "_"
                )
                if not same and (portable_path_key(name) == portable_path_key(other) or pi_alias):
                    raise self.fail(
                        "source-collision",
                        f"MCP name {name} collides with {other} in {source.name}",
                        source.path / "mcp.toml",
                    )
        transport = artifact.transport
        if isinstance(transport, HttpTransport):
            server: dict[str, object] = {"transport": "http", "url": transport.url}
            if transport.bearer_token:
                server["auth_env_var"] = transport.bearer_token.name
            for agent in affected:
                _literal(
                    transport.url, self.contexts.get(agent) or self._extra_context(agent), "url"
                )
        else:
            if transport.cwd is not None:
                raise self.fail(
                    "mcp-cwd", f"Loadout cannot preserve the cwd of MCP server {name}", path
                )
            if any(isinstance(value, EnvRef) for _, value in transport.env):
                raise self.fail(
                    "mcp-env-reference",
                    f"Loadout cannot preserve stdio environment references for {name}",
                    path,
                )
            argv = [_asset(arg, self.assets) for arg in transport.argv]
            env = {key: value for key, value in transport.env if isinstance(value, str)}
            for agent in affected:
                context = self.contexts.get(agent) or self._extra_context(agent)
                for arg in argv:
                    _literal(arg, context, "argv")
                for value in env.values():
                    _literal(value, context, "env")
            if Agent.PI in affected and any(
                value.startswith("!") or "$env:" in value for value in (*argv, *env.values())
            ):
                raise self.fail(
                    "mcp-literal",
                    f"Pi could interpret a literal MCP argument or environment value for {name}",
                    path,
                )
            server = {"transport": "stdio", "command": argv[0], "args": argv[1:], "env": env}
        self._write(artifact.id, Family.MCP, path, _structured((name,), server))

    def _extra_context(self, agent: Agent) -> RenderContext:
        first = self.contexts[self.agents[0]]
        return replace(first, target=Target.user(agent, home=first.root))

    def _check_server_table(self) -> dict[LoadoutSource, dict[str, object]]:
        tables = {}
        for source in self.manifest.sources:
            if "mcp" not in source.use:
                continue
            path = source.path / "mcp.toml"
            tables[source] = (
                self.inputs.table(path, "invalid-mcp", optional=True, within=source.path) or {}
            )
            for name, entry in tables[source].items():
                if (
                    not isinstance(entry, dict)
                    or set(entry)
                    - {"transport", "command", "args", "env", "url", "auth_env_var", "headers"}
                    or entry.get("transport") not in ("stdio", "http")
                ):
                    raise self.fail("invalid-mcp", f"Invalid Loadout MCP definition {name!r}", path)
                field = "command" if entry["transport"] == "stdio" else "url"
                if not isinstance(entry.get(field), str) or not entry[field]:
                    raise self.fail(
                        "invalid-mcp", f"MCP definition {name!r} requires {field}", path
                    )
                headers, env = entry.get("headers", {}), entry.get("env", {})
                if (
                    not isinstance(headers, dict)
                    or not isinstance(env, dict)
                    or not all(isinstance(value, str) for value in env.values())
                    or (headers and entry["transport"] != "http")
                    or any(
                        not isinstance(value, str)
                        or "$" in value
                        or "{" in value
                        or value.startswith("!")
                        for value in headers.values()
                    )
                    or ("auth_env_var" in entry and not isinstance(entry["auth_env_var"], str))
                    or (
                        entry.get("auth_env_var")
                        and any(key.lower() == "authorization" for key in headers)
                    )
                ):
                    raise self.fail(
                        "invalid-mcp",
                        f"Invalid headers or environment for MCP definition {name!r}",
                        path,
                    )
                strings(entry.get("args", []), path, f"{name}.args", "invalid-mcp")
        return tables

    def _check_mcp_permissions(self, servers: tuple[str, ...]) -> None:
        scoped = tuple(
            item.agent
            for item in self.manifest.harnesses
            if item.agent in {Agent.OPENCODE, Agent.PI} and "permissions" in item.slices
        )
        if not scoped:
            return
        for source in self.manifest.sources:
            if "permissions" not in source.use:
                continue
            path = source.path / "permissions.toml"
            data = (
                self.inputs.table(path, "invalid-permissions", optional=True, within=source.path)
                or {}
            )
            rules = data.get("mcp", {})
            if not isinstance(rules, dict) or set(rules) - {"allow", "deny", "ask"}:
                raise self.fail(
                    "invalid-permissions", "Expected MCP permission decision lists", path
                )
            for decision in ("allow", "deny", "ask"):
                for rule in strings(
                    rules.get(decision, []), path, f"mcp.{decision}", "invalid-permissions"
                ):
                    for agent in scoped:
                        try:
                            overlaps = any(
                                _permission_overlaps(rule, name, pi=agent is Agent.PI)
                                for name in servers
                            )
                        except ValueError as error:
                            raise self.fail("invalid-permissions", str(error), path) from error
                        if overlaps:
                            raise self.fail(
                                "mcp-permission-overlap",
                                f"MCP permission {rule!r} overlaps a proposed {agent} namespace",
                                path,
                            )

    def _module(self, agent: Agent, relative: str) -> Path:
        tail = f"{agent}/{relative}"
        for source in self.manifest.sources:
            if "module-config" not in source.use:
                continue
            if tail in dict(source.overrides).get("module-config", ()):
                raise self.fail(
                    "source-collision", f"An override controls {tail}", source.declared_in
                )
            if (
                source != self.source
                and self._read_source_file(source, "module-config/" + tail) is not None
            ):
                raise self.fail(
                    "source-collision",
                    f"Module {tail} already exists in {source.name}",
                    source.path,
                )
        return self._destination(self.source.path / "module-config" / tail)

    def _native(self, artifact: NativeArtifact) -> None:
        audience = artifact.audience
        prefix = _NATIVE_PREFIX.get(audience.agent)
        relative = artifact.destination.removeprefix(prefix) if prefix else ""
        if (
            not prefix
            or not artifact.destination.startswith(prefix)
            or "/" in relative
            or Path(relative).suffix not in {".ts", ".js", ".mjs"}
            or relative in {"loadout-hooks.js", "loadout-hooks.ts"}
            or audience.scope is not TargetScope.USER
            or audience.surface is not Surface.CLI
            or (
                audience.platform is not None
                and audience.platform is not self.contexts[audience.agent].platform
            )
            or not isinstance(artifact.content, FileContent)
            or _filtered(Path(relative))
        ):
            raise self.fail(
                "native-path", f"No Loadout source mapping for native artifact {artifact.id}"
            )
        module = _MODULE_HARNESSES[audience.agent][0]
        path = self._module(audience.agent, f"{module}/{relative}")
        self._impact("module-config", path, (audience.agent,))
        self._write(artifact.id, artifact.family, path, artifact.content)

    def _hooks(self) -> dict[str, list[str]]:
        identities: dict[str, list[str]] = {}
        for agent, context in self.contexts.items():
            outputs, edges, notices = hook_artifacts(
                self.bundle, context, self.assets, revision_launcher=True
            )
            authored = [
                item
                for item in outputs
                if any(
                    isinstance(a, HookArtifact) and a.id == item.id for a in self.bundle.artifacts
                )
            ]
            authored_ids = {item.id for item in authored}
            support = [item for item in outputs if item.id not in authored_ids]
            for item in support:
                self.artifacts.append(
                    replace(item, destination=self._destination(item.destination))
                )
            self.dependencies.update(
                edge
                for edge in edges
                if edge.dependent not in authored_ids and edge.required not in authored_ids
            )
            self.notices.extend(notices)
            if agent in _MODULE_HARNESSES:
                item = authored[0]
                relative = _MODULE_HARNESSES[agent][0] + "/" + item.destination.name
                path = self._module(agent, relative)
                self._impact("module-config", path, (agent,))
                identifier = _id("hook-module", self.bundle.id, agent)
                self.artifacts.append(replace(item, id=identifier, destination=path))
            else:
                identifier = self._hook_fragment(agent, authored)
            for item in authored:
                identities.setdefault(item.id, []).append(identifier)
            self.dependencies.update(
                replace(edge, dependent=identifier)
                for edge in edges
                if edge.dependent in authored_ids
            )
        return identities

    def _hook_fragment(self, agent: Agent, entries: list[RenderedArtifact]) -> str:
        name = "flyrail-" + _id("hooks", self.bundle.id, agent)
        relative = f"hooks/{name}.json"
        path = self._destination(self.source.path / relative)
        for source in self.manifest.sources:
            if (
                source != self.source
                and "hooks" in source.use
                and self._read_source_file(source, relative) is not None
            ):
                raise self.fail(
                    "source-collision",
                    f"Hook fragment {name} already exists in {source.name}",
                    source.path / relative,
                )
        values: dict[str, list[object]] = {}
        for item in entries:
            content = item.content
            if not isinstance(content, StructuredContent) or not isinstance(
                event := content.selector.parts[1], Key
            ):
                raise self.fail("hook-source", f"Cannot translate hook output {item.id}", path)
            raw = plain_value(content.value)
            if agent is Agent.CURSOR:
                if not isinstance(raw, dict):
                    raise self.fail("hook-source", f"Expected an object for hook {item.id}", path)
                raw = {"hooks": [{"type": "command", **raw}]}
                values[_EVENTS[Agent.CODEX][_EVENTS[Agent.CURSOR].index(event.name)]] = [raw]
            else:
                values[event.name] = [raw]
        identifier = _id("hook-fragment", self.bundle.id, agent)
        self._write(
            identifier,
            Family.HOOKS,
            path,
            FileContent(
                (
                    json.dumps(values, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
                    + "\n"
                ).encode()
            ),
        )
        selected = tuple(
            routes
            for routes in self._routes("hooks")
            if any(route.harness is agent for route in routes)
        )
        self._check_unselected("hooks", name, selected)
        for routes in selected:
            if {route.harness for route in routes} != {agent}:
                raise self.fail(
                    "hook-shared-selector",
                    "A shared hook selector cannot preserve separate harness ABIs. "
                    "Declare separate arrays.",
                    routes[0].manifest,
                )
            self._entry(routes, name, Family.HOOKS, identifier)
        return identifier

    def render(self) -> RenderedBundle:
        self._gate()
        servers = tuple(
            item.name for item in self.bundle.artifacts if isinstance(item, McpArtifact)
        )
        if len(servers) != len(set(servers)):
            raise self.fail("source-collision", "A bundle may declare each MCP server once")
        existing = self._check_server_table() if servers else {}
        if servers:
            self._check_mcp_permissions(servers)
        aliases: dict[str, list[str]] = {}
        for artifact in self.bundle.artifacts:
            try:
                if isinstance(artifact, SkillArtifact):
                    self._skill(artifact)
                elif isinstance(artifact, InstructionArtifact):
                    self._instruction(artifact)
                elif isinstance(artifact, McpArtifact):
                    self._mcp(artifact, existing)
                elif isinstance(artifact, NativeArtifact):
                    self._native(artifact)
            except UnsupportedTranslation as error:
                raise self.fail(error.code, str(error)) from error
        if any(isinstance(a, HookArtifact) for a in self.bundle.artifacts):
            try:
                aliases = self._hooks()
            except UnsupportedTranslation as error:
                raise self.fail(error.code, str(error)) from error
        for asset in self.bundle.assets:
            self._write(
                asset.id, asset.family, self._destination(self.assets[asset.id]), asset.content
            )
        self.dependencies.update(
            Dependency(dependent, required, edge.mode)
            for edge in self.bundle.dependencies
            for dependent in aliases.get(edge.dependent, [edge.dependent])
            for required in aliases.get(edge.required, [edge.required])
            if dependent != required
        )
        shared = next((impact for impact in self.impacts if impact.shared), None)
        if shared is not None and not self.accept_shared_scope:
            raise self.fail(
                "shared-scope",
                "Approve the shared source and selector scope shown in the preview",
                shared.path,
            )
        try:
            return RenderedBundle(self.artifacts, self.dependencies, self.notices)
        except ValueError as error:
            raise self.fail("source-plan", str(error)) from error
