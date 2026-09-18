import hashlib
import re
from collections.abc import Iterable
from dataclasses import dataclass, replace
from enum import StrEnum
from itertools import pairwise
from typing import TYPE_CHECKING, TypeAlias

from flyrail._json_document import JsonDocument, PathParts
from flyrail._toml_layout import header_path, layout_headers
from flyrail.authority import Claim, Ownership
from flyrail.content import (
    DocumentFormat,
    Key,
    Member,
    Placement,
    Position,
    SectionBoundaries,
    SectionContent,
    Selector,
    StructuredContent,
)
from flyrail.values import ArrayValue, ObjectValue, Scalar, Value, freeze_value, validate_value

if TYPE_CHECKING:
    from flyrail._toml_document import TomlDocument

SharedContent: TypeAlias = SectionContent | StructuredContent
Backend: TypeAlias = "JsonDocument | TomlDocument"


class Acquisition(StrEnum):
    CONFLICT = "conflict"
    TAKEOVER = "takeover"
    ADOPT = "adopt"


class Disposition(StrEnum):
    CREATED = "created"
    TAKEN_OVER = "taken-over"
    ADOPTED = "adopted"


class EditStatus(StrEnum):
    CREATED = "created"
    TAKEN_OVER = "taken-over"
    ADOPTED = "adopted"
    CURRENT = "current"
    UPDATED = "updated"
    REMOVED = "removed"
    MODIFIED = "modified"
    CONFLICT = "conflict"


@dataclass(frozen=True, slots=True)
class ArrayPosition:
    previous: Member | None = None
    following: Member | None = None
    prefix: bytes = b""

    def __post_init__(self) -> None:
        if any(
            item is not None and type(item) is not Member
            for item in (self.previous, self.following)
        ):
            raise TypeError("array position anchors must be Members")
        if not isinstance(self.prefix, bytes):
            raise TypeError("array position prefix must be immutable bytes")
        if self.previous is not None and self.previous == self.following:
            raise ValueError("array position anchors must be distinct")


@dataclass(frozen=True, slots=True)
class SelectionSnapshot:
    value: bytes | Value
    syntax: bytes
    owned_syntax: bytes
    position: ArrayPosition | None = None
    layout: bytes = b""

    def __post_init__(self) -> None:
        if not isinstance(self.value, bytes):
            validate_value(self.value)
        if any(
            not isinstance(item, bytes) for item in (self.syntax, self.owned_syntax, self.layout)
        ):
            raise TypeError("selection syntax must be immutable bytes")
        if self.position is not None and type(self.position) is not ArrayPosition:
            raise TypeError("selection position must be an ArrayPosition")
        for relative, _ in layout_headers(self.layout):
            value = self.value
            for part in relative:
                if isinstance(part, str) and isinstance(value, ObjectValue):
                    try:
                        value = dict(value.items)[part]
                    except KeyError as error:
                        raise ValueError("invalid TOML restoration path") from error
                elif (
                    isinstance(part, int)
                    and isinstance(value, ArrayValue)
                    and part < len(value.items)
                ):
                    value = value.items[part]
                else:
                    raise ValueError("invalid TOML restoration path")
            if not isinstance(value, ObjectValue):
                raise ValueError("TOML restoration header requires an object")


@dataclass(frozen=True, slots=True)
class OwnedSelection:
    claim: Claim
    format: DocumentFormat | None
    installed: SelectionSnapshot
    disposition: Disposition
    baseline: SelectionSnapshot | None = None
    separator: bytes = b""

    def __post_init__(self) -> None:
        _validate_claim(self.claim, self.format)
        if type(self.installed) is not SelectionSnapshot or not isinstance(
            self.disposition, Disposition
        ):
            raise TypeError("ownership requires an installed snapshot and disposition")
        if (self.disposition is Disposition.TAKEN_OVER) != (self.baseline is not None):
            raise ValueError("only takeover ownership has a baseline")
        if self.baseline is not None and type(self.baseline) is not SelectionSnapshot:
            raise TypeError("takeover baseline must be a SelectionSnapshot")
        if not isinstance(self.separator, bytes):
            raise TypeError("owned separators must be immutable bytes")
        for snapshot in (self.installed, self.baseline):
            if snapshot is not None and isinstance(snapshot.value, bytes) != (self.format is None):
                raise ValueError("selection snapshot and document format disagree")
            if snapshot is not None and snapshot.layout:
                if self.format is not DocumentFormat.TOML or not isinstance(
                    self.claim.selector, Selector
                ):
                    raise ValueError("restoration layout requires a TOML claim")
                path = tuple(
                    part.name for part in self.claim.selector.parts if isinstance(part, Key)
                )
                for relative, name in layout_headers(snapshot.layout):
                    if header_path(name) != (
                        *path,
                        *(part for part in relative if isinstance(part, str)),
                    ):
                        raise ValueError("TOML restoration header does not match its destination")
        if self.format is not None and self.separator:
            raise ValueError("section separators require a text claim")


