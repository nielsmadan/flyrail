from collections.abc import Iterable
from dataclasses import dataclass
from enum import StrEnum
from typing import TypeAlias

from flyrail._validation import portable_path_key, validate_identifier
from flyrail.models import BundleEntry
from flyrail.values import Scalar, Value, scalar_text, validate_value


class DocumentFormat(StrEnum):
    JSON = "json"
    JSONC = "jsonc"
    TOML = "toml"


class FileMode(StrEnum):
    PRIVATE = "private"
    READABLE = "readable"
    EXECUTABLE = "executable"


@dataclass(frozen=True, slots=True)
class Key:
    name: str

    def __post_init__(self) -> None:
        scalar_text(self.name, "selector key")
        if not self.name:
            raise ValueError("selector keys must be nonempty")


@dataclass(frozen=True, slots=True, init=False)
class Member:
    identity: Value
    key: tuple[str, ...]

    def __init__(self, identity: Value, key: Iterable[str] = ()) -> None:
        validate_value(identity)
        if isinstance(key, str):
            raise TypeError("member key must be a sequence of names")
        names = tuple(key)
        for name in names:
            Key(name)
        if names and not isinstance(identity, Scalar):
            raise ValueError("native member identity must be a scalar")
        object.__setattr__(self, "identity", identity)
        object.__setattr__(self, "key", names)


SelectorPart: TypeAlias = Key | Member


@dataclass(frozen=True, slots=True, init=False)
class Selector:
    parts: tuple[SelectorPart, ...]

    def __init__(self, parts: Iterable[SelectorPart]) -> None:
        values = tuple(parts)
        if not values:
            raise ValueError("a selector must select a key or member")
        if any(type(part) not in {Key, Member} for part in values):
            raise TypeError("selectors contain only Key or Member; indexes are unsupported")
        object.__setattr__(self, "parts", values)


class Position(StrEnum):
    FIRST = "first"
    LAST = "last"
    BEFORE = "before"
    AFTER = "after"


@dataclass(frozen=True, slots=True)
class Placement:
    position: Position = Position.LAST
    anchor: Member | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.position, Position):
            raise TypeError("position must be a Position")
        anchored = self.position in {Position.BEFORE, Position.AFTER}
        if (anchored and type(self.anchor) is not Member) or (
            not anchored and self.anchor is not None
        ):
            raise ValueError("before/after placement requires exactly one member anchor")


def snapshot_entries(entries: Iterable[BundleEntry]) -> tuple[BundleEntry, ...]:
    selected = tuple(entries)
    paths: dict[str, BundleEntry] = {}
    spellings: dict[str, str] = {}
    for entry in selected:
        if type(entry) is not BundleEntry:
            raise TypeError("entries must contain BundleEntry values")
        if entry.path in paths:
            raise ValueError("duplicate entry path")
        paths[entry.path] = entry
        parts = entry.path.split("/")
        for size in range(1, len(parts) + 1):
            path = "/".join(parts[:size])
            key = portable_path_key(path)
            if key in spellings and spellings[key] != path:
                raise ValueError("portable entry path collision")
            spellings[key] = path
    for path in tuple(paths):
        parts = path.split("/")
        for size in range(1, len(parts)):
            parent = "/".join(parts[:size])
            if parent in paths and not paths[parent].is_directory:
                raise ValueError("entry file/directory collision")
            paths.setdefault(parent, BundleEntry(parent))
    return tuple(sorted(paths.values(), key=lambda entry: entry.path.encode()))


@dataclass(frozen=True, slots=True)
class FileContent:
    data: bytes
    mode: FileMode = FileMode.PRIVATE

    def __post_init__(self) -> None:
        if not isinstance(self.data, bytes):
            raise TypeError("file content must be immutable bytes")
        if not isinstance(self.mode, FileMode):
            raise TypeError("file mode must be a FileMode")


@dataclass(frozen=True, slots=True, init=False)
class TreeContent:
    entries: tuple[BundleEntry, ...]

    def __init__(self, entries: Iterable[BundleEntry]) -> None:
        object.__setattr__(self, "entries", snapshot_entries(entries))


@dataclass(frozen=True, slots=True)
class SectionBoundaries:
    start: str
    end: str

    def __post_init__(self) -> None:
        for line in (self.start, self.end):
            scalar_text(line, "section boundary")
            if not line.strip() or any(character in line for character in "\r\n\0"):
                raise ValueError("section boundaries must be nonblank single lines without NUL")
        if self.start == self.end:
            raise ValueError("section boundaries must be distinct")

    @classmethod
    def from_marker(cls, marker: str) -> "SectionBoundaries":
        validate_identifier(marker, "section marker")
        return cls(f"<!-- flyrail:{marker}:start -->", f"<!-- flyrail:{marker}:end -->")


@dataclass(frozen=True, slots=True)
class SectionContent:
    marker: str
    text: str
    boundaries: SectionBoundaries | None = None

    def __post_init__(self) -> None:
        validate_identifier(self.marker, "section marker")
        scalar_text(self.text, "section text")
        if "\0" in self.text or "<!-- flyrail:" in self.text:
            raise ValueError("section text cannot contain NUL or Flyrail markers")
        if self.boundaries is not None:
            if type(self.boundaries) is not SectionBoundaries:
                raise TypeError("boundaries must be SectionBoundaries")
            if any(
                line in self.text.splitlines()
                for line in (self.boundaries.start, self.boundaries.end)
            ):
                raise ValueError("section text cannot contain its boundary lines")

    @property
    def selector(self) -> SectionBoundaries:
        return self.boundaries or SectionBoundaries.from_marker(self.marker)


@dataclass(frozen=True, slots=True)
class StructuredContent:
    format: DocumentFormat
    selector: Selector
    value: Value
    placement: Placement | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.format, DocumentFormat) or type(self.selector) is not Selector:
            raise TypeError("structured content requires a document format and selector")
        validate_value(self.value)
        if self.placement is not None:
            if type(self.placement) is not Placement:
                raise TypeError("placement must be a Placement")
            if not isinstance(self.selector.parts[-1], Member):
                raise ValueError("placement is only valid for an array member")
            if self.placement.anchor == self.selector.parts[-1]:
                raise ValueError("a member cannot be its own insertion anchor")


@dataclass(frozen=True, slots=True)
class DocumentSchema:
    format: DocumentFormat
    key: str
    value: Scalar

    def __post_init__(self) -> None:
        if not isinstance(self.format, DocumentFormat) or self.format not in {
            DocumentFormat.JSON,
            DocumentFormat.JSONC,
        }:
            raise ValueError("document schema scaffolds require JSON or JSONC")
        Key(self.key)
        if type(self.value) is not Scalar:
            raise TypeError("document schema fields require a scalar value")

    @property
    def content(self) -> StructuredContent:
        return StructuredContent(self.format, Selector([Key(self.key)]), self.value)


Content: TypeAlias = FileContent | TreeContent | SectionContent | StructuredContent


def validate_content(content: Content) -> None:
    if type(content) not in {FileContent, TreeContent, SectionContent, StructuredContent}:
        raise TypeError("content must be a file, tree, section or structured selection")
