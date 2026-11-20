import re
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import cast

from flyrail._observation import ObservationFailure
from flyrail._read_preconditions import ReadPrecondition, read_input
from flyrail._validation import portable_path_key, validate_relative_path
from flyrail.targets import Agent

HARNESS_NAMES = frozenset(agent.value for agent in Agent if agent is not Agent.COPILOT)
AUTOMATIC = frozenset(
    {"permissions", "mcp-permissions", "mcp", "skills", "agents", "module-config"}
)
TYPES = frozenset(
    {
        "instructions",
        "settings",
        "permissions",
        "hooks",
        "mcp",
        "plugins",
        "defaults",
        "skills",
        "agents",
        "module-config",
        "kits",
    }
)
SLICES = {
    name: (
        AUTOMATIC
        - ({"mcp-permissions"} if name != "claude" else set())
        - ({"module-config"} if name == "codex" else set())
    )
    | {"hooks", "instructions"}
    | ({"plugins"} if name in {"claude", "codex", "droid", "pi"} else set())
    | ({"defaults"} if name == "codex" else set())
    for name in HARNESS_NAMES
}
EXTRAS = frozenset({"settings", "preserve", "substitute"})
SETTINGS_READERS = {
    "claude": {"permissions", "hooks", "plugins"},
    "cursor": {"permissions"},
    "droid": {"permissions", "plugins"},
    "opencode": {"permissions", "mcp"},
    "pi": {"plugins"},
}
LEGACY_RENDERERS = {
    **{name: (name, "permissions") for name in HARNESS_NAMES},
    **{f"{name}-servers": (name, "mcp") for name in HARNESS_NAMES},
    "codex-config": ("codex", "mcp"),
    "codex-settings": ("codex", "defaults"),
    "claude-mcp-permissions": ("claude", "mcp-permissions"),
    **{f"{name}-hooks": (name, "hooks") for name in HARNESS_NAMES},
    **{f"{name}-plugins": (name, "plugins") for name in ("claude", "codex", "droid", "pi")},
}
_KEY = r"""(?:[A-Za-z0-9_-]+|"(?:[^"\\\r\n]|\\.)*"|'[^'\r\n]*')"""
_FIELD = {"instructions": "order", "hooks": "hooks"}
_NAMED_TABLES = {"instructions", "permissions", "skills"}
_RESERVED = _NAMED_TABLES | {"source", "extends", "remove", "all"}


@dataclass(frozen=True, slots=True)
class LoadoutDiagnostic:
    code: str
    message: str
    path: Path


class LoadoutFailure(Exception):
    def __init__(self, code: str, message: str, path: Path) -> None:
        super().__init__(message)
        self.diagnostic = LoadoutDiagnostic(code, message, path)


@dataclass(frozen=True, slots=True)
class LoadoutSource:
    name: str
    path: Path
    use: frozenset[str]
    declared_in: Path
    overrides: tuple[tuple[str, tuple[str, ...]], ...]


@dataclass(frozen=True, slots=True)
class LoadoutSelector:
    harness: Agent
    category: str
    manifest: Path
    key: tuple[str, ...]
    entries: tuple[str, ...]
    substitutes: tuple[tuple[str, str], ...] = ()
    array: bool = True
    destinations: int = 1
    all_profiles: bool = False


@dataclass(frozen=True, slots=True)
class LoadoutHarness:
    agent: Agent
    slices: frozenset[str]


@dataclass(frozen=True, slots=True)
class SkillPolicy:
    name: str
    agents: tuple[str, ...]
    render: bool
    declarations: tuple[Path, ...]


@dataclass(frozen=True, slots=True)
class Manifest:
    root: Path
    profile: str
    chain: tuple[Path, ...]
    sources: tuple[LoadoutSource, ...]
    harnesses: tuple[LoadoutHarness, ...]
    selectors: tuple[LoadoutSelector, ...]
    policies: tuple[SkillPolicy, ...]
    custom_routes: frozenset[tuple[str, str]]


