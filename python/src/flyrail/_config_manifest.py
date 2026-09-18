from typing import Protocol

from flyrail._manifest import _object, _string
from flyrail._validation import validate_relative_path
from flyrail.artifacts import (
    Artifact,
    AssetRef,
    Audience,
    Command,
    EnvRef,
    Family,
    HookArtifact,
    HookEvent,
    HookOutcome,
    HttpTransport,
    InstructionArtifact,
    McpArtifact,
    NativeArtifact,
    SkillArtifact,
    SupportAsset,
)
from flyrail.content import (
    Content,
    DocumentFormat,
    FileContent,
    FileMode,
    Key,
    Member,
    Placement,
    Position,
    SectionBoundaries,
    SectionContent,
    Selector,
    StructuredContent,
    TreeContent,
)
from flyrail.models import BundleEntry, BundleIdentity
from flyrail.rendered import Dependency, DependencyMode
from flyrail.targets import Agent, Platform, Surface, TargetScope
from flyrail.values import Value, freeze_value, value_from_record


class ConfigSource(Protocol):
    def read_file(self, path: str) -> bytes: ...

    def tree_entries(
        self, path: str, name: str, executables: tuple[str, ...]
    ) -> tuple[BundleEntry, ...]: ...


def _list(value: object) -> list[object]:
    if not isinstance(value, list):
        raise ValueError("expected a JSON array")
    return value


def _path(value: object) -> str:
    path = _string(value, "source path")
    validate_relative_path(path, "source path")
    return path


def _text(fields: dict[str, object], source: ConfigSource) -> str:
    if ("text" in fields) == ("path" in fields):
        raise ValueError("provide exactly one of text or path")
    return (
        _string(fields["text"], "text")
        if "text" in fields
        else source.read_file(_path(fields["path"])).decode("utf-8")
    )


def _tree(fields: dict[str, object], source: ConfigSource) -> TreeContent:
    executables = tuple(_path(value) for value in _list(fields.get("executables", [])))
    if len(set(executables)) != len(executables):
        raise ValueError("duplicate executable path")
    entries = source.tree_entries(_path(fields["path"]), "content", executables)
    result = TreeContent(
        BundleEntry(entry.path.removeprefix("content/"), entry.data, entry.executable)
        for entry in entries
        if entry.path != "content"
    )
    files = {entry.path for entry in result.entries if not entry.is_directory}
    if any(executable not in files for executable in executables):
        raise ValueError("every executable must name an existing file")
    return result


def _argument(value: object) -> str | AssetRef:
    if isinstance(value, dict):
        fields = _object(value, {"asset"}, {"path"})
        return AssetRef(
            _string(fields["asset"], "referenced asset id"),
            None if "path" not in fields else _path(fields["path"]),
        )
    return _string(value, "command argument")


def _command(value: object) -> Command:
    fields = _object(value, {"argv"}, {"env", "cwd"})
    raw_env = fields.get("env", {})
    if not isinstance(raw_env, dict):
        raise ValueError("command env must be a JSON object")
    env: list[tuple[str, str | EnvRef]] = []
    for key, item in raw_env.items():
        if isinstance(item, dict):
            reference = _object(item, {"env"}, set())
            env.append((key, EnvRef(_string(reference["env"], "environment reference"))))
        else:
            env.append((key, _string(item, "environment value")))
    return Command(
        [_argument(item) for item in _list(fields["argv"])],
        env,
        None if "cwd" not in fields else _argument(fields["cwd"]),
    )


def _member(value: object) -> Member:
    fields = _object(value, set(), {"identity", "semantic_identity", "key"})
    return Member(
        _value(fields, "identity", "semantic_identity"),
        [_string(name, "member key") for name in _list(fields.get("key", []))],
    )


def _value(fields: dict[str, object], plain: str, typed: str) -> Value:
    if (plain in fields) == (typed in fields):
        raise ValueError(f"provide exactly one of {plain} or {typed}")
    return freeze_value(fields[plain]) if plain in fields else value_from_record(fields[typed])


def _selector(value: object) -> Selector:
    return Selector(Key(item) if isinstance(item, str) else _member(item) for item in _list(value))


def _content(value: object, source: ConfigSource) -> Content:
    header = _object(
        value,
        {"kind"},
        {
            "path",
            "text",
            "mode",
            "executables",
            "marker",
            "format",
            "selector",
            "value",
            "semantic_value",
            "placement",
            "boundaries",
        },
    )
    kind = header["kind"]
    if kind == "file":
        fields = _object(value, {"kind", "path"}, {"mode"})
        return FileContent(
            source.read_file(_path(fields["path"])),
            FileMode(_string(fields.get("mode", "private"), "file mode")),
        )
    if kind == "tree":
        return _tree(_object(value, {"kind", "path"}, {"executables"}), source)
    if kind == "section":
        fields = _object(value, {"kind", "marker"}, {"text", "path", "boundaries"})
        boundaries = None
        if "boundaries" in fields:
            raw_boundaries = _object(fields["boundaries"], {"start", "end"}, set())
            boundaries = SectionBoundaries(
                _string(raw_boundaries["start"], "start boundary"),
                _string(raw_boundaries["end"], "end boundary"),
            )
        return SectionContent(
            _string(fields["marker"], "section marker"), _text(fields, source), boundaries
        )
    if kind == "structured":
        fields = _object(
            value, {"kind", "format", "selector"}, {"value", "semantic_value", "placement"}
        )
        placement = None
        if "placement" in fields:
            raw = _object(fields["placement"], {"position"}, {"anchor"})
            placement = Placement(
                Position(_string(raw["position"], "placement position")),
                None if "anchor" not in raw else _member(raw["anchor"]),
            )
        return StructuredContent(
            DocumentFormat(_string(fields["format"], "document format")),
            _selector(fields["selector"]),
            _value(fields, "value", "semantic_value"),
            placement,
        )
    raise ValueError("unknown native content kind")


