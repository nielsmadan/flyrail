import hashlib
import re
from dataclasses import dataclass, field
from enum import StrEnum
from pathlib import Path
from typing import TypeVar

from flyrail._codec import encode, register
from flyrail._validation import validate_identifier, validate_relative_path, validate_version
from flyrail.authority import Claim, Ownership, ResourceAuthority, state_path
from flyrail.content import DocumentSchema
from flyrail.editors import Disposition, DocumentProvenance, OwnedSelection, content_claim

T = TypeVar("T")


def sequence(items: tuple[T, ...], cls: type[T]) -> tuple[T, ...]:
    result = tuple(items)
    if any(type(item) is not cls for item in result):
        raise TypeError(f"expected {cls.__name__} values")
    return result


def digest(value: object) -> str:
    return hashlib.sha256(encode(value)).hexdigest()


def hexadecimal(value: str, length: int = 64) -> None:
    if not isinstance(value, str) or re.fullmatch(f"[0-9a-f]{{{length}}}", value) is None:
        raise ValueError("invalid hexadecimal identity")


def absolute(path: Path) -> None:
    if type(path) is not type(Path()) or not path.is_absolute() or ".." in path.parts:
        raise ValueError("expected an absolute concrete path")
    if path == path.parent:
        raise ValueError("filesystem roots are not resources")


class ResourceKind(StrEnum):
    DOCUMENT = "document"
    TREE = "tree"
    SKILL_CONTAINER = "skill-container"


@dataclass(frozen=True, slots=True)
class Ancestor:
    path: Path
    device: int
    inode: int
    mode: int
    uid: int
    gid: int
    security: bytes = b""

    def __post_init__(self) -> None:
        if type(self.path) is not type(Path()) or not self.path.is_absolute():
            raise ValueError("ancestor path must be absolute")
        if (
            any(
                type(item) is not int or item < 0
                for item in (self.device, self.inode, self.mode, self.uid, self.gid)
            )
            or self.mode > 0o7777
        ):
            raise ValueError("invalid ancestor metadata")
        if not isinstance(self.security, bytes):
            raise TypeError("ancestor security must be immutable")


@dataclass(frozen=True, slots=True)
class Node:
    path: str
    data: bytes | None
    mode: int
    uid: int
    gid: int
    device: int
    inode: int
    security: bytes = b""

    def __post_init__(self) -> None:
        if self.path:
            validate_relative_path(self.path, "revision path")
        elif self.path != "":
            raise TypeError("revision path must be a string")
        if self.data is not None and not isinstance(self.data, bytes):
            raise TypeError("revision bytes must be immutable")
        if (
            any(
                type(item) is not int or item < 0
                for item in (self.mode, self.uid, self.gid, self.device, self.inode)
            )
            or self.mode > 0o777
        ):
            raise ValueError("invalid revision metadata")
        if not isinstance(self.security, bytes):
            raise TypeError("security metadata must be immutable")


@dataclass(frozen=True, slots=True)
class Revision:
    nodes: tuple[Node, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "nodes", sequence(self.nodes, Node))
        by_path = {node.path: node for node in self.nodes}
        if len(by_path) != len(self.nodes) or (self.nodes and self.nodes[0].path):
            raise ValueError("revision must have exactly one root")
        for node in self.nodes[1:]:
            parent = by_path.get(node.path.rpartition("/")[0])
            if parent is None or parent.data is not None:
                raise ValueError("revision parents must be directories")
        if tuple(sorted(by_path)) != tuple(node.path for node in self.nodes):
            raise ValueError("revision entries must be ordered")

    @property
    def data(self) -> bytes | None:
        return self.nodes[0].data if self.nodes else None

    @property
    def content_digest(self) -> str:
        return digest(tuple((node.path, node.data, node.mode) for node in self.nodes))


