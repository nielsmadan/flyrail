import json
from pathlib import Path

import pytest
from conformance import FIXTURES
from test_inspection import make_bundle

from flyrail._intent import Change, Intent, intent_bytes, read_intent
from flyrail._inventory import InventoryEntry
from flyrail._receipts import Receipt, bundle_receipt


def test_intent_roundtrip_and_observed_incomplete_owned_tree(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    previous = bundle_receipt(bundle, "1" * 32)
    removed = Receipt("team", "2" * 32, None, None, ())
    intent = Intent(previous, removed, (Change("review", (InventoryEntry("review"),), ()),))
    assert read_intent(intent_bytes(intent)) == intent
    assert read_intent(
        intent_bytes(Intent(previous, removed, (Change("review", (), ()),)))
    ).new.removed


@pytest.mark.parametrize(
    "damage",
    [
        "schema",
        "nested",
        "identity",
        "changes-type",
        "name",
        "inventory-type",
        "duplicate",
        "escape",
        "parent",
        "duplicate-change",
        "unowned",
        "new",
        "outside",
        "omitted",
    ],
)
def test_invalid_intent_cannot_authorize_recovery(tmp_path: Path, damage: str) -> None:
    bundle = make_bundle(tmp_path / "source")
    old = bundle_receipt(bundle, "1" * 32)
    removed = Receipt("team", "2" * 32, None, None, ())
    raw = json.loads(intent_bytes(Intent(old, removed, (Change("review", old.entries, ()),))))
    if damage == "schema":
        raw["schema_version"] = True
    elif damage == "nested":
        with pytest.raises(ValueError):
            read_intent(b"[" * 2000 + b"0" + b"]" * 2000)
        return
    elif damage == "identity":
        raw["new"]["transaction_id"] = old.transaction_id
    elif damage == "changes-type":
        raw["changes"] = None
    elif damage == "name":
        raw["changes"][0]["name"] = "../outside"
    elif damage == "inventory-type":
        raw["changes"][0]["old"] = None
    elif damage == "duplicate":
        raw["changes"][0]["old"].append(raw["changes"][0]["old"][0])
    elif damage == "escape":
        raw["changes"][0]["old"][0]["path"] = "other"
    elif damage == "parent":
        raw["changes"][0]["old"].pop(0)
    elif damage == "duplicate-change":
        raw["changes"].append(raw["changes"][0])
    elif damage == "unowned":
        raw["previous"] = None
    elif damage == "new":
        raw["changes"][0]["new"] = raw["changes"][0]["old"]
    elif damage == "outside":
        raw["changes"].append({"name": "outside", "old": [], "new": []})
    else:
        raw["changes"] = []
    with pytest.raises(ValueError):
        read_intent(json.dumps(raw).encode())


def test_language_neutral_intents() -> None:
    fixtures = FIXTURES / "transactions.json"
    for case in json.loads(fixtures.read_bytes()):
        data = json.dumps(case["intent"]).encode()
        parsed = read_intent(data)
        assert json.loads(intent_bytes(parsed)) == case["intent"]
        assert parsed.transaction_id == case["intent"]["new"]["transaction_id"]