@dataclass(frozen=True, slots=True)
class CreatedContainer:
    selector: Selector
    empty_syntax: bytes

    def __post_init__(self) -> None:
        if type(self.selector) is not Selector or not isinstance(self.empty_syntax, bytes):
            raise TypeError("container provenance requires a selector and immutable syntax")


@dataclass(frozen=True, slots=True)
class DocumentProvenance:
    empty_document: bytes | None = None
    containers: tuple[CreatedContainer, ...] = ()
    trailing_separators: tuple[str, ...] = ()
    schema_fields: tuple[OwnedSelection, ...] = ()
    schema_empty_document: bytes | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "schema_fields", tuple(self.schema_fields))
        for item in self.schema_fields:
            if (
                type(item) is not OwnedSelection
                or item.disposition is not Disposition.CREATED
                or not isinstance(item.claim.selector, Selector)
                or len(item.claim.selector.parts) != 1
                or not isinstance(item.claim.selector.parts[0], Key)
                or item.format not in {DocumentFormat.JSON, DocumentFormat.JSONC}
                or not isinstance(item.installed.value, Scalar)
            ):
                raise ValueError("schema provenance requires created top-level scalar fields")
        if len({item.claim for item in self.schema_fields}) != len(self.schema_fields):
            raise ValueError("duplicate schema provenance")
        if self.schema_empty_document is not None and (
            not isinstance(self.schema_empty_document, bytes) or self.schema_empty_document.strip()
        ):
            raise ValueError("schema empty-document evidence must be whitespace bytes")
        if self.empty_document is not None and not isinstance(self.empty_document, bytes):
            raise TypeError("created document evidence must be immutable bytes")
        object.__setattr__(self, "containers", tuple(self.containers))
        object.__setattr__(self, "trailing_separators", tuple(self.trailing_separators))
        if any(type(item) is not CreatedContainer for item in self.containers):
            raise TypeError("container provenance must contain CreatedContainer values")
        if len({item.selector for item in self.containers}) != len(self.containers):
            raise ValueError("duplicate container provenance")
        if any(
            not isinstance(item, str) or re.fullmatch("[0-9a-f]{64}", item) is None
            for item in self.trailing_separators
        ):
            raise ValueError("separator evidence must be SHA-256 digests")


@dataclass(frozen=True, slots=True)
class DocumentRequest:
    claim: Claim
    content: SharedContent | None
    acquisition: Acquisition = Acquisition.CONFLICT

    def __post_init__(self) -> None:
        if not isinstance(self.acquisition, Acquisition):
            raise TypeError("acquisition must be an Acquisition")
        if type(self.claim) is not Claim or self.claim.kind not in {
            Ownership.SECTION,
            Ownership.STRUCTURED,
        }:
            raise ValueError("document requests require section or structured claims")
        if self.content is not None:
            if type(self.content) not in {SectionContent, StructuredContent}:
                raise TypeError("document content must be a section or structured selection")
            if self.claim != content_claim(self.content):
                raise ValueError("requested content must address its physical claim")
        elif self.acquisition is not Acquisition.CONFLICT:
            raise ValueError("retirement does not acquire ownership")

    @classmethod
    def set(
        cls, content: SharedContent, *, acquisition: Acquisition = Acquisition.CONFLICT
    ) -> "DocumentRequest":
        return cls(content_claim(content), content, acquisition)

    @classmethod
    def retire(cls, claim: Claim) -> "DocumentRequest":
        return cls(claim, None)


@dataclass(frozen=True, slots=True)
class EditObservation:
    claim: Claim
    status: EditStatus
    current: SelectionSnapshot | None
    message: str = ""

    def __post_init__(self) -> None:
        if type(self.claim) is not Claim or not isinstance(self.status, EditStatus):
            raise TypeError("edit observations require a claim and status")
        if self.current is not None and type(self.current) is not SelectionSnapshot:
            raise TypeError("observations require an immutable selection snapshot")
        if not isinstance(self.message, str):
            raise TypeError("observation messages must be strings")


