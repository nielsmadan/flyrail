import hashlib
from collections.abc import Iterable
from dataclasses import dataclass

from flyrail._validation import validate_relative_path
from flyrail.models import BundleEntry


@dataclass(frozen=True, slots=True)
class InventoryEntry:
    path: str
    size: int | None = None
    sha256: str | None = None
    executable: bool = False

    def __post_init__(self) -> None:
        validate_relative_path(self.path, "inventory path")
        if (
            (self.size is not None and (type(self.size) is not int or self.size < 0))
            or (self.sha256 is not None and not isinstance(self.sha256, str))
            or type(self.executable) is not bool
        ):
            raise TypeError("inventory metadata must be immutable scalar values")

    @property
    def is_directory(self) -> bool:
        return self.size is None


def inventory(entries: Iterable[BundleEntry]) -> tuple[InventoryEntry, ...]:
    return tuple(
        InventoryEntry(
            entry.path,
            None if entry.data is None else len(entry.data),
            None if entry.data is None else hashlib.sha256(entry.data).hexdigest(),
            entry.executable,
        )
        for entry in sorted(entries, key=lambda entry: entry.path.encode("utf-8"))
    )
