import copy
import hashlib
import json
import re
from typing import cast

import pytest
from conformance import FIXTURES

from flyrail._inventory import InventoryEntry, inventory_digest
from flyrail._receipts import Receipt, read_receipt, receipt_bytes, reconcile_receipts

FIXTURE = cast(
    dict[str, object],
    json.loads((FIXTURES / "receipts.json").read_bytes()),
)


def record() -> dict[str, object]:
    return copy.deepcopy(cast(dict[str, object], FIXTURE["receipt"]))


def decode(value: dict[str, object]) -> Receipt:
    return read_receipt(json.dumps(value).encode(), "team")


def test_language_neutral_receipt_and_independent_inventory_framing() -> None:
    expected = record()
    receipt = decode(expected)

    assert receipt.bundle_id == "team"
    assert receipt.transaction_id == "0123456789abcdef0123456789abcdef"
    assert receipt.version == "autumn"
    assert receipt.content_digest == expected["content_digest"]
    assert receipt.skills == ("review",)
    assert receipt.removed is False
    assert receipt.entries == (
        InventoryEntry("review"),
        InventoryEntry(
            "review/SKILL.md",
            6,
            "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03",
            False,
        ),
        InventoryEntry("review/empty"),
    )
    framing = bytes.fromhex(cast(str, FIXTURE["inventory_framing_hex"]))
    assert hashlib.sha256(framing).hexdigest() == expected["inventory_digest"]
    assert inventory_digest(receipt.entries) == expected["inventory_digest"]
    assert json.loads(receipt_bytes(receipt)) == expected
    assert read_receipt(receipt_bytes(receipt), "team") == receipt
    unordered = record()
    unordered["entries"] = list(reversed(cast(list[object], unordered["entries"])))
    assert decode(unordered) == receipt


@pytest.mark.parametrize(
    ("field", "invalid"),
    [
        ("schema_version", True),
        ("schema_version", 1.0),
        ("schema_version", 2),
        ("bundle_id", "other"),
        ("bundle_id", "TEAM"),
        ("bundle_id", 1),
        ("transaction_id", "a" * 31),
        ("transaction_id", "A" * 32),
        ("transaction_id", None),
        ("status", "future"),
        ("status", False),
        ("version", " "),
        ("version", None),
        ("content_digest", "bad"),
        ("content_digest", "A" * 64),
        ("content_digest", 5),
        ("inventory_digest", "0" * 64),
        ("inventory_digest", None),
        ("entries", None),
        ("entries", {}),
        ("entries", []),
    ],
)
def test_receipt_fields_are_strict(field: str, invalid: object) -> None:
    value = record()
    value[field] = invalid
    with pytest.raises(ValueError):
        decode(value)


@pytest.mark.parametrize(
    "kind", ["unknown", "missing", "duplicate", "bom", "invalid-utf8", "constant", "not-object"]
)
def test_receipt_json_is_strict(kind: str) -> None:
    value = record()
    if kind == "unknown":
        value["unknown"] = 1
    if kind == "missing":
        del value["version"]
    data = json.dumps(value).encode()
    if kind == "duplicate":
        data = data.replace(b'"schema_version": 1', b'"schema_version": 1, "schema_version": 1')
    elif kind == "bom":
        data = b"\xef\xbb\xbf" + data
    elif kind == "invalid-utf8":
        data = b"\xff"
    elif kind == "constant":
        data = data.replace(b'"schema_version": 1', b'"schema_version": NaN')
    elif kind == "not-object":
        data = b"[]"
    with pytest.raises(ValueError):
        read_receipt(data, "team")


@pytest.mark.parametrize(
    "change",
    [
        "directory-data",
        "unknown-kind",
        "missing-file-field",
        "bool-size",
        "negative-size",
        "oversize",
        "float-size",
        "string-executable",
        "bad-hash",
        "traversal",
        "unknown-entry-field",
    ],
)
def test_receipt_inventory_entry_fields_are_strict(change: str) -> None:
    value = record()
    entries = cast(list[dict[str, object]], value["entries"])
    file = entries[1]
    if change == "directory-data":
        entries[0]["size"] = 0
    elif change == "unknown-kind":
        file["kind"] = "link"
    elif change == "missing-file-field":
        del file["size"]
    elif change.endswith("size"):
        file["size"] = {
            "bool-size": True,
            "negative-size": -1,
            "oversize": 2**64,
            "float-size": 6.0,
        }[change]
    elif change == "string-executable":
        file["executable"] = "false"
    elif change == "bad-hash":
        file["sha256"] = "wrong"
    elif change == "traversal":
        file["path"] = "review/../SKILL.md"
    else:
        file["unknown"] = True
    with pytest.raises(ValueError):
        decode(value)