def _artifact(value: object, source: ConfigSource) -> Artifact:
    header = _object(
        value,
        {"kind", "id"},
        {
            "name",
            "path",
            "executables",
            "text",
            "transport",
            "event",
            "command",
            "timeout_ms",
            "outcomes",
            "tools",
            "protocol_version",
            "family",
            "audience",
            "destination",
            "content",
        },
    )
    identifier = _string(header["id"], "artifact id")
    kind = header["kind"]
    if kind == "skill":
        fields = _object(value, {"kind", "id", "name", "path"}, {"executables"})
        return SkillArtifact(
            identifier, _string(fields["name"], "skill name"), _tree(fields, source)
        )
    if kind == "instruction":
        fields = _object(value, {"kind", "id"}, {"text", "path"})
        return InstructionArtifact(identifier, _text(fields, source))
    if kind == "mcp":
        fields = _object(value, {"kind", "id", "name", "transport"}, set())
        transport = _object(fields["transport"], {"kind"}, {"command", "url", "bearer_token_env"})
        if transport["kind"] == "stdio":
            transport = _object(fields["transport"], {"kind", "command"}, set())
            definition: Command | HttpTransport = _command(transport["command"])
        elif transport["kind"] == "http":
            transport = _object(fields["transport"], {"kind", "url"}, {"bearer_token_env"})
            definition = HttpTransport(
                _string(transport["url"], "MCP URL"),
                None
                if "bearer_token_env" not in transport
                else EnvRef(_string(transport["bearer_token_env"], "bearer environment reference")),
            )
        else:
            raise ValueError("unknown MCP transport")
        return McpArtifact(identifier, _string(fields["name"], "MCP server name"), definition)
    if kind == "hook":
        fields = _object(
            value,
            {"kind", "id", "event", "command", "protocol_version"},
            {"timeout_ms", "outcomes", "tools"},
        )
        if type(fields["protocol_version"]) is not int or fields["protocol_version"] != 1:
            raise ValueError("hook protocol_version must be integer 1")
        timeout = fields.get("timeout_ms", 10000)
        if type(timeout) is not int:
            raise ValueError("hook timeout must be an integer")
        return HookArtifact(
            identifier,
            HookEvent(_string(fields["event"], "hook event")),
            _command(fields["command"]),
            timeout,
            [
                HookOutcome(_string(item, "hook outcome"))
                for item in _list(fields.get("outcomes", ["continue"]))
            ],
            [_string(item, "hook tool") for item in _list(fields.get("tools", []))],
        )
    if kind == "native":
        fields = _object(
            value, {"kind", "id", "family", "audience", "destination", "content"}, set()
        )
        audience = _object(fields["audience"], {"agent", "scope"}, {"surface", "platform"})
        return NativeArtifact(
            identifier,
            Family(_string(fields["family"], "native family")),
            Audience(
                Agent(_string(audience["agent"], "agent")),
                TargetScope(_string(audience["scope"], "scope")),
                Surface(_string(audience.get("surface", "cli"), "surface")),
                None
                if "platform" not in audience
                else Platform(_string(audience["platform"], "platform")),
            ),
            _path(fields["destination"]),
            _content(fields["content"], source),
        )
    raise ValueError("unknown artifact kind")


def read_config_manifest(
    raw: object, source: ConfigSource
) -> tuple[BundleIdentity, tuple[Artifact, ...], tuple[SupportAsset, ...], tuple[Dependency, ...]]:
    fields = _object(
        raw, {"schema_version", "id", "version", "artifacts"}, {"assets", "dependencies"}
    )
    if type(fields["schema_version"]) is not int or fields["schema_version"] != 2:
        raise ValueError("configuration schema_version must be integer 2")
    artifacts = tuple(_artifact(item, source) for item in _list(fields["artifacts"]))
    assets: list[SupportAsset] = []
    for item in _list(fields.get("assets", [])):
        asset = _object(item, {"id", "family", "path"}, {"executables"})
        assets.append(
            SupportAsset(
                _string(asset["id"], "asset id"),
                Family(_string(asset["family"], "asset family")),
                _tree(asset, source),
            )
        )
    dependencies: list[Dependency] = []
    for item in _list(fields.get("dependencies", [])):
        edge = _object(item, {"dependent", "required"}, {"mode"})
        dependencies.append(
            Dependency(
                _string(edge["dependent"], "dependent"),
                _string(edge["required"], "required"),
                DependencyMode(_string(edge.get("mode", "revision"), "dependency mode")),
            )
        )
    return (
        BundleIdentity(_string(fields["id"], "bundle id"), _string(fields["version"], "version")),
        artifacts,
        tuple(assets),
        tuple(dependencies),
    )