@dataclass(frozen=True, slots=True)
class DocumentEdit:
    before: bytes | None
    after: bytes | None
    ownership: tuple[OwnedSelection, ...]
    provenance: DocumentProvenance
    observations: tuple[EditObservation, ...]

    def __post_init__(self) -> None:
        if any(
            item is not None and not isinstance(item, bytes) for item in (self.before, self.after)
        ):
            raise TypeError("document revisions must be immutable bytes")
        object.__setattr__(self, "ownership", tuple(self.ownership))
        object.__setattr__(self, "observations", tuple(self.observations))
        if (
            type(self.provenance) is not DocumentProvenance
            or any(type(item) is not OwnedSelection for item in self.ownership)
            or any(type(item) is not EditObservation for item in self.observations)
        ):
            raise TypeError("document results require immutable ownership and observations")

    @property
    def applicable(self) -> bool:
        return all(
            item.status not in {EditStatus.CONFLICT, EditStatus.MODIFIED}
            for item in self.observations
        )


def content_claim(content: SharedContent) -> Claim:
    if isinstance(content, SectionContent):
        return Claim(Ownership.SECTION, content.selector)
    if isinstance(content, StructuredContent):
        return Claim(Ownership.STRUCTURED, content.selector)
    raise TypeError("document content must be a section or structured selection")


def _validate_claim(claim: Claim, format: DocumentFormat | None) -> None:
    if type(claim) is not Claim:
        raise TypeError("ownership requires a Claim")
    if claim.kind is Ownership.SECTION and format is None:
        return
    if claim.kind is Ownership.STRUCTURED and isinstance(format, DocumentFormat):
        return
    raise ValueError("shared claim and document format disagree")


def _native(value: Value) -> object:
    if isinstance(value, Scalar):
        return value.value
    if isinstance(value, ObjectValue):
        return {key: _native(item) for key, item in value.items}
    return [_native(item) for item in value.items]


def _identity(value: object, member: Member) -> Value | None:
    current = value
    for key in member.key:
        if not isinstance(current, dict) or key not in current:
            return None
        current = current[key]
    result = freeze_value(current)
    return result if not member.key or isinstance(result, Scalar) else None


def _member_index(array: list[object], member: Member) -> int | None:
    seen: set[Value] = set()
    found = None
    for index, item in enumerate(array):
        identity = _identity(item, member)
        if identity is None:
            continue
        if identity in seen:
            raise ValueError("duplicate or ambiguous array member identities")
        seen.add(identity)
        if identity == member.identity:
            found = index
    return found


def _resolve(backend: Backend, selector: Selector) -> PathParts | None:
    path: PathParts = ()
    for part in selector.parts:
        node = backend.native(path)
        if isinstance(part, Key):
            if not isinstance(node, dict):
                raise ValueError("object key selector crosses a non-object")
            if part.name not in node:
                return None
            path += (part.name,)
        else:
            if not isinstance(node, list):
                raise ValueError("member selector requires an array")
            index = _member_index(node, part)
            if index is None:
                return None
            path += (index,)
    return path


def _anchor(array: list[object], index: int, key: tuple[str, ...]) -> Member:
    member = Member(Scalar(None), key)
    identity = _identity(array[index], member)
    result = Member(identity, key) if identity is not None else Member(freeze_value(array[index]))
    if _member_index(array, result) != index:
        raise ValueError("array position has an ambiguous anchor")
    return result


def _array_position(backend: Backend, path: PathParts, member: Member) -> ArrayPosition:
    array = backend.native(path[:-1])
    index = path[-1]
    if not isinstance(array, list) or not isinstance(index, int):
        raise ValueError("array position requires an array member")
    return ArrayPosition(
        _anchor(array, index - 1, member.key) if index else None,
        _anchor(array, index + 1, member.key) if index + 1 < len(array) else None,
        backend.position_prefix(path),
    )


def _structured_snapshot(backend: Backend, selector: Selector) -> SelectionSnapshot | None:
    path = _resolve(backend, selector)
    if path is None:
        return None
    member = selector.parts[-1]
    return SelectionSnapshot(
        freeze_value(backend.native(path)),
        backend.syntax(path),
        backend.owned_syntax(path),
        _array_position(backend, path, member) if isinstance(member, Member) else None,
        backend.layout(path),
    )


def _insertion_index(array: list[object], placement: Placement | None) -> int:
    placement = placement or Placement()
    if placement.position is Position.FIRST:
        return 0
    if placement.position is Position.LAST:
        return len(array)
    if placement.anchor is None:
        raise ValueError("array placement requires an anchor")
    index = _member_index(array, placement.anchor)
    if index is None:
        raise ValueError("array placement anchor is missing")
    return index + (placement.position is Position.AFTER)


def _restoration_index(array: list[object], position: ArrayPosition) -> int:
    previous = _member_index(array, position.previous) if position.previous is not None else None
    following = _member_index(array, position.following) if position.following is not None else None
    if previous is not None and following is not None and previous >= following:
        raise ValueError("foreign array order prevents safe restoration")
    if previous is not None:
        return previous + 1
    if following is not None:
        return following
    if array:
        raise ValueError("original array position has no surviving anchors")
    return 0


