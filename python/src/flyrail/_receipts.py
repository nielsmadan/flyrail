import json
import re
from dataclasses import dataclass

from flyrail._inventory import InventoryEntry, inventory, inventory_digest
from flyrail._manifest import _object, _reject_constant, _string, _unique_object
from flyrail._validation import (
    portable_path_key,
    validate_identifier,
    validate_relative_path,
    validate_version,
)
from flyrail.bundle import Bundle


@dataclass(frozen=True, slots=True)
class Receipt:
    bundle_id: str
    transaction_id: str
    version: str | None
    content_digest: str | None
    entries: tuple[InventoryEntry, ...]

    @property
    def removed(self) -> bool:
        return self.version is None

    @property
    def skills(self) -> tuple[str, ...]:
        return tuple(entry.path for entry in self.entries if "/" not in entry.path)


def _hex(value: object, length: int, label: str) -> str:
    result = _string(value, label)
    if re.fullmatch(f"[0-9a-f]{{{length}}}", result) is None:
        raise ValueError(f"{label} must be {length} lowercase hexadecimal characters")
    return result


def _entry(value: object) -> InventoryEntry:
    fields = _object(value, {"path", "kind"}, {"size", "sha256", "executable"})
    path = _string(fields["path"], "inventory path")
    validate_relative_path(path, "inventory path")
    if fields["kind"] == "directory":
        if fields.keys() != {"path", "kind"}:
            raise ValueError("directory inventory entries contain only path and kind")
        return InventoryEntry(path)
    if fields["kind"] != "file" or fields.keys() != {
        "path",
        "kind",
        "size",
        "sha256",
        "executable",
    }:
        raise ValueError("file inventory entries require path, kind, size, sha256 and executable")
    size = fields["size"]
    executable = fields["executable"]
    if type(size) is not int or not 0 <= size < 2**64:
        raise ValueError("inventory size must be an unsigned 64-bit integer")
    if not isinstance(executable, bool):
        raise ValueError("inventory executable must be a boolean")
    return InventoryEntry(path, size, _hex(fields["sha256"], 64, "file SHA-256"), executable)


def _validate_tree(entries: tuple[InventoryEntry, ...]) -> None:
    by_path = {entry.path: entry for entry in entries}
    keys = {portable_path_key(entry.path) for entry in entries}
    if len(keys) != len(entries):
        raise ValueError("inventory contains duplicate or portable-colliding paths")
    roots: list[str] = []
    for entry in entries:
        if "/" not in entry.path:
            validate_identifier(entry.path, "receipt skill name")
            if not entry.is_directory:
                raise ValueError("receipt skill roots must be directories")
            roots.append(entry.path)
        else:
            parent = by_path.get(entry.path.rsplit("/", 1)[0])
            if parent is None or not parent.is_directory:
                raise ValueError("inventory must contain every exact parent directory")
    if not roots:
        raise ValueError("installed receipt must own at least one skill")
    for root in roots:
        skill_file = by_path.get(root + "/SKILL.md")
        if skill_file is None or skill_file.is_directory:
            raise ValueError("receipt skills must contain a SKILL.md file")


def read_receipt(data: bytes, bundle_id: str) -> Receipt:
    try:
        raw: object = json.loads(
            data.decode("utf-8"), object_pairs_hook=_unique_object, parse_constant=_reject_constant
        )
    except RecursionError as error:
        raise ValueError("receipt nesting exceeds the JSON parser limit") from error
    fields = _object(
        raw,
        {
            "schema_version",
            "bundle_id",
            "transaction_id",
            "status",
            "version",
            "content_digest",
            "inventory_digest",
            "entries",
        },
        set(),
    )
    if type(fields["schema_version"]) is not int or fields["schema_version"] != 1:
        raise ValueError("receipt schema_version must be the integer 1")
    identifier = _string(fields["bundle_id"], "receipt bundle id")
    validate_identifier(identifier, "receipt bundle id")
    if identifier != bundle_id:
        raise ValueError("receipt bundle id must match its filename")
    transaction_id = _hex(fields["transaction_id"], 32, "transaction id")
    records = fields["entries"]
    if not isinstance(records, list):
        raise ValueError("receipt entries must be a JSON array")
    if fields["status"] == "removed":
        if records or any(
            fields[key] is not None for key in ("version", "content_digest", "inventory_digest")
        ):
            raise ValueError("removed receipts must contain no version, digests or inventory")
        return Receipt(identifier, transaction_id, None, None, ())
    if fields["status"] != "installed":
        raise ValueError("receipt status must be installed or removed")
    version = _string(fields["version"], "receipt version")
    validate_version(version)
    content = _hex(fields["content_digest"], 64, "content digest")
    entries = tuple(
        sorted((_entry(record) for record in records), key=lambda e: e.path.encode("utf-8"))
    )
    _validate_tree(entries)
    expected = _hex(fields["inventory_digest"], 64, "inventory digest")
    if inventory_digest(entries) != expected:
        raise ValueError("receipt inventory digest disagrees with its entries")
    return Receipt(identifier, transaction_id, version, content, entries)


def receipt_bytes(receipt: Receipt) -> bytes:
    entries: list[dict[str, object]] = []
    for entry in receipt.entries:
        record: dict[str, object] = {
            "path": entry.path,
            "kind": "directory" if entry.is_directory else "file",
        }
        if not entry.is_directory:
            record.update(size=entry.size, sha256=entry.sha256, executable=entry.executable)
        entries.append(record)
    value = {
        "schema_version": 1,
        "bundle_id": receipt.bundle_id,
        "transaction_id": receipt.transaction_id,
        "status": "removed" if receipt.removed else "installed",
        "version": receipt.version,
        "content_digest": receipt.content_digest,
        "inventory_digest": None if receipt.removed else inventory_digest(receipt.entries),
        "entries": entries,
    }
    data = (
        json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n"
    ).encode("utf-8")
    read_receipt(data, receipt.bundle_id)
    return data


def bundle_receipt(bundle: Bundle, transaction_id: str) -> Receipt:
    receipt = Receipt(
        bundle.id, transaction_id, bundle.version, bundle.content_digest, inventory(bundle.entries)
    )
    read_receipt(receipt_bytes(receipt), bundle.id)
    return receipt


def reconcile_receipts(receipts: tuple[Receipt, ...]) -> dict[str, str]:
    owners: dict[str, str] = {}
    transactions: set[str] = set()
    bundles: set[str] = set()
    for receipt in receipts:
        if receipt.bundle_id in bundles or receipt.transaction_id in transactions:
            raise ValueError("receipts contain duplicate bundle or transaction identities")
        bundles.add(receipt.bundle_id)
        transactions.add(receipt.transaction_id)
        for name in receipt.skills:
            if name in owners:
                raise ValueError(f"receipts claim the same skill: {name}")
            owners[name] = receipt.bundle_id
    return owners
