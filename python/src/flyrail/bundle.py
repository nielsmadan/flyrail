import hashlib
import importlib
import importlib.resources
import json
import os
import zipfile
from collections.abc import Iterable, Mapping
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType

from flyrail._config_manifest import read_config_manifest
from flyrail._manifest import _reject_constant, _unique_object, read_manifest
from flyrail._memory import MemorySource
from flyrail._sources import DirectorySource, ZipSource
from flyrail._validation import validate_relative_path
from flyrail.artifacts import (
    Artifact,
    AssetRef,
    Command,
    HookArtifact,
    McpArtifact,
    SkillArtifact,
    SupportAsset,
    validate_artifact,
)
from flyrail.content import TreeContent
from flyrail.models import BundleEntry, BundleIdentity, SkillSpec
from flyrail.rendered import Dependency, dependency_order, model_value
from flyrail.values import Scalar, scalar_text, semantic_bytes


def _json_value(value: object, active: frozenset[int] = frozenset()) -> object:
    if id(value) in active:
        raise ValueError("circular manifest value")
    if isinstance(value, Mapping):
        result: dict[str, object] = {}
        for key, item in value.items():
            scalar_text(key, "manifest object key")
            result[key] = _json_value(item, active | {id(value)})
        return result
    if type(value) is list:
        return [_json_value(item, active | {id(value)}) for item in value]
    if value is None or isinstance(value, str | int | float | bool):
        Scalar(value)
        return value
    raise TypeError("manifest values must be JSON objects, arrays or scalars")


def _content_digest(entries: Iterable[BundleEntry]) -> str:
    ordered = sorted(entries, key=lambda entry: entry.path.encode("utf-8"))
    digest = hashlib.sha256(b"flyrail-content-v1\0")
    digest.update(len(ordered).to_bytes(8, "big"))
    for entry in ordered:
        path = entry.path.encode("utf-8")
        digest.update(b"D" if entry.is_directory else b"F")
        digest.update(len(path).to_bytes(8, "big"))
        digest.update(path)
        if entry.data is not None:
            digest.update(bytes([entry.executable]))
            digest.update(len(entry.data).to_bytes(8, "big"))
            digest.update(entry.data)
    return digest.hexdigest()


def _asset_references(
    artifacts: tuple[Artifact, ...], assets: tuple[SupportAsset, ...], edges: tuple[Dependency, ...]
) -> None:
    known = {asset.id: asset for asset in assets}
    for artifact in artifacts:
        command = (
            artifact.command
            if isinstance(artifact, HookArtifact)
            else artifact.transport
            if isinstance(artifact, McpArtifact)
            else None
        )
        if not isinstance(command, Command):
            continue
        for value in (*command.argv, command.cwd):
            if not isinstance(value, AssetRef):
                continue
            if value.asset_id not in known:
                raise ValueError("asset reference names an unknown support asset")
            if Dependency(artifact.id, value.asset_id) not in edges:
                raise ValueError("asset reference requires a direct dependency")
            entry = next(
                (item for item in known[value.asset_id].content.entries if item.path == value.path),
                None,
            )
            if value.path is not None and entry is None:
                raise ValueError("asset reference path does not exist in snapshot")
            if value is command.cwd and entry is not None and not entry.is_directory:
                raise ValueError("asset cwd must reference a directory")
            if value is command.argv[0] and (entry is None or entry.is_directory):
                raise ValueError("asset command must reference a file")