def _parents(backend: Backend, selector: Selector, containers: list[CreatedContainer]) -> PathParts:
    path: PathParts = ()
    for index, part in enumerate(selector.parts[:-1]):
        node = backend.native(path)
        if isinstance(part, Key):
            if not isinstance(node, dict):
                raise ValueError("object key selector crosses a non-object")
            path += (part.name,)
            if part.name not in node:
                backend.put(path, {} if isinstance(selector.parts[index + 1], Key) else [])
                containers.append(
                    CreatedContainer(
                        Selector(selector.parts[: index + 1]), backend.empty_syntax(path)
                    )
                )
        else:
            if not isinstance(node, list):
                raise ValueError("member selector requires an array")
            member_index = _member_index(node, part)
            if member_index is None:
                raise ValueError("intermediate array member is missing")
            path += (member_index,)
    return path


def _put_structured(
    backend: Backend, content: StructuredContent, containers: list[CreatedContainer]
) -> None:
    selector = content.selector
    part = selector.parts[-1]
    value = _native(content.value)
    parent = _parents(backend, selector, containers)
    current = backend.native(parent)
    if isinstance(part, Key):
        if not isinstance(current, dict):
            raise ValueError("object key selector crosses a non-object")
        backend.put((*parent, part.name), value)
        return
    if not isinstance(current, list):
        raise ValueError("member selector requires an array")
    if _identity(value, part) != part.identity:
        raise ValueError("generated array member does not match its selector")
    index = _member_index(current, part)
    if index is not None:
        remaining = current[:index] + current[index + 1 :]
        if content.placement is None or _insertion_index(remaining, content.placement) == index:
            backend.put((*parent, index), value)
            return
        backend.relocate((*parent, index), _insertion_index(remaining, content.placement), value)
        return
    array = backend.native(parent)
    if not isinstance(array, list):
        raise ValueError("member selector requires an array")
    insertion = _insertion_index(array, content.placement)
    backend.insert(parent, insertion, value)


def _restore_structured(
    backend: Backend, selector: Selector, baseline: SelectionSnapshot | None
) -> None:
    path = _resolve(backend, selector)
    if path is None:
        raise ValueError("owned selection is missing")
    if baseline is None:
        backend.delete(path)
    elif isinstance(selector.parts[-1], Key):
        if isinstance(baseline.value, bytes):
            raise ValueError("structured baseline requires a typed value")
        backend.put(path, _native(baseline.value), baseline.syntax, layout=baseline.layout)
    else:
        if baseline.position is None or isinstance(baseline.value, bytes):
            raise ValueError("array baseline requires its original position and typed value")
        array = backend.native(path[:-1])
        index = path[-1]
        if not isinstance(array, list) or not isinstance(index, int):
            raise ValueError("member selector requires an array")
        remaining = array[:index] + array[index + 1 :]
        insertion = _restoration_index(remaining, baseline.position)
        if insertion == index:
            backend.put(path, _native(baseline.value), baseline.syntax, layout=baseline.layout)
        else:
            backend.relocate(
                path,
                insertion,
                _native(baseline.value),
                baseline.syntax,
                prefix=baseline.position.prefix,
                layout=baseline.layout,
            )


def _restore_batch(backend: Backend, selections: list[OwnedSelection]) -> None:
    snapshots: dict[int, SelectionSnapshot] = {}
    array_path: PathParts = ()
    array: list[object] = []
    for selection in selections:
        selector = selection.claim.selector
        if not isinstance(selector, Selector) or selection.baseline is None:
            raise ValueError("array restoration requires a baseline and selector")
        path = _resolve(backend, selector)
        if path is None or not isinstance(path[-1], int):
            raise ValueError("owned array member is missing")
        array_path = path[:-1]
        native = backend.native(array_path)
        if not isinstance(native, list):
            raise ValueError("member selector requires an array")
        array = native
        snapshots[path[-1]] = selection.baseline
    edges: dict[int, set[int]] = {index: set() for index in range(len(array))}
    foreign = [index for index in edges if index not in snapshots]
    for left, right in pairwise(foreign):
        edges[right].add(left)
    for index, baseline in snapshots.items():
        position = baseline.position
        if position is None:
            raise ValueError("array baseline requires its original position")
        anchors = [
            _member_index(array, anchor) if anchor is not None else None
            for anchor in (position.previous, position.following)
        ]
        previous, following = anchors
        if previous is None and following is None and len(array) > 1:
            raise ValueError("original array position has no surviving anchors")
        if previous is not None:
            edges[index].add(previous)
        if following is not None:
            edges[following].add(index)
    order: list[int] = []
    while edges:
        ready = next(
            (
                index
                for index in sorted(edges, key=lambda item: (item not in snapshots, item))
                if not edges[index]
            ),
            None,
        )
        if ready is None:
            raise ValueError("foreign array order prevents safe restoration")
        order.append(ready)
        del edges[ready]
        for dependencies in edges.values():
            dependencies.discard(ready)
    current = list(range(len(array)))
    for target, identity in enumerate(order):
        while current[target] != identity and identity not in snapshots:
            path = (*array_path, target)
            value, syntax, layout = backend.native(path), backend.syntax(path), backend.layout(path)
            backend.relocate(path, len(current) - 1, value, syntax, layout=layout)
            current.append(current.pop(target))
        if identity not in snapshots:
            continue
        baseline = snapshots[identity]
        if isinstance(baseline.value, bytes) or baseline.position is None:
            raise ValueError("array baseline requires a typed value and position")
        index = current.index(identity)
        if index == target:
            backend.put(
                (*array_path, index),
                _native(baseline.value),
                baseline.syntax,
                layout=baseline.layout,
            )
        else:
            backend.relocate(
                (*array_path, index),
                target,
                _native(baseline.value),
                baseline.syntax,
                prefix=baseline.position.prefix,
                layout=baseline.layout,
            )
            current.insert(target, current.pop(index))


