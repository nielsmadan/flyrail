from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path

from flyrail._inventory import InventoryEntry
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


@dataclass(frozen=True, slots=True)
class Installation:
    bundle_id: str
    version: str
    content_digest: str
    transaction_id: str
    entries: tuple[InventoryEntry, ...]


@dataclass(frozen=True, slots=True)
class Modification:
    bundle_id: str
    path: str
    kind: ModificationKind


@dataclass(frozen=True, slots=True)
class Conflict:
    path: str
    owner: str | None


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
