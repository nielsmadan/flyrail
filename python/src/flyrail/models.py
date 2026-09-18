from collections.abc import Iterable
from dataclasses import dataclass
from enum import StrEnum

from flyrail._validation import (
    portable_path_key,
    validate_identifier,
    validate_relative_path,
    validate_version,
)


@dataclass(frozen=True, slots=True)
class BundleIdentity:
    id: str
    version: str

    def __post_init__(self) -> None:
        validate_identifier(self.id, "bundle id")
        validate_version(self.version)


@dataclass(frozen=True, slots=True, init=False)
class SkillSpec:
    name: str
    path: str
    executables: tuple[str, ...]

    def __init__(self, name: str, path: str, executables: Iterable[str] = ()) -> None:
        validate_identifier(name, "skill name")
        validate_relative_path(path, "skill path")
        if path.rsplit("/", 1)[-1] != name:
            raise ValueError("skill name must match its containing directory")
        if isinstance(executables, str | bytes):
            raise TypeError("executables must be an iterable of relative paths")
        snapshot = tuple(executables)
        seen: set[str] = set()
        for executable in snapshot:
            validate_relative_path(executable, "executable path")
            key = portable_path_key(executable)
            if key in seen:
                raise ValueError("executable paths must be unique across portable filesystems")
            seen.add(key)
        object.__setattr__(self, "name", name)
        object.__setattr__(self, "path", path)
        object.__setattr__(self, "executables", snapshot)


@dataclass(frozen=True, slots=True)
class BundleEntry:
    path: str
    data: bytes | None = None
    executable: bool = False

    def __post_init__(self) -> None:
        validate_relative_path(self.path, "entry path")
        if self.data is not None and not isinstance(self.data, bytes):
            raise TypeError("entry data must be bytes or None")
        if not isinstance(self.executable, bool):
            raise TypeError("entry executable must be a boolean")
        if self.is_directory and self.executable:
            raise ValueError("a directory cannot have executable file intent")

    @property
    def is_directory(self) -> bool:
        return self.data is None


class OperationStatus(StrEnum):
    APPLIED = "applied"
    UNCHANGED = "unchanged"
    FAILED = "failed"
    PARTIAL = "partial"
    INCOMPLETE = "incomplete"
