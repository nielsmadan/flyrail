import json
from dataclasses import dataclass

from flyrail._inventory import InventoryEntry
from flyrail._manifest import _object, _reject_constant, _string, _unique_object
from flyrail._receipts import Receipt, _entry, read_receipt, receipt_bytes
from flyrail._validation import portable_path_key, validate_identifier


@dataclass(frozen=True, slots=True)
class Change:
    name: str
    old: tuple[InventoryEntry, ...]
    new: tuple[InventoryEntry, ...]


@dataclass(frozen=True, slots=True)
class Intent:
    previous: Receipt | None
    new: Receipt
    changes: tuple[Change, ...]

    @property
    def transaction_id(self) -> str:
        return self.new.transaction_id


def select(entries: tuple[InventoryEntry, ...], name: str) -> tuple[InventoryEntry, ...]:
    return tuple(entry for entry in entries if entry.path.split("/", 1)[0] == name)


def _record(entry: InventoryEntry) -> dict[str, object]:
    value: dict[str, object] = {
        "path": entry.path,
        "kind": "directory" if entry.is_directory else "file",
    }
    if not entry.is_directory:
        value.update(size=entry.size, sha256=entry.sha256, executable=entry.executable)
    return value


def intent_bytes(intent: Intent) -> bytes:
    value = {
        "schema_version": 1,
        "previous": None if intent.previous is None else json.loads(receipt_bytes(intent.previous)),
        "new": json.loads(receipt_bytes(intent.new)),
        "changes": [
            {
                "name": item.name,
                "old": [_record(e) for e in item.old],
                "new": [_record(e) for e in item.new],
            }
            for item in intent.changes
        ],
    }
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def _tree(value: object, name: str) -> tuple[InventoryEntry, ...]:
    if not isinstance(value, list):
        raise ValueError("transaction inventories must be arrays")
    entries = tuple(sorted((_entry(item) for item in value), key=lambda e: e.path.encode()))
    paths = {entry.path: entry for entry in entries}
    if len({portable_path_key(e.path) for e in entries}) != len(entries):
        raise ValueError("transaction inventory paths collide")
    for entry in entries:
        if entry.path.split("/", 1)[0] != name:
            raise ValueError("transaction inventory escapes its skill")
        if "/" in entry.path:
            parent = paths.get(entry.path.rsplit("/", 1)[0])
            if parent is None or not parent.is_directory:
                raise ValueError("transaction inventory lacks its parent directory")
    return entries


def read_intent(data: bytes) -> Intent:
    try:
        raw: object = json.loads(
            data.decode("utf-8"), object_pairs_hook=_unique_object, parse_constant=_reject_constant
        )
    except RecursionError as error:
        raise ValueError("transaction nesting exceeds the JSON parser limit") from error
    fields = _object(raw, {"schema_version", "previous", "new", "changes"}, set())
    if type(fields["schema_version"]) is not int or fields["schema_version"] != 1:
        raise ValueError("transaction schema_version must be the integer 1")
    new_fields = _object(
        fields["new"],
        {"bundle_id"},
        {
            "schema_version",
            "transaction_id",
            "status",
            "version",
            "content_digest",
            "inventory_digest",
            "entries",
        },
    )
    identifier = _string(new_fields["bundle_id"], "transaction bundle id")
    validate_identifier(identifier, "transaction bundle id")
    new = read_receipt(json.dumps(fields["new"]).encode(), identifier)
    previous = (
        None
        if fields["previous"] is None
        else read_receipt(json.dumps(fields["previous"]).encode(), identifier)
    )
    if previous is not None and previous.transaction_id == new.transaction_id:
        raise ValueError("transaction must have a fresh identity")
    records = fields["changes"]
    if not isinstance(records, list):
        raise ValueError("transaction changes must be an array")
    changes: list[Change] = []
    seen: set[str] = set()
    old_entries = () if previous is None else previous.entries
    for record in records:
        item = _object(record, {"name", "old", "new"}, set())
        name = _string(item["name"], "transaction skill name")
        validate_identifier(name, "transaction skill name")
        old, desired = _tree(item["old"], name), _tree(item["new"], name)
        if name in seen:
            raise ValueError("transaction changes must be unique")
        seen.add(name)
        if old and not select(old_entries, name):
            raise ValueError("transaction cannot replace unowned content")
        if desired != select(new.entries, name):
            raise ValueError("transaction new inventory disagrees with receipt")
        changes.append(Change(name, old, desired))
    names = set(new.skills) | (set() if previous is None else set(previous.skills))
    if not seen <= names:
        raise ValueError("transaction operates outside receipt ownership")
    for name in names - seen:
        if select(old_entries, name) != select(new.entries, name):
            raise ValueError("transaction omits a required change")
    return Intent(previous, new, tuple(changes))
