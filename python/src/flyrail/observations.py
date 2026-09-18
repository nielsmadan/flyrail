from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path

from flyrail._inventory import InventoryEntry
from flyrail._validation import validate_identifier, validate_version
from flyrail.models import OperationStatus
from flyrail.targets import Target


class ObservationState(StrEnum):
    ABSENT = "absent"
    INSTALLED = "installed"
    RECOVERY_NEEDED = "recovery_needed"
    UNKNOWN = "unknown"


class ErrorCode(StrEnum):
    BUSY = "busy"
    CONFLICT = "conflict"
    MODIFIED = "modified"
    UPDATE_REQUIRED = "update_required"
    UNSUPPORTED = "unsupported"
    IO_ERROR = "io_error"
    UNSAFE_PATH = "unsafe_path"
    INVALID_STATE = "invalid_state"
    CONCURRENT_CHANGE = "concurrent_change"
    RECOVERY_NEEDED = "recovery_needed"


class ModificationKind(StrEnum):
    MISSING = "missing"
    ADDED = "added"
    TYPE_CHANGED = "type_changed"
    CONTENT_CHANGED = "content_changed"
    EXECUTABLE_CHANGED = "executable_changed"


@dataclass(frozen=True, slots=True)
class TargetError:
    code: ErrorCode
    message: str
    path: Path | None = None
    errno: int | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.code, ErrorCode) or not isinstance(self.message, str):
            raise TypeError("errors require a code and immutable message")
        if self.path is not None and type(self.path) is not type(Path()):
            raise TypeError("error paths must be concrete paths")
        if self.errno is not None and type(self.errno) is not int:
            raise TypeError("errno must be an integer")


@dataclass(frozen=True, slots=True)
class Installation:
    bundle_id: str
    version: str
    content_digest: str
    transaction_id: str
    entries: tuple[InventoryEntry, ...]

    def __post_init__(self) -> None:
        validate_identifier(self.bundle_id, "bundle id")
        validate_version(self.version)
        if not isinstance(self.content_digest, str) or not isinstance(self.transaction_id, str):
            raise TypeError("installation identities must be strings")
        entries = tuple(self.entries)
        if any(type(item) is not InventoryEntry for item in entries):
            raise TypeError("installation entries must be immutable inventory values")
        object.__setattr__(self, "entries", entries)


@dataclass(frozen=True, slots=True)
class Modification:
    bundle_id: str
    path: str
    kind: ModificationKind

    def __post_init__(self) -> None:
        validate_identifier(self.bundle_id, "bundle id")
        if not isinstance(self.path, str) or not isinstance(self.kind, ModificationKind):
            raise TypeError("modifications require an immutable path and kind")


@dataclass(frozen=True, slots=True)
class Conflict:
    path: str
    owner: str | None

    def __post_init__(self) -> None:
        if not isinstance(self.path, str) or (
            self.owner is not None and not isinstance(self.owner, str)
        ):
            raise TypeError("conflict identity must be immutable")


@dataclass(frozen=True, slots=True)
class Observation:
    state: ObservationState
    installed: Installation | None = None
    installations: tuple[Installation, ...] = ()
    version_matches: bool | None = None
    recorded_content_matches: bool | None = None
    content_matches: bool | None = None
    modifications: tuple[Modification, ...] = ()
    conflicts: tuple[Conflict, ...] = ()
    error: TargetError | None = None
    recovery_paths: tuple[Path, ...] = ()

    def __post_init__(self) -> None:
        if not isinstance(self.state, ObservationState) or (
            self.installed is not None and type(self.installed) is not Installation
        ):
            raise TypeError("observation requires immutable installation state")
        if self.error is not None and type(self.error) is not TargetError:
            raise TypeError("observation error must be a TargetError")
        if any(
            value is not None and type(value) is not bool
            for value in (self.version_matches, self.recorded_content_matches, self.content_matches)
        ):
            raise TypeError("observation comparisons must be boolean or absent")
        for name, cls in (
            ("installations", Installation),
            ("modifications", Modification),
            ("conflicts", Conflict),
            ("recovery_paths", type(Path())),
        ):
            values = tuple(getattr(self, name))
            if any(type(item) is not cls for item in values):
                raise TypeError("observation members must be immutable typed values")
            object.__setattr__(self, name, values)

    @property
    def is_current(self) -> bool:
        return (
            self.state is ObservationState.INSTALLED
            and self.error is None
            and self.installed is not None
            and self.version_matches is True
            and self.recorded_content_matches is True
            and self.content_matches is True
            and not self.conflicts
            and not any(
                change.bundle_id == self.installed.bundle_id for change in self.modifications
            )
        )


@dataclass(frozen=True, slots=True)
class TargetInspection:
    target: Target
    root: Path
    state_root: Path
    alias_of: int | None
    observation: Observation

    def __post_init__(self) -> None:
        _target_value(self.target, self.root, self.state_root, self.alias_of, self.observation)


@dataclass(frozen=True, slots=True)
class TargetResult:
    target: Target
    root: Path
    state_root: Path
    alias_of: int | None
    status: OperationStatus
    observation: Observation
    error: TargetError | None = None
    recovery_paths: tuple[Path, ...] = ()

    def __post_init__(self) -> None:
        _target_value(self.target, self.root, self.state_root, self.alias_of, self.observation)
        if not isinstance(self.status, OperationStatus) or (
            self.error is not None and type(self.error) is not TargetError
        ):
            raise TypeError("target results require immutable status and error")
        paths = tuple(self.recovery_paths)
        if any(type(path) is not type(Path()) for path in paths):
            raise TypeError("recovery paths must be concrete paths")
        object.__setattr__(self, "recovery_paths", paths)


def _target_value(
    target: Target, root: Path, state_root: Path, alias: int | None, observation: Observation
) -> None:
    if type(target) is not Target or type(observation) is not Observation:
        raise TypeError("target summaries require an immutable target and observation")
    if any(type(path) is not type(Path()) for path in (root, state_root)):
        raise TypeError("target summary paths must be concrete paths")
    if alias is not None and (type(alias) is not int or alias < 0):
        raise ValueError("target alias must be a nonnegative index")