@dataclass(frozen=True, slots=True)
class ResourceRef:
    destination: Path
    kind: ResourceKind
    state_root: Path
    schema_version: int = 2

    def __post_init__(self) -> None:
        if type(self.schema_version) is not int or self.schema_version != 2:
            raise ValueError("authority schema must be 2")
        absolute(self.destination)
        if not isinstance(self.kind, ResourceKind):
            raise TypeError("resource kind must be a ResourceKind")
        if self.state_root != state_path(self.destination):
            raise ValueError("resource state path disagrees with destination")

    @property
    def authority(self) -> ResourceAuthority:
        result = ResourceAuthority(self.destination)
        if result.destination != self.destination:
            raise ValueError("resource canonical destination changed")
        return result


@dataclass(frozen=True, slots=True)
class OwnedClaim:
    claim: Claim
    claim_id: str
    bundle_id: str
    artifact_id: str
    version: str
    bundle_digest: str
    render_digest: str
    selection: OwnedSelection | None = None
    installed: Revision | None = None
    disposition: Disposition = Disposition.CREATED
    baseline: Revision | None = None
    requirements: tuple[tuple[Path, str], ...] = ()
    stable_requirements: tuple[tuple[Path, str], ...] = ()
    schema_requirements: tuple[DocumentSchema, ...] = ()

    def __post_init__(self) -> None:
        if type(self.claim) is not Claim or not isinstance(self.disposition, Disposition):
            raise TypeError("ownership requires a claim and disposition")
        hexadecimal(self.claim_id)
        validate_identifier(self.bundle_id, "owner")
        validate_identifier(self.artifact_id, "artifact")
        validate_version(self.version)
        hexadecimal(self.bundle_digest)
        hexadecimal(self.render_digest)
        pairs = tuple((path, identifier) for path, identifier in self.requirements)
        for path, identifier in pairs:
            absolute(path)
            hexadecimal(identifier)
        if len(set(pairs)) != len(pairs):
            raise ValueError("duplicate claim requirement")
        object.__setattr__(self, "requirements", pairs)
        stable = tuple((path, identifier) for path, identifier in self.stable_requirements)
        if len(set(stable)) != len(stable) or not set(stable) <= set(pairs):
            raise ValueError("stable requirements must be a unique subset of requirements")
        object.__setattr__(self, "stable_requirements", stable)
        object.__setattr__(
            self, "schema_requirements", sequence(self.schema_requirements, DocumentSchema)
        )
        if len({item.key for item in self.schema_requirements}) != len(self.schema_requirements):
            raise ValueError("duplicate claim schema requirement")
        if self.schema_requirements and (
            self.selection is None
            or any(item.format is not self.selection.format for item in self.schema_requirements)
        ):
            raise ValueError("schema requirements need matching document selections")
        if self.claim.kind in {Ownership.SECTION, Ownership.STRUCTURED}:
            if type(self.selection) is not OwnedSelection or self.selection.claim != self.claim:
                raise ValueError("shared claims require matching selection ownership")
            if self.installed is not None or self.baseline is not None:
                raise ValueError("shared claims cannot own complete revisions")
            if self.disposition != self.selection.disposition:
                raise ValueError("selection disposition disagrees")
        else:
            if self.selection is not None or type(self.installed) is not Revision:
                raise ValueError("whole claims require an installed revision")
            if not self.installed.nodes:
                raise ValueError("owned revisions cannot be absent")
            if (self.disposition is Disposition.TAKEN_OVER) != (self.baseline is not None):
                raise ValueError("only takeover has a baseline")
            if self.baseline is not None and (
                type(self.baseline) is not Revision or not self.baseline.nodes
            ):
                raise ValueError("takeover needs an original revision")