def _prune(
    backend: Backend, containers: list[CreatedContainer], owned: list[OwnedSelection]
) -> list[CreatedContainer]:
    retained: list[CreatedContainer] = []
    for container in sorted(containers, key=lambda item: len(item.selector.parts), reverse=True):
        path = _resolve(backend, container.selector)
        claim = Claim(Ownership.STRUCTURED, container.selector)
        if path is None:
            continue
        value = backend.native(path)
        if (
            not value
            and isinstance(value, dict | list)
            and backend.empty_syntax(path) == container.empty_syntax
            and not any(claim.overlaps(item.claim) for item in owned)
        ):
            backend.delete(path)
        else:
            retained.append(container)
    return list(reversed(retained))


def _decode(data: bytes | None) -> tuple[str, bytes]:
    if data is not None and not isinstance(data, bytes):
        raise TypeError("document data must be immutable bytes or None")
    bom = b"\xef\xbb\xbf" if data is not None and data.startswith(b"\xef\xbb\xbf") else b""
    text = (data or b"")[len(bom) :].decode("utf-8")
    if "\0" in text:
        raise ValueError("documents cannot contain NUL")
    return text, bom


def _newline(text: str) -> str:
    match = re.search(r"\r\n|\n|\r", text)
    return match[0] if match is not None else "\n"


def _generated_section(content: SectionContent, newline: str) -> str:
    body = re.sub(r"\r\n|\n|\r", lambda _: newline, content.text)
    if body and not body.endswith(newline):
        body += newline
    return content.selector.start + newline + body + content.selector.end + newline


def _sections(
    text: str, boundaries: Iterable[SectionBoundaries]
) -> dict[SectionBoundaries, tuple[int, int]]:
    pairs = set(boundaries)
    for match in re.finditer(r"<!-- flyrail:([^\r\n<>]+):(start|end) -->", text):
        pairs.add(SectionBoundaries.from_marker(match[1]))
    markers: dict[str, tuple[SectionBoundaries, bool]] = {}
    for pair in pairs:
        for line, opening in ((pair.start, True), (pair.end, False)):
            if line in markers:
                raise ValueError("section boundaries overlap")
            markers[line] = (pair, opening)
    spans: dict[SectionBoundaries, tuple[int, int]] = {}
    opened: tuple[SectionBoundaries, int] | None = None
    offset = 0
    for match in re.finditer(r"[^\r\n]*(?:\r\n|\n|\r|$)", text):
        line = match[0]
        if not line:
            continue
        bare = re.sub(r"(?:\r\n|\r|\n)$", "", line)
        if bare in markers:
            pair, opening = markers[bare]
            if opening:
                if opened is not None or pair in spans:
                    raise ValueError("nested or repeated section boundaries")
                opened = (pair, offset)
            else:
                if opened is None or opened[0] != pair:
                    raise ValueError("malformed or intersecting section boundaries")
                spans[pair] = (opened[1], offset + len(line))
                opened = None
        elif "<!-- flyrail:" in bare or any(marker in bare for marker in markers):
            raise ValueError("section boundaries must occupy complete lines")
        offset += len(line)
    if opened is not None:
        raise ValueError("section has no closing boundary")
    return spans


def _section_snapshot(text: str, span: tuple[int, int] | None) -> SelectionSnapshot | None:
    if span is None:
        return None
    raw = text[span[0] : span[1]].encode()
    return SelectionSnapshot(raw, raw, raw)