class Inputs:
    def __init__(self) -> None:
        self._reads: dict[Path, ReadPrecondition] = {}

    @property
    def preconditions(self) -> tuple[ReadPrecondition, ...]:
        return tuple(self._reads.values())

    def physical(self, path: Path) -> Path:
        return self._reads[path].resolved

    def read(
        self, path: Path, code: str, *, within: Path | None = None
    ) -> tuple[ReadPrecondition, bytes | None]:
        try:
            physical = path.resolve()
            if within is not None and not physical.is_relative_to(within):
                raise LoadoutFailure("escaped-path", f"{path} escapes {within}", path)
            seen, data = read_input(path, expected=physical)
        except (OSError, ObservationFailure, RuntimeError, ValueError) as error:
            raise LoadoutFailure(code, f"Cannot read {path}: {error}", path) from error
        prior = self._reads.setdefault(path, seen)
        if prior != seen:
            raise LoadoutFailure(
                "concurrent-change", f"{path} changed while reading the setup", path
            )
        return seen, data

    def table(
        self, path: Path, code: str, *, optional: bool = False, within: Path | None = None
    ) -> dict[str, object] | None:
        seen, data = self.read(path, code, within=within)
        if data is None:
            if optional and seen.identity is None:
                return None
            raise LoadoutFailure(code, f"Expected a TOML file at {path}", path)
        try:
            return tomllib.loads(data.decode("utf-8"))
        except (UnicodeError, tomllib.TOMLDecodeError, RecursionError) as error:
            raise LoadoutFailure(code, f"{path} is not valid UTF-8 TOML: {error}", path) from error

    def directory(self, path: Path) -> Path:
        seen, data = self.read(path, "invalid-source")
        if data is not None or seen.identity is None:
            raise LoadoutFailure("invalid-source", f"Expected a source directory at {path}", path)
        return seen.resolved


def absolute(path: Path, label: str) -> Path:
    if not isinstance(path, Path) or not path.is_absolute() or "\0" in str(path):
        raise ValueError(f"{label} must be an absolute path without NUL")
    return path


def source_path(raw: str, home: Path, relative_to: Path | None = None) -> Path:
    if raw == "~" or raw.startswith("~/"):
        return home / raw[2:].lstrip("/") if raw.startswith("~/") else home
    if raw.startswith("~") or "\0" in raw:
        raise ValueError("Use ~/ for the explicit home directory")
    value = Path(raw)
    return (
        relative_to / value
        if relative_to is not None and not value.is_absolute()
        else absolute(value, "source")
    )


def profile_file(root: Path, profile: str) -> Path:
    if not profile or profile in {".", ".."} or any(char in profile for char in "/\\\0"):
        raise LoadoutFailure("invalid-profile", "Profile must name a sibling manifest", root)
    return root / ("loadout.toml" if profile == "default" else profile + ".toml")