@dataclass(frozen=True, slots=True)
class Receipt:
    resource: ResourceRef
    transaction_id: str
    claims: tuple[OwnedClaim, ...] = ()
    provenance: DocumentProvenance = field(default_factory=DocumentProvenance)
    created_directories: tuple[Node, ...] = ()
    schema_version: int = 2

    def __post_init__(self) -> None:
        if type(self.schema_version) is not int or self.schema_version != 2:
            raise ValueError("receipt schema must be 2")
        if (
            type(self.resource) is not ResourceRef
            or type(self.provenance) is not DocumentProvenance
        ):
            raise TypeError("invalid receipt resource or provenance")
        hexadecimal(self.transaction_id, 32)
        object.__setattr__(self, "claims", sequence(self.claims, OwnedClaim))
        object.__setattr__(self, "created_directories", sequence(self.created_directories, Node))
        if self.created_directories and self.resource.kind is not ResourceKind.SKILL_CONTAINER:
            raise ValueError("directory provenance requires a skill container")
        if any(node.data is not None for node in self.created_directories) or len(
            {node.path for node in self.created_directories}
        ) != len(self.created_directories):
            raise ValueError("created directory evidence must contain distinct directories")
        authority = object.__new__(ResourceAuthority)
        object.__setattr__(authority, "destination", self.resource.destination)
        object.__setattr__(authority, "state_root", self.resource.state_root)
        for index, owned in enumerate(self.claims):
            if owned.claim_id != owned.claim.identity(authority):
                raise ValueError("claim identity disagrees with physical destination")
            if any(owned.claim.overlaps(other.claim) for other in self.claims[:index]):
                raise ValueError("receipt claims overlap")
            allowed = {
                ResourceKind.DOCUMENT: {Ownership.FILE, Ownership.SECTION, Ownership.STRUCTURED},
                ResourceKind.TREE: {Ownership.TREE},
                ResourceKind.SKILL_CONTAINER: {Ownership.SUBTREE},
            }
            if owned.claim.kind not in allowed[self.resource.kind]:
                raise ValueError("claim kind disagrees with resource kind")
        schemas = {schema for owned in self.claims for schema in owned.schema_requirements}
        if len({schema.key for schema in schemas}) != len(schemas):
            raise ValueError("receipt has incompatible schema requirements")
        for schema in schemas:
            if any(content_claim(schema.content).overlaps(owned.claim) for owned in self.claims):
                raise ValueError("schema requirement overlaps app ownership")


@dataclass(frozen=True, slots=True)
class Generation:
    version: str
    bundle_digest: str
    render_digest: str
    membership: tuple[tuple[ResourceRef, tuple[str, ...]], ...]

    def __post_init__(self) -> None:
        validate_version(self.version)
        hexadecimal(self.bundle_digest)
        hexadecimal(self.render_digest)
        pairs = tuple((ref, tuple(ids)) for ref, ids in self.membership)
        for ref, ids in pairs:
            if type(ref) is not ResourceRef:
                raise TypeError("generation requires resource references")
            for identifier in ids:
                hexadecimal(identifier)
            if len(set(ids)) != len(ids):
                raise ValueError("duplicate membership claim")
        if len({ref.destination for ref, _ in pairs}) != len(pairs):
            raise ValueError("duplicate generation resource")
        object.__setattr__(self, "membership", pairs)


@dataclass(frozen=True, slots=True)
class Index:
    bundle_id: str
    context_digest: str
    current: Generation | None = None
    pending: Generation | None = None
    previous: Generation | None = None
    residual: tuple[ResourceRef, ...] = ()
    schema_version: int = 2

    def __post_init__(self) -> None:
        validate_identifier(self.bundle_id, "index owner")
        hexadecimal(self.context_digest)
        if type(self.schema_version) is not int or self.schema_version != 2:
            raise ValueError("index schema must be 2")
        for generation in (self.current, self.pending, self.previous):
            if generation is not None and type(generation) is not Generation:
                raise TypeError("index requires generation records")
        object.__setattr__(self, "residual", sequence(self.residual, ResourceRef))
        refs = self.resources
        if len({ref.destination for ref in refs}) != len(refs):
            raise ValueError("index has conflicting resource kinds")

    @property
    def resources(self) -> tuple[ResourceRef, ...]:
        refs = set(self.residual)
        for generation in (self.current, self.pending, self.previous):
            if generation is not None:
                refs.update(ref for ref, _ in generation.membership)
        return tuple(sorted(refs, key=lambda ref: str(ref.state_root)))


for _cls in (
    ResourceKind,
    Ancestor,
    Node,
    Revision,
    ResourceRef,
    OwnedClaim,
    Receipt,
    Generation,
    Index,
):
    register(_cls)