@pytest.mark.parametrize(
    ("change", "error"),
    [
        ("duplicate", "inventory contains duplicate or portable-colliding paths"),
        ("case-collision", "inventory contains duplicate or portable-colliding paths"),
        ("missing-parent", "inventory must contain every exact parent directory"),
        ("file-parent", "inventory must contain every exact parent directory"),
        ("root-file", "receipt skill roots must be directories"),
        (
            "invalid-root",
            "receipt skill name must be 1-64 lowercase ASCII letters/digits with internal hyphens",
        ),
        ("no-skill-file", "receipt skills must contain a SKILL.md file"),
        ("directory-skill-file", "receipt skills must contain a SKILL.md file"),
        ("no-root", "installed receipt must own at least one skill"),
    ],
)
def test_receipt_inventory_requires_complete_portable_skill_trees(change: str, error: str) -> None:
    value = record()
    entries = cast(list[dict[str, object]], value["entries"])
    file = entries[1]
    if change == "duplicate":
        entries.append(copy.deepcopy(file))
    elif change == "case-collision":
        entries.append({"path": "review/EMPTY", "kind": "directory"})
    elif change == "missing-parent":
        entries.append({"path": "review/missing/child", "kind": "directory"})
    elif change == "file-parent":
        entries.append({"path": "review/SKILL.md/child", "kind": "directory"})
    elif change == "root-file":
        entries.append({**file, "path": "extra"})
    elif change == "invalid-root":
        for entry in entries:
            entry["path"] = cast(str, entry["path"]).replace("review", "Invalid", 1)
    elif change == "no-skill-file":
        file["path"] = "review/readme"
    elif change == "directory-skill-file":
        entries[1] = {"path": "review/SKILL.md", "kind": "directory"}
    else:
        entries.clear()
    value["inventory_digest"] = inventory_digest(
        tuple(
            InventoryEntry(
                cast(str, entry["path"]),
                cast(int | None, entry.get("size")),
                cast(str | None, entry.get("sha256")),
                cast(bool, entry.get("executable", False)),
            )
            for entry in entries
        )
    )
    with pytest.raises(ValueError, match="^" + re.escape(error) + "$"):
        decode(value)


@pytest.mark.parametrize("version", ["秋", "  cafe\u0301 \U0001f680  "])
def test_receipt_versions_round_trip_unicode_verbatim(version: str) -> None:
    value = record()
    value["version"] = version
    receipt = decode(value)

    assert receipt.version == version
    assert read_receipt(receipt_bytes(receipt), "team").version == version


@pytest.mark.parametrize("version", ["\ud800", "release\udfff"])
def test_receipts_reject_non_scalar_versions(version: str) -> None:
    value = record()
    value["version"] = version
    with pytest.raises(ValueError, match="version must contain only Unicode scalar values"):
        decode(value)


def test_removed_receipt_is_transaction_tagged_and_owns_nothing() -> None:
    value = record()
    value.update(
        status="removed", version=None, content_digest=None, inventory_digest=None, entries=[]
    )
    receipt = decode(value)

    assert receipt.removed is True
    assert receipt.skills == ()
    assert reconcile_receipts((receipt,)) == {}
    assert json.loads(receipt_bytes(receipt)) == value
    for field, invalid in (
        ("version", "1"),
        ("entries", [{"path": "review", "kind": "directory"}]),
        ("content_digest", "a" * 64),
        ("inventory_digest", "a" * 64),
    ):
        broken = {**value, field: invalid}
        with pytest.raises(ValueError, match="removed receipts"):
            decode(broken)


def test_reconciliation_checks_ownership_and_transaction_identity() -> None:
    first = decode(record())
    value = record()
    value.update(bundle_id="other", transaction_id="a" * 32)
    same_owner = read_receipt(json.dumps(value).encode(), "other")

    assert reconcile_receipts((first,)) == {"review": "team"}
    with pytest.raises(ValueError, match="same skill"):
        reconcile_receipts((first, same_owner))
    with pytest.raises(ValueError, match="duplicate bundle"):
        reconcile_receipts((first, first))
    tombstone = Receipt("other", first.transaction_id, None, None, ())
    with pytest.raises(ValueError, match="transaction identities"):
        reconcile_receipts((first, tombstone))