def _status(
    current: SelectionSnapshot | None,
    desired: bytes | Value | None,
    owned: OwnedSelection | None,
    acquisition: Acquisition,
    format: DocumentFormat | None,
) -> EditStatus:
    if owned is not None:
        if (
            current is None
            or current.value != owned.installed.value
            or (
                format is not DocumentFormat.JSON
                and current.owned_syntax != owned.installed.owned_syntax
            )
        ):
            return EditStatus.MODIFIED
        if desired is None:
            return EditStatus.REMOVED
        return EditStatus.CURRENT if current.value == desired else EditStatus.UPDATED
    if desired is None:
        return EditStatus.CONFLICT
    if current is None:
        return EditStatus.CREATED
    if acquisition is Acquisition.TAKEOVER:
        return EditStatus.TAKEN_OVER
    if acquisition is Acquisition.ADOPT and current.value == desired:
        return EditStatus.ADOPTED
    return EditStatus.CONFLICT


def _ownership(
    request: DocumentRequest,
    format: DocumentFormat | None,
    installed: SelectionSnapshot,
    current: SelectionSnapshot | None,
    previous: OwnedSelection | None,
    status: EditStatus,
    separator: bytes = b"",
) -> OwnedSelection:
    if previous is not None:
        return replace(previous, installed=installed)
    disposition = Disposition(status.value)
    return OwnedSelection(
        request.claim,
        format,
        installed,
        disposition,
        current if disposition is Disposition.TAKEN_OVER else None,
        separator,
    )


def _validate_inputs(
    requests: tuple[DocumentRequest, ...], ownership: tuple[OwnedSelection, ...]
) -> DocumentFormat | None:
    if any(type(item) is not DocumentRequest for item in requests) or any(
        type(item) is not OwnedSelection for item in ownership
    ):
        raise TypeError("document edits require DocumentRequest and OwnedSelection values")
    if len({item.claim for item in requests}) != len(requests) or len(
        {item.claim for item in ownership}
    ) != len(ownership):
        raise ValueError("duplicate document claim")
    claims = list(
        dict.fromkeys([item.claim for item in requests] + [item.claim for item in ownership])
    )
    for index, claim in enumerate(claims):
        if any(claim.overlaps(other) for other in claims[index + 1 :]):
            raise ValueError("document claims overlap")
    formats = {item.format for item in ownership}
    formats.update(
        item.content.format if isinstance(item.content, StructuredContent) else None
        for item in requests
        if item.content is not None
    )
    if len(formats) > 1:
        raise ValueError("one document must use one editor format")
    return next(iter(formats), None)


_EMPTY_PROVENANCE = DocumentProvenance()


def _observe_selections(
    data: bytes | None, ownership: tuple[OwnedSelection, ...]
) -> tuple[EditObservation, ...]:
    format = _validate_inputs((), ownership)
    text, _ = _decode(data)
    snapshots: dict[Claim, SelectionSnapshot | None] = {}
    try:
        if data is None:
            snapshots = {item.claim: None for item in ownership}
        elif format is None:
            spans = _sections(
                text,
                (
                    item.claim.selector
                    for item in ownership
                    if isinstance(item.claim.selector, SectionBoundaries)
                ),
            )
            for item in ownership:
                if isinstance(item.claim.selector, SectionBoundaries):
                    snapshots[item.claim] = _section_snapshot(text, spans.get(item.claim.selector))
        else:
            backend: Backend
            if format is DocumentFormat.TOML:
                from flyrail._toml_document import TomlDocument

                backend = TomlDocument(text)
            else:
                backend = JsonDocument(text, comments=format is DocumentFormat.JSONC)
            for item in ownership:
                if isinstance(item.claim.selector, Selector):
                    snapshots[item.claim] = _structured_snapshot(backend, item.claim.selector)
    except RecursionError as error:
        raise ValueError("document nesting is too deep") from error
    return tuple(
        EditObservation(
            item.claim,
            _status(
                snapshots[item.claim], item.installed.value, item, Acquisition.CONFLICT, format
            ),
            snapshots[item.claim],
        )
        for item in ownership
    )


def edit_document(
    data: bytes | None,
    requests: Iterable[DocumentRequest],
    *,
    ownership: Iterable[OwnedSelection] = (),
    provenance: DocumentProvenance = _EMPTY_PROVENANCE,
) -> DocumentEdit:
    selected = tuple(requests)
    previous = tuple(ownership)
    format = _validate_inputs(selected, previous)
    if type(provenance) is not DocumentProvenance:
        raise TypeError("document provenance must be a DocumentProvenance")
    text, bom = _decode(data)
    if not selected:
        return DocumentEdit(data, data, previous, provenance, ())
    try:
        if format is None:
            return _edit_sections(data, text, bom, selected, previous, provenance)
        return _edit_structured(data, text, bom, format, selected, previous, provenance)
    except RecursionError as error:
        raise ValueError("document nesting is too deep") from error


