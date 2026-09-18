import hashlib
import os
import stat
import sys
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path

from flyrail._validation import portable_path_key, validate_relative_path
from flyrail.content import Key, Member, SectionBoundaries, Selector
from flyrail.values import freeze_value, semantic_bytes, semantic_record


class _UnsafeDestination(ValueError):
    def __init__(self, message: str, path: Path) -> None:
        super().__init__(message)
        self.path = path


def _safe(metadata: os.stat_result, path: Path) -> None:
    if getattr(metadata, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT:
        raise _UnsafeDestination(f"resource reparse points are unsupported: {path}", path)
    if not (stat.S_ISREG(metadata.st_mode) or stat.S_ISDIR(metadata.st_mode)):
        raise _UnsafeDestination(f"resource links and special files are unsupported: {path}", path)
    if stat.S_ISREG(metadata.st_mode) and metadata.st_nlink != 1:
        raise _UnsafeDestination(f"hard-linked resources are ambiguous: {path}", path)


def _physical_directory(path: Path) -> Path:
    if sys.platform != "darwin":
        return path.resolve(strict=True)
    import fcntl

    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        actual = fcntl.fcntl(descriptor, fcntl.F_GETPATH, b"\0" * 1024)
        physical = Path(os.fsdecode(actual.split(b"\0", 1)[0]))
        opened = os.fstat(descriptor)
        current = physical.lstat()
        if (opened.st_dev, opened.st_ino) != (current.st_dev, current.st_ino):
            raise _UnsafeDestination("resource parent changed during canonicalization", path)
        return physical
    finally:
        os.close(descriptor)


def _canonical_child(parent: Path, name: str) -> Path:
    candidate = parent / name
    matches = [
        child
        for child in parent.iterdir()
        if portable_path_key(child.name) == portable_path_key(name)
    ]
    if len(matches) > 1:
        raise _UnsafeDestination("portable resource path collision", candidate)
    if not matches:
        return candidate
    physical = matches[0]
    metadata = physical.lstat()
    _safe(metadata, physical)
    if physical.name != name:
        try:
            alias = candidate.lstat()
        except FileNotFoundError as error:
            raise _UnsafeDestination(
                "resource path has a conflicting portable spelling", candidate
            ) from error
        if (alias.st_dev, alias.st_ino) != (metadata.st_dev, metadata.st_ino):
            raise _UnsafeDestination("resource path alias is ambiguous", candidate)
    return physical


def _management_name(name: str) -> bool:
    folded = portable_path_key(name)
    return folded.startswith(".flyrail-") and folded.endswith(".state")


def _validate_destination(path: Path) -> None:
    for part in path.parts[1:]:
        validate_relative_path(part, "resource path component")
        if _management_name(part):
            raise ValueError("resource destination enters reserved management state")


def canonical_destination(path: str | os.PathLike[str]) -> Path:
    raw = os.fspath(path)
    if not isinstance(raw, str) or not raw or "\0" in raw:
        raise ValueError("resource destination must be a nonempty string path without NUL")
    selected = Path(raw)
    if not selected.is_absolute():
        selected = Path.cwd() / selected
    if ".." in selected.parts or selected == selected.parent:
        raise ValueError("resource destination cannot be a root or contain traversal")
    current = Path(selected.anchor)
    missing: list[str] = []
    _validate_destination(selected)
    for part in selected.parts[1:-1]:
        if missing:
            missing.append(part)
            continue
        candidate = current / part
        try:
            metadata = candidate.lstat()
        except FileNotFoundError:
            missing.append(part)
            continue
        _safe(metadata, candidate)
        if not stat.S_ISDIR(metadata.st_mode):
            raise _UnsafeDestination("resource ancestor must be a directory", candidate)
        current = candidate
    parent = _physical_directory(current)
    if missing:
        first = _canonical_child(parent, missing[0])
        canonical = first.joinpath(*missing[1:], selected.name)
    else:
        canonical = _canonical_child(parent, selected.name)
    _validate_destination(canonical)
    return canonical


def state_path(destination: Path) -> Path:
    key = hashlib.sha256(portable_path_key(destination.name).encode()).hexdigest()
    return destination.parent / f".flyrail-{key}.state"


@dataclass(frozen=True, slots=True, init=False)
class ResourceAuthority:
    destination: Path
    state_root: Path

    def __init__(self, destination: str | os.PathLike[str]) -> None:
        canonical = canonical_destination(destination)
        object.__setattr__(self, "destination", canonical)
        object.__setattr__(self, "state_root", state_path(canonical))

    @property
    def lock_path(self) -> Path:
        return self.state_root / "lock"

    @property
    def ancestor_fences(self) -> tuple[Path, ...]:
        return tuple(
            state_path(parent) for parent in self.destination.parents if parent != parent.parent
        )


class Ownership(StrEnum):
    FILE = "file"
    TREE = "tree"
    SUBTREE = "subtree"
    SECTION = "section"
    STRUCTURED = "structured"


@dataclass(frozen=True, slots=True)
class Claim:
    kind: Ownership
    selector: str | Selector | SectionBoundaries | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.kind, Ownership):
            raise TypeError("claim kind must be an Ownership")
        if self.kind in {Ownership.FILE, Ownership.TREE}:
            if self.selector is not None:
                raise ValueError("whole-resource claims cannot have selectors")
        elif self.kind is Ownership.STRUCTURED:
            if type(self.selector) is not Selector:
                raise TypeError("structured claims require a Selector")
        elif self.kind is Ownership.SECTION:
            if isinstance(self.selector, str):
                object.__setattr__(self, "selector", SectionBoundaries.from_marker(self.selector))
            elif type(self.selector) is not SectionBoundaries:
                raise TypeError("section claims require SectionBoundaries or a marker name")
        else:
            if not isinstance(self.selector, str):
                raise TypeError("subtree claims require a string selector")
            validate_relative_path(self.selector, "subtree selector")

    def overlaps(self, other: "Claim") -> bool:
        if type(other) is not Claim:
            raise TypeError("claims can only be compared with Claim values")
        if self.kind in {Ownership.FILE, Ownership.TREE} or other.kind in {
            Ownership.FILE,
            Ownership.TREE,
        }:
            return True
        if self.kind != other.kind:
            return True
        if isinstance(self.selector, SectionBoundaries) and isinstance(
            other.selector, SectionBoundaries
        ):
            return bool(
                {self.selector.start, self.selector.end}
                & {other.selector.start, other.selector.end}
            )
        if isinstance(self.selector, Selector) and isinstance(other.selector, Selector):
            for left, right in zip(self.selector.parts, other.selector.parts, strict=False):
                if left == right:
                    continue
                if isinstance(left, Key) and isinstance(right, Key):
                    return False
                if isinstance(left, Member) and isinstance(right, Member):
                    return not (left.key == right.key and left.identity != right.identity)
                return True
            return True
        if isinstance(self.selector, str) and isinstance(other.selector, str):
            left_path, right_path = (
                portable_path_key(self.selector),
                portable_path_key(other.selector),
            )
            return (
                left_path == right_path
                or left_path.startswith(right_path + "/")
                or right_path.startswith(left_path + "/")
            )
        raise AssertionError("validated claim selectors disagree")

    def identity(self, authority: ResourceAuthority) -> str:
        selected: object = self.selector
        if isinstance(self.selector, SectionBoundaries):
            selected = [self.selector.start, self.selector.end]
        if isinstance(self.selector, Selector):
            selected = [
                ["key", part.name]
                if isinstance(part, Key)
                else ["member", list(part.key), semantic_record(part.identity)]
                for part in self.selector.parts
            ]
        record = [authority.destination.as_posix(), str(self.kind), selected]
        return hashlib.sha256(
            b"flyrail-claim-v1\0" + semantic_bytes(freeze_value(record))
        ).hexdigest()


def hierarchy_conflicts(authority: ResourceAuthority, *, whole_tree: bool) -> tuple[Path, ...]:
    if not isinstance(whole_tree, bool):
        raise TypeError("whole_tree must be a boolean")
    conflicts = [path for path in authority.ancestor_fences if path.exists() or path.is_symlink()]
    if whole_tree and authority.destination.exists():
        pending = [authority.destination]
        while pending:
            directory = pending.pop()
            metadata = directory.lstat()
            _safe(metadata, directory)
            if not stat.S_ISDIR(metadata.st_mode):
                raise _UnsafeDestination("whole-tree resource must be a directory", directory)
            for child in directory.iterdir():
                if _management_name(child.name):
                    conflicts.append(child)
                    continue
                metadata = child.lstat()
                _safe(metadata, child)
                if stat.S_ISDIR(metadata.st_mode):
                    pending.append(child)
    return tuple(sorted(conflicts))
