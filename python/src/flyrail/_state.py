import hashlib
import os
import re
import stat
import sys
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import TypeVar

from flyrail._codec import decode, encode
from flyrail._observation import ObservationFailure
from flyrail._resource_models import ResourceRef
from flyrail._validation import portable_path_key
from flyrail.authority import (
    ResourceAuthority,
    _management_name,
    _physical_directory,
    _safe,
    _UnsafeDestination,
    transaction_path,
)
from flyrail.observations import ErrorCode
from flyrail.stores import StateStore, StoreBusy

T = TypeVar("T")
_DEVICE_NUMBER = re.compile(r"[0-9]+:[0-9]+")


def index_key(bundle_id: str, context_digest: str) -> str:
    return f"index/{bundle_id}/{context_digest}"


def destination_key(destination: Path) -> str:
    return hashlib.sha256(portable_path_key(destination.as_posix()).encode()).hexdigest()


def resource_key(destination: Path, record: str) -> str:
    return f"resource/{destination_key(destination)}/{record}"


def physical_location(path: Path) -> Path:
    resolved = path.resolve()
    existing = resolved
    while not existing.is_dir():
        existing = existing.parent
    return _physical_directory(existing).joinpath(*resolved.parts[len(existing.parts) :])


def _device(path: Path) -> int:
    for candidate in (path, *path.parents):
        try:
            return os.stat(candidate).st_dev
        except FileNotFoundError:
            continue
    raise FileNotFoundError(path)


def _mount_point(field: str) -> Path:
    return Path(re.sub(r"\\([0-7]{3})", lambda match: chr(int(match[1], 8)), field))


def _mount_id(entries: list[tuple[int, Path]], path: Path) -> int | None:
    selected: tuple[int, int] | None = None
    for identifier, point in entries:
        if path.parts[: len(point.parts)] == point.parts and (
            selected is None or len(point.parts) >= selected[1]
        ):
            selected = (identifier, len(point.parts))
    return None if selected is None else selected[0]


def shared_mount(mountinfo: str, first: Path, second: Path) -> bool:
    entries = []
    for line in mountinfo.splitlines():
        fields = line.split(" ")
        if (
            len(fields) < 10
            or "-" not in fields[6:]
            or not (fields[0].isdigit() and fields[1].isdigit())
            or _DEVICE_NUMBER.fullmatch(fields[2]) is None
            or not fields[4].startswith("/")
        ):
            return False
        entries.append((int(fields[0]), _mount_point(fields[4])))
    mount = _mount_id(entries, first)
    return mount is not None and mount == _mount_id(entries, second)


def _mountinfo() -> str | None:
    if sys.platform != "linux":
        return None
    try:
        return os.fsdecode(Path("/proc/self/mountinfo").read_bytes())
    except OSError:
        return ""


def _same_mount(first: Path, second: Path) -> bool:
    if _device(first) != _device(second):
        return False
    mountinfo = _mountinfo()
    return mountinfo is None or shared_mount(mountinfo, first, second)


def _staged(staging: Path, destination: Path, transaction_id: str) -> Path:
    return physical_location(staging) / f"{destination_key(destination)}-{transaction_id}"


def choose_transaction_directory(store: StateStore, destination: Path, transaction_id: str) -> Path:
    fallback = transaction_path(destination, transaction_id)
    staging = store.staging_directory
    if staging is None:
        return fallback
    try:
        selected = _staged(staging, destination, transaction_id)
        if not _same_mount(selected.parent, destination.parent):
            return fallback
    except (OSError, _UnsafeDestination):
        return fallback
    return selected


def recognized_transaction_directory(
    store: StateStore, destination: Path, transaction_id: str, directory: Path
) -> bool:
    if directory == transaction_path(destination, transaction_id):
        return True
    staging = store.staging_directory
    return staging is not None and directory == _staged(staging, destination, transaction_id)


def read_record(store: StateStore, key: str, cls: type[T]) -> T | None:
    data = store.read(key)
    if data is None:
        return None
    try:
        value = decode(data)
    except (ValueError, UnicodeError) as error:
        raise ObservationFailure(
            ErrorCode.INVALID_STATE, f"{key}: {error}", store.location
        ) from error
    if type(value) is not cls:
        raise ObservationFailure(
            ErrorCode.INVALID_STATE, f"{key}: record has the wrong type", store.location
        )
    if encode(value) != data:
        raise ObservationFailure(
            ErrorCode.INVALID_STATE, f"{key}: record is not canonical", store.location
        )
    return value


def write_record(store: StateStore, key: str, value: object, *, expected: object | None) -> None:
    if not store.write(key, encode(value), expected=None if expected is None else encode(expected)):
        raise ObservationFailure(
            ErrorCode.CONCURRENT_CHANGE, f"{key}: state changed", store.location
        )


def delete_record(store: StateStore, key: str, *, expected: object) -> None:
    if not store.delete(key, expected=encode(expected)):
        raise ObservationFailure(
            ErrorCode.CONCURRENT_CHANGE, f"{key}: state changed", store.location
        )


@contextmanager
def store_lock(store: StateStore, name: str, timeout: float) -> Iterator[None]:
    try:
        with store.lock(name, timeout):
            yield
    except StoreBusy as error:
        raise ObservationFailure(ErrorCode.BUSY, str(error), store.location) from error


def _transaction_entries(destination: Path) -> list[Path]:
    entries = []
    pending = [destination]
    while pending:
        directory = pending.pop()
        metadata = directory.lstat()
        _safe(metadata, directory)
        if not stat.S_ISDIR(metadata.st_mode):
            raise _UnsafeDestination("whole-tree resource must be a directory", directory)
        for child in directory.iterdir():
            if _management_name(child.name):
                entries.append(child)
                continue
            metadata = child.lstat()
            _safe(metadata, child)
            if stat.S_ISDIR(metadata.st_mode):
                pending.append(child)
    return entries


def hierarchy_conflicts(
    store: StateStore, authority: ResourceAuthority, *, whole_tree: bool
) -> tuple[Path, ...]:
    if not isinstance(whole_tree, bool):
        raise TypeError("whole_tree must be a boolean")
    destination = authority.destination
    conflicts = [
        parent
        for parent in destination.parents
        if parent != parent.parent and store.read(resource_key(parent, "authority")) is not None
    ]
    if whole_tree:
        for key in store.keys("resource/"):
            if key.endswith("/authority"):
                ref = read_record(store, key, ResourceRef)
                if ref is not None and destination in ref.destination.parents:
                    conflicts.append(ref.destination)
        if destination.exists():
            conflicts.extend(_transaction_entries(destination))
    return tuple(sorted(set(conflicts)))
