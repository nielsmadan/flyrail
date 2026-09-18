import hashlib
from collections.abc import Iterable
from dataclasses import dataclass, fields, is_dataclass
from enum import StrEnum
from pathlib import Path

from flyrail._validation import validate_identifier, validate_relative_path
from flyrail.artifacts import Family
from flyrail.content import (
    Content,
    DocumentSchema,
    FileContent,
    StructuredContent,
    TreeContent,
    validate_content,
)
from flyrail.values import Value, freeze_value, semantic_bytes


def model_value(value: object) -> Value:
    if is_dataclass(value) and not isinstance(value, type):
        return freeze_value(
            {
                "type": type(value).__name__,
                "fields": {
                    field.name: model_value(getattr(value, field.name)) for field in fields(value)
                },
            }
        )
    if isinstance(value, bytes):
        return freeze_value({"bytes": value.hex()})
    if isinstance(value, Path):
        return freeze_value(value.as_posix())
    if isinstance(value, StrEnum):
        return freeze_value(str(value))
    if isinstance(value, tuple):
        from flyrail.values import ArrayValue

        return ArrayValue(model_value(item) for item in value)
    return freeze_value(value)


class DependencyMode(StrEnum):
    REVISION = "revision"
    STABLE_REFERENCE = "stable-reference"


@dataclass(frozen=True, slots=True)
class Dependency:
    dependent: str
    required: str
    mode: DependencyMode = DependencyMode.REVISION

    def __post_init__(self) -> None:
        validate_identifier(self.dependent, "dependent artifact")
        validate_identifier(self.required, "required artifact")
        if not isinstance(self.mode, DependencyMode):
            raise TypeError("dependency mode must be a DependencyMode")
        if self.dependent == self.required:
            raise ValueError("an artifact cannot depend on itself")


def dependency_order(names: Iterable[str], dependencies: Iterable[Dependency]) -> tuple[str, ...]:
    pending = set(names)
    edges = tuple(dependencies)
    if any(type(edge) is not Dependency for edge in edges):
        raise TypeError("dependencies must contain Dependency values")
    if len({(edge.dependent, edge.required) for edge in edges}) != len(edges):
        raise ValueError("duplicate dependency")
    if any(edge.required not in pending or edge.dependent not in pending for edge in edges):
        raise ValueError("dependency references an unknown artifact or asset")
    result: list[str] = []
    while pending:
        ready = sorted(
            name
            for name in pending
            if not any(edge.dependent == name and edge.required in pending for edge in edges)
        )
        if not ready:
            raise ValueError("cyclic artifact dependencies")
        result.extend(ready)
        pending.difference_update(ready)
    return tuple(result)


@dataclass(frozen=True, slots=True)
class RenderedArtifact:
    id: str
    family: Family
    destination: Path
    content: Content
    subtree: str | None = None
    require_existing: bool = False
    schema_requirements: tuple[DocumentSchema, ...] = ()

    def __post_init__(self) -> None:
        validate_identifier(self.id, "rendered artifact id")
        if not isinstance(self.family, Family):
            raise TypeError("rendered family must be a Family")
        if not isinstance(self.destination, Path) or not self.destination.is_absolute():
            raise ValueError("rendered destination must be an absolute Path")
        if ".." in self.destination.parts or self.destination == self.destination.parent:
            raise ValueError("rendered destination must name a concrete resource")
        validate_content(self.content)
        schemas = tuple(self.schema_requirements)
        if any(type(schema) is not DocumentSchema for schema in schemas):
            raise TypeError("schema requirements must contain DocumentSchema values")
        if len({schema.key for schema in schemas}) != len(schemas):
            raise ValueError("duplicate document schema requirement")
        if schemas and (
            not isinstance(self.content, StructuredContent)
            or any(schema.format is not self.content.format for schema in schemas)
        ):
            raise ValueError("document schema requirements need matching structured content")
        object.__setattr__(self, "schema_requirements", schemas)
        if not isinstance(self.require_existing, bool):
            raise TypeError("require_existing must be a boolean")
        if self.subtree is not None:
            validate_relative_path(self.subtree, "rendered subtree")
            if not isinstance(self.content, TreeContent):
                raise ValueError("subtree claims require tree content")


class NoticeKind(StrEnum):
    PREREQUISITE = "prerequisite"
    ACTIVATION = "activation"
    UNSUPPORTED = "unsupported"


@dataclass(frozen=True, slots=True)
class Notice:
    artifact_id: str
    kind: NoticeKind
    message: str
    code: str = ""

    def __post_init__(self) -> None:
        validate_identifier(self.artifact_id, "notice artifact id")
        if not isinstance(self.kind, NoticeKind):
            raise TypeError("notice kind must be a NoticeKind")
        if not isinstance(self.message, str) or not self.message.strip():
            raise ValueError("notice message must be nonblank")
        if not isinstance(self.code, str):
            raise TypeError("notice code must be a string")
        if self.code:
            validate_identifier(self.code, "notice code")


@dataclass(frozen=True, slots=True, init=False)
class RenderedBundle:
    artifacts: tuple[RenderedArtifact, ...]
    dependencies: tuple[Dependency, ...]
    notices: tuple[Notice, ...]
    content_digest: str
    routing_context: tuple[tuple[str, str], ...]

    def __init__(
        self,
        artifacts: Iterable[RenderedArtifact],
        dependencies: Iterable[Dependency] = (),
        notices: Iterable[Notice] = (),
        *,
        routing_context: Iterable[tuple[str, str]] = (),
    ) -> None:
        selected = tuple(artifacts)
        edges = tuple(dependencies)
        messages = tuple(notices)
        context = tuple((key, value) for key, value in routing_context)
        if any(not isinstance(key, str) or not isinstance(value, str) for key, value in context):
            raise TypeError("render routing context must contain string pairs")
        if len({key for key, _ in context}) != len(context):
            raise ValueError("render routing context keys must be unique")
        object.__setattr__(self, "routing_context", tuple(sorted(context)))
        if any(type(artifact) is not RenderedArtifact for artifact in selected):
            raise TypeError("rendered artifacts must contain RenderedArtifact values")
        names = [artifact.id for artifact in selected]
        if len(set(names)) != len(names):
            raise ValueError("rendered artifact ids must be unique")
        dependency_order(names, edges)
        by_id = {artifact.id: artifact for artifact in selected}
        for edge in edges:
            if edge.mode is DependencyMode.STABLE_REFERENCE:
                if not isinstance(by_id[edge.required].content, FileContent):
                    raise ValueError("stable references require a whole-file routing entry")
                if any(
                    child.dependent == edge.required and child.mode is not DependencyMode.REVISION
                    for child in edges
                ):
                    raise ValueError("stable routing entries require revision-bound dependencies")
        for notice in messages:
            if type(notice) is not Notice:
                raise TypeError("notices must contain Notice values")
        ordered = tuple(sorted(selected, key=lambda artifact: artifact.id))
        object.__setattr__(self, "artifacts", ordered)
        object.__setattr__(
            self, "dependencies", tuple(sorted(edges, key=lambda e: (e.dependent, e.required)))
        )
        object.__setattr__(self, "notices", messages)
        digest = hashlib.sha256(
            b"flyrail-render-v1\0" + semantic_bytes(model_value((ordered, self.dependencies)))
        )
        object.__setattr__(self, "content_digest", digest.hexdigest())

    @property
    def supported(self) -> bool:
        return not any(notice.kind is NoticeKind.UNSUPPORTED for notice in self.notices)