def _edit_sections(
    data: bytes | None,
    text: str,
    bom: bytes,
    requests: tuple[DocumentRequest, ...],
    previous: tuple[OwnedSelection, ...],
    provenance: DocumentProvenance,
) -> DocumentEdit:
    pairs = [item.claim.selector for item in requests] + [item.claim.selector for item in previous]
    if any(not isinstance(pair, SectionBoundaries) for pair in pairs):
        raise ValueError("retirement requires existing ownership and its document format")
    boundaries = tuple(pair for pair in pairs if isinstance(pair, SectionBoundaries))
    spans = _sections(text, boundaries)
    owned = {item.claim: item for item in previous}
    observations: list[EditObservation] = []
    edits: list[tuple[int, int, str]] = []
    additions: list[tuple[DocumentRequest, str, EditStatus]] = []
    newline = _newline(text)
    for request in requests:
        pair = request.claim.selector
        if not isinstance(pair, SectionBoundaries):
            raise ValueError("text editor requires section boundaries")
        span = spans.get(pair)
        current = _section_snapshot(text, span)
        section_newline = _newline(text[span[0] : span[1]]) if span is not None else newline
        desired = (
            _generated_section(request.content, section_newline)
            if isinstance(request.content, SectionContent)
            else None
        )
        old = owned.get(request.claim)
        status = _status(
            current,
            desired.encode() if desired is not None else None,
            old,
            request.acquisition,
            None,
        )
        observations.append(EditObservation(request.claim, status, current))
        if status in {EditStatus.CONFLICT, EditStatus.MODIFIED}:
            continue
        if status is EditStatus.REMOVED:
            if old is None or span is None:
                raise ValueError("retirement requires existing ownership")
            replacement = old.baseline.syntax.decode() if old.baseline is not None else ""
            start, end = span
            if not replacement and old.separator:
                separator = old.separator.decode()
                separator_start = start - len(separator)
                if text[:start].endswith(separator) and not any(
                    separator_start < other_end and other_start < start
                    for other_start, other_end in spans.values()
                ):
                    start = separator_start
            edits.append((start, end, replacement))
            del owned[request.claim]
        elif desired is not None:
            if span is None:
                additions.append((request, desired, status))
            else:
                if status is EditStatus.UPDATED or status is EditStatus.TAKEN_OVER:
                    edits.append((*span, desired))
                installed = (
                    SelectionSnapshot(desired.encode(), desired.encode(), desired.encode())
                    if status in {EditStatus.UPDATED, EditStatus.TAKEN_OVER}
                    else current
                )
                if installed is None:
                    raise ValueError("selection is missing")
                owned[request.claim] = _ownership(request, None, installed, current, old, status)
    ordered = sorted(edits)
    if any(first[1] > second[0] for first, second in pairwise(ordered)):
        raise ValueError("section edits overlap")
    for start, end, replacement in reversed(ordered):
        text = text[:start] + replacement + text[end:]
    for request, desired, status in additions:
        separator = (
            ""
            if not text or text.endswith(newline * 2)
            else newline
            if text.endswith(newline)
            else newline * 2
        )
        text += separator + desired
        installed = SelectionSnapshot(desired.encode(), desired.encode(), desired.encode())
        owned[request.claim] = _ownership(
            request, None, installed, None, None, status, separator.encode()
        )
    _sections(text, boundaries)
    creation = replace(provenance, empty_document=b"") if data is None and owned else provenance
    return _result(
        data,
        bom + text.encode(),
        previous,
        tuple(owned.values()),
        provenance,
        creation,
        tuple(observations),
    )