@dataclass(frozen=True, slots=True, init=False)
class Bundle:
    identity: BundleIdentity
    skills: tuple[SkillSpec, ...]
    entries: tuple[BundleEntry, ...]
    content_digest: str
    source_roots: tuple[Path, ...]
    artifacts: tuple[Artifact, ...]
    assets: tuple[SupportAsset, ...]
    dependencies: tuple[Dependency, ...]
    schema_version: int

    def __init__(self) -> None:
        raise TypeError(
            "use Bundle.from_directory, from_package, from_zip, from_memory or from_artifacts"
        )

    @property
    def id(self) -> str:
        return self.identity.id

    @property
    def version(self) -> str:
        return self.identity.version

    @classmethod
    def from_directory(cls, root: str | os.PathLike[str]) -> "Bundle":
        source = DirectorySource(Path(root))
        return cls._from_source(source, (source.root,))

    @classmethod
    def from_memory(
        cls, manifest: bytes | Mapping[str, object], entries: Iterable[BundleEntry] = ()
    ) -> "Bundle":
        if isinstance(manifest, Mapping):
            raw = json.dumps(_json_value(manifest), ensure_ascii=False, allow_nan=False).encode()
        elif isinstance(manifest, bytes):
            raw = manifest
        else:
            raise TypeError("manifest must be bytes or a mapping")
        return cls._from_source(MemorySource(raw, entries), ())

    @classmethod
    def from_zip(cls, path: str | os.PathLike[str], resource: str = "flyrail") -> "Bundle":
        validate_relative_path(resource, "archive resource")
        origin = Path(path)
        source = DirectorySource(origin.parent)
        source._observe(origin)
        with zipfile.ZipFile(origin) as archive:
            bundle = cls._from_source(ZipSource(archive, resource), (origin.resolve(strict=True),))
        source.finish()
        return bundle

    @classmethod
    def from_artifacts(
        cls,
        identity: BundleIdentity,
        artifacts: Iterable[Artifact],
        *,
        assets: Iterable[SupportAsset] = (),
        dependencies: Iterable[Dependency] = (),
    ) -> "Bundle":
        if type(identity) is not BundleIdentity:
            raise TypeError("identity must be a BundleIdentity")
        selected = tuple(artifacts)
        resources = tuple(assets)
        edges = tuple(dependencies)
        if not selected:
            raise ValueError("a configuration bundle requires at least one artifact")
        for artifact in selected:
            validate_artifact(artifact)
        if any(type(asset) is not SupportAsset for asset in resources):
            raise TypeError("assets must contain SupportAsset values")
        names = [artifact.id for artifact in selected] + [asset.id for asset in resources]
        if len(set(names)) != len(names):
            raise ValueError("artifact and asset ids must be unique")
        skill_names = [
            artifact.name for artifact in selected if isinstance(artifact, SkillArtifact)
        ]
        if len(set(skill_names)) != len(skill_names):
            raise ValueError("duplicate skill name")
        dependency_order(names, edges)
        _asset_references(selected, resources, edges)
        for asset in resources:
            if not any(edge.required == asset.id for edge in edges):
                raise ValueError("support assets must have an explicit dependent")
        ordered = tuple(sorted(selected, key=lambda artifact: artifact.id))
        ordered_assets = tuple(sorted(resources, key=lambda asset: asset.id))
        ordered_edges = tuple(sorted(edges, key=lambda edge: (edge.dependent, edge.required)))
        specs = tuple(
            SkillSpec(
                artifact.name,
                artifact.name,
                [entry.path for entry in artifact.content.entries if entry.executable],
            )
            for artifact in ordered
            if isinstance(artifact, SkillArtifact)
        )
        entries = tuple(
            sorted(
                (
                    entry
                    for artifact in ordered
                    if isinstance(artifact, SkillArtifact)
                    for entry in (
                        BundleEntry(artifact.name),
                        *(
                            BundleEntry(artifact.name + "/" + item.path, item.data, item.executable)
                            for item in artifact.content.entries
                        ),
                    )
                ),
                key=lambda entry: entry.path.encode(),
            )
        )
        result = object.__new__(cls)
        digest = hashlib.sha256(
            b"flyrail-config-v2\0"
            + semantic_bytes(model_value((ordered, ordered_assets, ordered_edges)))
        ).hexdigest()
        for name, value in (
            ("identity", identity),
            ("skills", specs),
            ("entries", entries),
            ("content_digest", digest),
            ("source_roots", ()),
            ("artifacts", ordered),
            ("assets", ordered_assets),
            ("dependencies", ordered_edges),
            ("schema_version", 2),
        ):
            object.__setattr__(result, name, value)
        return result

    @classmethod
    def from_package(cls, package: str | ModuleType, resource: str = "flyrail") -> "Bundle":
        validate_relative_path(resource, "package resource")
        if not isinstance(package, str | ModuleType):
            raise TypeError("package must be an importable name or module")
        module = importlib.import_module(package) if isinstance(package, str) else package
        spec = module.__spec__
        if spec is None or spec.submodule_search_locations is None:
            raise ValueError("package must identify a package, not a module")
        locations = tuple(spec.submodule_search_locations)
        if len(locations) != 1:
            raise ValueError("packages with multiple resource locations are ambiguous")
        root = importlib.resources.files(module)
        if isinstance(root, Path):
            source = DirectorySource(root)
            selected = source._find(resource)
            bundle = cls.from_directory(selected)
            source.finish()
            return bundle
        if isinstance(root, zipfile.Path):
            with root.root as archive:
                if not isinstance(archive.filename, str):
                    raise ValueError("package archive must have a filesystem origin")
                origin = Path(archive.filename)
                source_archive = DirectorySource(origin.parent)
                source_archive._observe(origin)
                source_zip = ZipSource(archive, root.at + resource)
                bundle = cls._from_source(source_zip, (origin.resolve(strict=True),))
                source_archive.finish()
                return bundle
        if spec.origin is None:
            source = DirectorySource(Path(locations[0]))
            bundle = cls.from_directory(source._find(resource))
            source.finish()
            return bundle
        raise ValueError("unsupported package resource provider; use filesystem or ZIP resources")

    @classmethod
    def _from_source(
        cls, source: DirectorySource | ZipSource | MemorySource, roots: tuple[Path, ...]
    ) -> "Bundle":
        data = source.read_manifest()
        raw: object = json.loads(
            data.decode("utf-8"), object_pairs_hook=_unique_object, parse_constant=_reject_constant
        )
        if isinstance(raw, dict) and raw.get("schema_version") == 2:
            identity, artifacts, assets, dependencies = read_config_manifest(raw, source)
            result = cls.from_artifacts(
                identity, artifacts, assets=assets, dependencies=dependencies
            )
            object.__setattr__(result, "source_roots", roots)
            if isinstance(source, DirectorySource):
                source.finish()
            return result
        identity, specs = read_manifest(data)
        entries = tuple(
            sorted(
                (entry for spec in specs for entry in source.skill_entries(spec)),
                key=lambda entry: entry.path.encode("utf-8"),
            )
        )
        files = {entry.path for entry in entries if not entry.is_directory}
        for spec in specs:
            if spec.name + "/SKILL.md" not in files:
                raise ValueError(f"skill must contain a SKILL.md file: {spec.name}")
            if any(spec.name + "/" + executable not in files for executable in spec.executables):
                raise ValueError(f"every executable must name an existing file: {spec.name}")
        if isinstance(source, DirectorySource):
            source.finish()
        result = object.__new__(cls)
        object.__setattr__(result, "identity", identity)
        object.__setattr__(result, "skills", specs)
        object.__setattr__(result, "entries", entries)
        object.__setattr__(result, "content_digest", _content_digest(entries))
        object.__setattr__(result, "source_roots", roots)
        object.__setattr__(
            result,
            "artifacts",
            tuple(
                SkillArtifact(
                    spec.name,
                    spec.name,
                    TreeContent(
                        BundleEntry(entry.path[len(spec.name) + 1 :], entry.data, entry.executable)
                        for entry in entries
                        if entry.path.startswith(spec.name + "/")
                    ),
                )
                for spec in specs
            ),
        )
        object.__setattr__(result, "assets", ())
        object.__setattr__(result, "dependencies", ())
        object.__setattr__(result, "schema_version", 1)
        return result