def _table(value: object, path: Path, label: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise LoadoutFailure("invalid-manifest", f"{label} must be a table", path)
    return cast("dict[str, object]", value)


def strings(
    value: object, path: Path, label: str, code: str = "invalid-manifest"
) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise LoadoutFailure(code, f"{label} must be an array of strings", path)
    return tuple(value)


def _remove_path(value: str, path: Path) -> tuple[str, ...]:
    if not re.fullmatch(rf"{_KEY}(?:\s*\.\s*{_KEY})*", value.strip()):
        raise LoadoutFailure("invalid-profile", f"Invalid TOML remove path {value!r}", path)
    try:
        return tuple(next(iter(tomllib.loads(f"{part} = 0"))) for part in re.findall(_KEY, value))
    except tomllib.TOMLDecodeError as error:
        raise LoadoutFailure(
            "invalid-profile", f"Invalid TOML remove path {value!r}", path
        ) from error


class _Merge:
    def __init__(self) -> None:
        self.values: dict[str, object] = {}
        self.origins: dict[tuple[str, ...], Path] = {}

    def remove(self, data: Mapping[str, object], file: Path) -> None:
        if "remove" not in data:
            return
        if "extends" not in data:
            raise LoadoutFailure("invalid-profile", "Profile remove requires extends", file)
        paths = [
            _remove_path(raw, file)
            for raw in strings(data["remove"], file, "remove", "invalid-profile")
        ]
        for index, path in enumerate(paths):
            if any(
                path[: len(prev)] == prev or prev[: len(path)] == path for prev in paths[:index]
            ):
                raise LoadoutFailure("invalid-profile", f"Overlapping remove path {path!r}", file)
            current = self.values
            for key in path[:-1]:
                child = current.get(key)
                if not isinstance(child, dict):
                    raise LoadoutFailure(
                        "invalid-profile",
                        f"Remove path {path!r} must traverse existing tables",
                        file,
                    )
                current = cast("dict[str, object]", child)
            if path[-1] not in current:
                raise LoadoutFailure(
                    "invalid-profile", f"Remove path {path!r} does not exist", file
                )
            del current[path[-1]]
            self.forget(path)

    def forget(self, prefix: tuple[str, ...]) -> None:
        self.origins = {
            key: value for key, value in self.origins.items() if key[: len(prefix)] != prefix
        }

    def merge(self, data: dict[str, object], file: Path) -> None:
        self.remove(data, file)
        entries: list[tuple[dict[str, object], tuple[str, ...], object, int]] = [
            (self.values, (key,), value, 2 if key in _NAMED_TABLES else 1)
            for key, value in data.items()
            if key not in {"extends", "remove"}
        ]
        while entries:
            parent, path, value, depth = entries.pop()
            key = path[-1]
            existing = parent.get(key)
            if depth and isinstance(value, dict):
                if not isinstance(existing, dict):
                    self.origins.pop(path, None)
                    existing = {}
                    parent[key] = existing
                self.origins[path] = file
                entries.extend(
                    (existing, (*path, name), value, depth - 1) for name, value in value.items()
                )
            else:
                if isinstance(existing, dict):
                    self.forget(path)
                parent[key] = value
                self.origins[path] = file


def _read_chain(root: Path, profile: str, inputs: Inputs) -> tuple[_Merge, tuple[Path, ...], bool]:
    marker = root / "loadout.toml"
    base = inputs.table(marker, "invalid-manifest", within=root)
    logical = profile_file(root, profile)
    data = (
        base
        if logical == marker
        else inputs.table(logical, "invalid-profile", optional=True, within=root)
    )
    fallback = data is None
    if fallback:
        logical = marker
        data = base
    chain: list[tuple[Path, dict[str, object]]] = []
    seen: set[Path] = set()
    while True:
        path = inputs.physical(logical)
        if path.parent != root:
            raise LoadoutFailure(
                "escaped-path", "Profile manifest must be a sibling of loadout.toml", logical
            )
        if path in seen:
            raise LoadoutFailure("profile-cycle", f"Profile extends cycle at {logical}", logical)
        table = cast("dict[str, object]", data)
        seen.add(path)
        chain.append((path, table))
        parent = table.get("extends")
        if parent is None:
            break
        if not isinstance(parent, str):
            raise LoadoutFailure(
                "invalid-profile", "Profile extends must name a sibling manifest", path
            )
        logical = profile_file(root, parent)
        data = inputs.table(logical, "invalid-profile", within=root)
    merged = _Merge()
    for path, data in reversed(chain):
        merged.merge(data, path)
    return (
        merged,
        tuple(path for path, _ in chain),
        fallback or profile == "loadout" or "extends" not in chain[0][1],
    )


def _sources(
    raw: object, file: Path, root: Path, home: Path, inputs: Inputs
) -> tuple[LoadoutSource, ...]:
    if not isinstance(raw, list) or not raw:
        raise LoadoutFailure("invalid-manifest", "Declare at least one [[source]]", file)
    result: list[LoadoutSource] = []
    names: set[str] = set()
    for item in raw:
        source = _table(item, file, "source")
        name, raw_path = source.get("name"), source.get("path")
        if not isinstance(name, str) or not isinstance(raw_path, str) or not raw_path:
            raise LoadoutFailure("invalid-source", "Each source needs a name and path", file)
        if set(source) - {"name", "path", "use", "overrides"}:
            raise LoadoutFailure(
                "unsupported-manifest", f"Unsupported fields in source {name}", file
            )
        try:
            if "/" in name:
                raise ValueError("source name must be one component")
            validate_relative_path(name, "source name")
            path = inputs.directory(source_path(raw_path, home, root))
        except ValueError as error:
            raise LoadoutFailure("invalid-source", str(error), file) from error
        key = portable_path_key(name)
        if key in names:
            raise LoadoutFailure("invalid-source", f"Duplicate source name {name}", file)
        names.add(key)
        use = frozenset(strings(source["use"], file, f"{name}.use")) if "use" in source else TYPES
        if not use or use - TYPES:
            raise LoadoutFailure(
                "invalid-source", f"Invalid use categories for source {name}", file
            )
        if any(path == other.path and use & other.use for other in result):
            raise LoadoutFailure("invalid-source", f"Source {name} aliases an earlier source", file)
        if (
            "kits" in use
            and inputs.read(path / "templates", "invalid-source", within=path)[0].identity
            is not None
        ):
            raise LoadoutFailure(
                "unsupported-manifest", "Rename templates/ to kits/ in this source", path
            )
        overrides = _table(source.get("overrides", {}), file, "source.overrides")
        if set(overrides) - (use & {"module-config", "agents", "skills"}):
            raise LoadoutFailure(
                "invalid-source", f"Invalid override categories for source {name}", file
            )
        parsed = []
        for category, entries in sorted(overrides.items()):
            values = tuple(
                Path(value).as_posix()
                for value in strings(entries, file, f"{name}.overrides.{category}")
            )
            if len(set(values)) != len(values):
                raise LoadoutFailure(
                    "invalid-source", f"Duplicate {category} override in source {name}", file
                )
            for value in values:
                parts = value.split("/")
                try:
                    validate_relative_path(value, "source override")
                    if (
                        ".git" in parts
                        or (
                            category == "module-config"
                            and (len(parts) < 2 or parts[0] not in HARNESS_NAMES)
                        )
                        or (category != "module-config" and len(parts) != 1)
                    ):
                        raise ValueError("invalid override path")
                except ValueError as error:
                    raise LoadoutFailure(
                        "invalid-source", f"Invalid {category} override {value!r} in {name}", file
                    ) from error
            parsed.append((category, values))
        result.append(LoadoutSource(name, path, use, file, tuple(parsed)))
    return tuple(result)


def _substitutes(raw: object, path: Path) -> tuple[tuple[str, str], ...]:
    values = _table(raw, path, "substitute")
    if not all(isinstance(v, str) for v in values.values()):
        raise LoadoutFailure("invalid-manifest", "substitute values must be strings", path)
    return tuple((key, cast("str", value)) for key, value in sorted(values.items()))


def _paths(block: dict[str, object], file: Path, claimed: set[str]) -> int:
    outputs = strings(block.get("destinations", []), file, "destinations")
    output = block.get("output")
    if (not outputs and output is None) or any(not item or "\0" in item for item in outputs):
        raise LoadoutFailure(
            "invalid-manifest", "Legacy targets require valid destinations or output", file
        )
    if output is not None:
        if (
            not isinstance(output, str)
            or not output
            or Path(output).is_absolute()
            or ".." in Path(output).parts
            or Path(output) == Path(".")
            or output in claimed
        ):
            raise LoadoutFailure(
                "invalid-manifest", "Legacy output must be a unique relative path", file
            )
        claimed.add(output)
    return len(outputs) + int(output is not None)


def load_manifest(root: Path, profile: str, home: Path, inputs: Inputs) -> Manifest:
    merged, chain, legacy_profile = _read_chain(root, profile, inputs)
    data, origins = merged.values, merged.origins
    file = chain[0]
    if set(data) - _RESERVED - HARNESS_NAMES:
        raise LoadoutFailure(
            "unsupported-manifest",
            f"Unknown manifest tables: {', '.join(sorted(set(data) - _RESERVED - HARNESS_NAMES))}",
            file,
        )
    sources = _sources(data.get("source"), origins.get(("source",), file), root, home, inputs)
    common = _table(data.get("all", {}), file, "all")
    harnesses: dict[Agent, set[str]] = {}
    selectors: list[LoadoutSelector] = []
    custom_routes: set[tuple[str, str]] = set()
    named_profiles: set[str] = set()
    claimed: set[str] = set()
    for category in ("instructions", "permissions"):
        for name, raw in sorted(_table(data.get(category, {}), file, category).items()):
            block = _table(raw, file, f"{category}.{name}")
            count = _paths(block, file, claimed)
            target_profile = block.get("profile")
            if target_profile is not None:
                if not isinstance(target_profile, str):
                    raise LoadoutFailure(
                        "invalid-manifest", "Legacy target profile must be a string", file
                    )
                if target_profile:
                    named_profiles.add(target_profile)
            if category == "instructions":
                if name not in HARNESS_NAMES:
                    raise LoadoutFailure(
                        "unsupported-manifest", f"Unknown legacy instruction harness {name}", file
                    )
                key = (category, name, "order")
                entries = strings(block.get("order"), file, f"{category}.{name}.order")
                substitutes = _substitutes(block.get("substitute", {}), file)
                if target_profile is None or target_profile == profile:
                    harnesses.setdefault(Agent(name), set()).add("instructions")
                    selectors.append(
                        LoadoutSelector(
                            Agent(name),
                            "instructions",
                            origins[key],
                            key,
                            entries,
                            substitutes,
                            destinations=count,
                            all_profiles=target_profile is None,
                        )
                    )
            else:
                renderer = block.get("render")
                if (
                    not isinstance(renderer, str)
                    or renderer not in LEGACY_RENDERERS
                    or "rules" in block
                ):
                    raise LoadoutFailure(
                        "unsupported-manifest",
                        f"Unsupported legacy renderer in permissions.{name}",
                        file,
                    )
                owner, slice_name = LEGACY_RENDERERS[renderer]
                if target_profile is None or target_profile == profile:
                    harnesses.setdefault(Agent(owner), set()).add(slice_name)
                    custom_routes.add((owner, slice_name))
    if profile != "default" and legacy_profile and profile not in named_profiles:
        raise LoadoutFailure("invalid-profile", f"Unknown profile {profile!r}", file)
    for name in sorted(HARNESS_NAMES & data.keys()):
        declared = _table(data[name], file, name)
        offered = SLICES[name]
        stray = set(declared) - offered - EXTRAS
        if stray or (name == "codex" and {"settings", "preserve"} & declared.keys()):
            raise LoadoutFailure("invalid-manifest", f"Unsupported fields in [{name}]", file)
        block = {
            key: value for key, value in (common | declared).items() if key in offered | EXTRAS
        }
        active = set(offered & AUTOMATIC) - block.keys()
        for category, value in block.items():
            if category in AUTOMATIC and not (
                isinstance(value, bool)
                or (value == [] and category in {"mcp", "permissions", "mcp-permissions"})
            ):
                raise LoadoutFailure(
                    "invalid-manifest",
                    f"{name}.{category} is automatic and must be enabled or disabled",
                    file,
                )
            if category == "preserve":
                strings(value, file, f"{name}.preserve")
            if category == "substitute":
                _substitutes(value, file)
            if category == "settings" and not isinstance(value, str):
                strings(value, file, f"{name}.settings")
            if category not in offered or value is False:
                continue
            active.add(category)
            if category not in _FIELD:
                if category in {"defaults", "plugins"} and not isinstance(value, str):
                    strings(value, file, f"{name}.{category}")
                continue
            route = (name if category in declared else "all", category)
            entries = (
                (value,)
                if category == "hooks" and isinstance(value, str)
                else strings(value, file, f"{name}.{category}")
            )
            substitutes = (
                _substitutes(block.get("substitute", {}), file)
                if category == "instructions"
                else ()
            )
            selectors.append(
                LoadoutSelector(
                    Agent(name),
                    category,
                    origins[route],
                    route,
                    entries,
                    substitutes,
                    not isinstance(value, str),
                )
            )
        if "settings" in declared and not active & SETTINGS_READERS.get(name, set()):
            raise LoadoutFailure(
                "invalid-manifest", f"{name}.settings needs an enabled slice that reads it", file
            )
        harnesses.setdefault(Agent(name), set()).update(active)
    if not any(harnesses.values()):
        raise LoadoutFailure("invalid-manifest", "No active targets for the selected profile", file)
    policies = []
    for name, raw in sorted(_table(data.get("skills", {}), file, "skills").items()):
        block = _table(raw, file, f"skills.{name}")
        agents = strings(block.get("agents"), file, f"skills.{name}.agents")
        render = block.get("render", True)
        if (
            not name
            or name in {".", ".."}
            or any(char in name for char in "/\\\0")
            or set(block) - {"agents", "render"}
            or not isinstance(render, bool)
            or len(agents) != len(set(agents))
            or any(
                agent not in HARNESS_NAMES or "skills" not in harnesses.get(Agent(agent), ())
                for agent in agents
            )
        ):
            raise LoadoutFailure("invalid-manifest", f"Invalid skill policy for {name!r}", file)
        declarations = tuple(sorted({origins["skills", name, key] for key in block}))
        policies.append(SkillPolicy(name, agents, render, declarations))
    return Manifest(
        root,
        profile,
        chain,
        sources,
        tuple(
            LoadoutHarness(agent, frozenset(slices))
            for agent, slices in sorted(harnesses.items())
            if slices
        ),
        tuple(selectors),
        tuple(policies),
        frozenset(custom_routes),
    )
