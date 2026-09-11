import hashlib
from collections.abc import Iterable
from dataclasses import dataclass

from flyrail.models import BundleEntry


@dataclass(frozen=True, slots=True)
class InventoryEntry:
    path: str
    size: int | None = None
    sha256: str | None = None
    executable: bool = False

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


def inventory_digest(entries: tuple[InventoryEntry, ...]) -> str:
    digest = hashlib.sha256(b"flyrail-inventory-v1\0")
    digest.update(len(entries).to_bytes(8, "big"))
    for entry in sorted(entries, key=lambda entry: entry.path.encode("utf-8")):
        path = entry.path.encode("utf-8")
        digest.update(b"D" if entry.is_directory else b"F")
        digest.update(len(path).to_bytes(8, "big"))
        digest.update(path)
        if entry.size is not None and entry.sha256 is not None:
            digest.update(bytes([entry.executable]))
            digest.update(entry.size.to_bytes(8, "big"))
            digest.update(bytes.fromhex(entry.sha256))
    return digest.hexdigest()
