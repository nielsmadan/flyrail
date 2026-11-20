import hashlib
import stat
from dataclasses import dataclass
from pathlib import Path

from flyrail._observation import ObservationFailure, Observer
from flyrail._resource_io import ancestors, failure
from flyrail.observations import ErrorCode

_MAX_INPUT_BYTES = 8 * 1024 * 1024


@dataclass(frozen=True, slots=True)
class ReadPrecondition:
    path: Path
    resolved: Path
    identity: tuple[int, ...] | None
    digest: bytes | None

    def require(self) -> None:
        try:
            current, _ = read_input(self.path, expected=self.resolved)
        except (OSError, ObservationFailure, RuntimeError, ValueError) as error:
            raise self.changed() from error
        if current != self:
            raise self.changed()

    def changed(self) -> ObservationFailure:
        return failure(ErrorCode.CONCURRENT_CHANGE, "Loadout routing input changed", self.path)

    def require_binding(self) -> None:
        try:
            physical = self.path.resolve()
        except (OSError, RuntimeError) as error:
            raise self.changed() from error
        if physical != self.resolved:
            raise self.changed()


def read_input(
    path: Path, *, expected: Path | None = None
) -> tuple[ReadPrecondition, bytes | None]:
    physical = path.resolve()
    if expected is not None and physical != expected:
        raise failure(ErrorCode.CONCURRENT_CHANGE, "Loadout input path changed", path)
    parents = ancestors(physical)
    reader = Observer()
    info = reader.metadata(physical)
    identity: tuple[int, ...] | None = None
    data = None
    if info is None:
        try:
            path.lstat()
        except FileNotFoundError:
            pass
        else:
            raise failure(ErrorCode.UNSAFE_PATH, "dangling Loadout input", path)
    else:
        identity = (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_gid)
        if stat.S_ISREG(info.st_mode):
            if info.st_size > _MAX_INPUT_BYTES:
                raise failure(ErrorCode.INVALID_STATE, "Loadout input is too large", path)
            data = reader.file(physical, limit=_MAX_INPUT_BYTES)
            identity += (info.st_size, info.st_mtime_ns)
    reader.finish()
    if path.resolve() != physical or ancestors(physical) != parents:
        raise failure(ErrorCode.CONCURRENT_CHANGE, "Loadout input path changed", path)
    return ReadPrecondition(
        path, physical, identity, None if data is None else hashlib.sha256(data).digest()
    ), data