def _edit_structured(
    data: bytes | None,
    text: str,
    bom: bytes,
    format: DocumentFormat,
    requests: tuple[DocumentRequest, ...],
    previous: tuple[OwnedSelection, ...],
    provenance: DocumentProvenance,
) -> DocumentEdit:
    empty = ""
    if format is not DocumentFormat.TOML:
        selectors = [item.claim.selector for item in requests] + [
            item.claim.selector for item in previous
        ]
        root_array = any(
            isinstance(selector, Selector) and isinstance(selector.parts[0], Member)
            for selector in selectors
        )
        empty = "[]" if root_array else "{}"
    if data is None:
        text = empty
    backend: Backend
    if format is DocumentFormat.TOML:
        from flyrail._toml_document import TomlDocument

        backend = TomlDocument(text)
    else:
        backend = JsonDocument(text, comments=format is DocumentFormat.JSONC)
    owned = {item.claim: item for item in previous}
    containers = list(provenance.containers)
    observations: list[EditObservation] = []
    preimages = {
        request.claim: _structured_snapshot(backend, request.claim.selector)
        for request in requests
        if isinstance(request.claim.selector, Selector)
    }
    groups: dict[tuple[Key | Member, ...], list[OwnedSelection]] = {}
    for request in requests:
        old = owned.get(request.claim)
        selector = request.claim.selector
        if (
            request.content is None
            and old is not None
            and old.baseline is not None
            and isinstance(selector, Selector)
            and isinstance(selector.parts[-1], Member)
            and _status(preimages[request.claim], None, old, request.acquisition, format)
            is EditStatus.REMOVED
        ):
            groups.setdefault(selector.parts[:-1], []).append(old)
    batched = {item.claim: group for group in groups.values() if len(group) > 1 for item in group}
    retired: set[Claim] = set()
    original_owned = owned.copy()
    for request in requests:
        selector = request.claim.selector
        if not isinstance(selector, Selector):
            raise ValueError("structured editor requires a Selector")
        current = preimages[request.claim]
        content = request.content
        desired = content.value if isinstance(content, StructuredContent) else None
        old = original_owned.get(request.claim)
        status = _status(current, desired, old, request.acquisition, format)
        if status in {EditStatus.CONFLICT, EditStatus.MODIFIED}:
            observations.append(EditObservation(request.claim, status, current))
            continue
        try:
            candidate: Backend
            if (
                status in {EditStatus.CURRENT, EditStatus.ADOPTED}
                and isinstance(content, StructuredContent)
                and content.placement is None
            ):
                candidate = backend
            elif format is DocumentFormat.TOML:
                candidate = TomlDocument(backend.text)
            else:
                candidate = JsonDocument(backend.text, comments=format is DocumentFormat.JSONC)
            if status is EditStatus.REMOVED:
                if old is None:
                    raise ValueError("retirement requires existing ownership")
                if request.claim in batched and request.claim not in retired:
                    group = batched[request.claim]
                    _restore_batch(candidate, group)
                    retired.update(item.claim for item in group)
                elif request.claim not in retired:
                    _restore_structured(candidate, selector, old.baseline)
                del owned[request.claim]
            elif isinstance(content, StructuredContent):
                if (
                    status not in {EditStatus.ADOPTED, EditStatus.CURRENT}
                    or content.placement is not None
                ):
                    before = candidate.text
                    _put_structured(candidate, content, containers)
                    if status is EditStatus.CURRENT and candidate.text != before:
                        status = EditStatus.UPDATED
                installed = _structured_snapshot(candidate, selector)
                if installed is None or installed.value != desired:
                    raise ValueError("published selection differs from the requested value")
                owned[request.claim] = _ownership(request, format, installed, current, old, status)
            backend = candidate
            observations.append(EditObservation(request.claim, status, current))
        except (ValueError, TypeError) as error:
            observations.append(
                EditObservation(request.claim, EditStatus.CONFLICT, current, str(error))
            )
    if any(item.status in {EditStatus.CONFLICT, EditStatus.MODIFIED} for item in observations):
        return DocumentEdit(data, data, previous, provenance, tuple(observations))
    containers = _prune(backend, containers, list(owned.values()))
    separators = list(provenance.trailing_separators)
    if (
        format is DocumentFormat.TOML
        and data is not None
        and text
        and not text.endswith(("\r", "\n"))
        and backend.text.endswith(("\r", "\n"))
    ):
        separators.append(hashlib.sha256(data).hexdigest())
    creation = DocumentProvenance(
        empty.encode() if data is None and owned else provenance.empty_document,
        tuple(containers),
        tuple(dict.fromkeys(separators)),
    )
    return _result(
        data,
        bom + backend.text.encode(),
        previous,
        tuple(owned.values()),
        provenance,
        creation,
        tuple(observations),
    )


def _result(
    before: bytes | None,
    after: bytes,
    previous: tuple[OwnedSelection, ...],
    owned: tuple[OwnedSelection, ...],
    old_provenance: DocumentProvenance,
    provenance: DocumentProvenance,
    observations: tuple[EditObservation, ...],
) -> DocumentEdit:
    if any(item.status in {EditStatus.CONFLICT, EditStatus.MODIFIED} for item in observations):
        return DocumentEdit(before, before, previous, old_provenance, observations)
    if not owned:
        for suffix in (b"\r\n", b"\n", b"\r"):
            if (
                after.endswith(suffix)
                and hashlib.sha256(after[: -len(suffix)]).hexdigest()
                in provenance.trailing_separators
            ):
                after = after[: -len(suffix)]
                break
        result = (
            None
            if provenance.empty_document is not None and after == provenance.empty_document
            else after
        )
        return DocumentEdit(before, result, owned, DocumentProvenance(), observations)
    return DocumentEdit(before, after, owned, provenance, observations)
