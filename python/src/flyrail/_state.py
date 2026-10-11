import hashlib
import stat
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import TypeVar

from flyrail._codec import decode, encode
from flyrail._observation import ObservationFailure
from flyrail._resource_models import ResourceRef
from flyrail._validation import portable_path_key
from flyrail.authority import ResourceAuthority, _management_name, _safe, _UnsafeDestination
from flyrail.observations import ErrorCode
from flyrail.stores import StateStore, StoreBusy

T = TypeVar("T")


def index_key(bundle_id: str, context_digest: str) -> str:
    return f"index/{bundle_id}/{context_digest}"


def destination_key(destination: Path) -> str:
    return hashlib.sha256(portable_path_key(destination.as_posix()).encode()).hexdigest()


def resource_key(destination: Path, record: str) -> str:
    return f"resource/{destination_key(destination)}/{record}"


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
