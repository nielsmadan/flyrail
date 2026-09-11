import hashlib
import importlib
import importlib.resources
import os
import zipfile
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType

from flyrail._manifest import read_manifest
from flyrail._sources import DirectorySource, ZipSource
from flyrail._validation import validate_relative_path
from flyrail.models import BundleEntry, BundleIdentity, SkillSpec


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


@dataclass(frozen=True, slots=True, init=False)
class Bundle:
    identity: BundleIdentity
    skills: tuple[SkillSpec, ...]
    entries: tuple[BundleEntry, ...]
    content_digest: str
    source_roots: tuple[Path, ...]

    def __init__(self) -> None:
        raise TypeError("use Bundle.from_directory or Bundle.from_package")

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
    def _from_source(cls, source: DirectorySource | ZipSource, roots: tuple[Path, ...]) -> "Bundle":
        identity, specs = read_manifest(source.read_manifest())
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
        return result
