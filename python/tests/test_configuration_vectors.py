import json
import os
from dataclasses import replace
from pathlib import Path
from typing import Any

import pytest
from conformance import FIXTURES
from state_helpers import put_index

from flyrail import (
    Claim,
    DirectoryStore,
    InstallationTarget,
    OperationStatus,
    ResourceRef,
    remove,
)
from flyrail._codec import decode, encode, record
from flyrail._observation import ObservationFailure
from flyrail._resource_io import observe
from flyrail._resource_models import Generation, Index, Preparation, Receipt, Revision
from flyrail._resource_transaction import Journal, recover, transaction_ancestors
from flyrail._state import (
    destination_key,
    index_key,
    read_record,
    resource_key,
    store_lock,
    write_record,
)
from flyrail.authority import transaction_path

VECTORS = json.loads((FIXTURES / "configurations.json").read_text(encoding="utf-8"))


def relocated(raw: object, root: Path) -> object:
    if isinstance(raw, list):
        return [relocated(item, root) for item in raw]
    if not isinstance(raw, dict):
        return raw
    if raw.keys() == {"path"}:
        return {"path": (root / raw["path"].rsplit("/", 1)[1]).as_posix()}
    result: dict[str, Any] = {key: relocated(value, root) for key, value in raw.items()}
    if result.get("type") == "Receipt":
        fields = result["fields"]
        ref = decode(json.dumps(fields["resource"]).encode())
        assert isinstance(ref, ResourceRef)
        for item in fields["claims"]:
            claim = decode(json.dumps(item["fields"]["claim"]).encode())
            assert isinstance(claim, Claim)
            item["fields"]["claim_id"] = claim.identity(ref.authority)
    return result


@pytest.mark.parametrize("case", VECTORS["receipts"], ids=lambda case: case["name"])
def test_neutral_receipt_vectors_drive_production_source_free_removal(
    tmp_path: Path, case: dict[str, Any]
) -> None:
    for name, outcome in case["removal"].items():
        root = tmp_path / name
        root.mkdir()
        value = decode(json.dumps(relocated(case["record"], root)).encode())
        assert isinstance(value, Receipt)
        path = value.resource.destination
        data = bytes.fromhex(
            case["document_hex"] if name == "unchanged" else outcome["document_hex"]
        )
        path.write_bytes(data)
        path.chmod(0o600)
        target = InstallationTarget(DirectoryStore(root / "index"))
        owner = value.claims[0].bundle_id
        generation = Generation(
            "1", "a" * 64, "b" * 64, ((value.resource, (value.claims[0].claim_id,)),)
        )
        key = index_key(owner, target.context_digest)
        store = target.store
        with (
            store_lock(store, key, 0),
            store_lock(store, "resource/" + destination_key(path), 0),
        ):
            write_record(store, resource_key(path, "authority"), value.resource, expected=None)
            write_record(store, resource_key(path, "receipt"), value, expected=None)
            put_index(target, owner, Index(owner, target.context_digest, generation))
        assert record(decode(encode(value))) == record(value)
        result = remove(owner, target)
        assert str(result.status) == outcome["status"]
        if result.status is OperationStatus.APPLIED:
            expected = outcome["document_hex"]
            assert (path.read_bytes() if path.exists() else None) == (
                bytes.fromhex(expected) if expected is not None else None
            )
        else:
            assert path.read_bytes() == data
            assert read_record(store, resource_key(path, "receipt"), Receipt) == value


@pytest.mark.parametrize("case", VECTORS["preparations"], ids=lambda case: case["name"])
def test_neutral_preparation_vectors_round_trip(tmp_path: Path, case: dict[str, Any]) -> None:
    raw = relocated(case["record"], tmp_path)
    value = decode(json.dumps(raw).encode())
    assert isinstance(value, Preparation)
    assert json.loads(encode(value)) == raw
    assert value.directory == transaction_path(value.resource.destination, value.transaction_id)


@pytest.mark.parametrize("case", VECTORS["transactions"], ids=lambda case: case["name"])
def test_neutral_transaction_vectors_drive_production_recovery(
    tmp_path: Path, case: dict[str, Any]
) -> None:
    raw = next(item["record"] for item in VECTORS["journals"] if item["name"] == case["journal"])
    journal = decode(json.dumps(relocated(raw, tmp_path)).encode())
    assert isinstance(journal, Journal)
    ref = journal.resource
    sources = {}
    revisions = {}
    for revision in (journal.before, journal.staged):
        token = revision.nodes[0].inode
        if token not in sources:
            path = tmp_path / f"origin-{token}"
            path.write_bytes(revision.data or b"")
            path.chmod(revision.nodes[0].mode)
            sources[token] = path
            revisions[token] = observe(path)
    journal = replace(
        journal,
        before=revisions[journal.before.nodes[0].inode],
        staged=revisions[journal.staged.nodes[0].inode],
        published=revisions[journal.published.nodes[0].inode],
        backup=revisions[journal.backup.nodes[0].inode],
    )
    store = DirectoryStore(tmp_path / "state")
    directory = transaction_path(ref.destination, journal.receipt.transaction_id)
    with store_lock(store, "resource/" + destination_key(ref.destination), 0):
        write_record(store, resource_key(ref.destination, "authority"), ref, expected=None)
        write_record(
            store,
            resource_key(ref.destination, "preparation"),
            Preparation(ref, journal.receipt.transaction_id, directory),
            expected=None,
        )
        directory.mkdir(mode=0o700)
        for folder in ["staging", "backup"]:
            (directory / folder).mkdir(mode=0o700)
        for key, path in [
            ("resource", ref.destination),
            ("staged", directory / "staging" / "resource"),
            ("backup", directory / "backup" / "resource"),
        ]:
            raw_revision = case["observed"][key]
            if raw_revision is None:
                continue
            observed = decode(json.dumps(raw_revision).encode())
            assert isinstance(observed, Revision)
            token = observed.nodes[0].inode
            source = sources.get(token)
            if source is None:
                source = tmp_path / f"origin-{token}"
                source.write_bytes(observed.data or b"")
            source.write_bytes(observed.data or b"")
            source.chmod(observed.nodes[0].mode)
            source.rename(path)
        journal = replace(journal, ancestors=transaction_ancestors(ref.destination, directory))
        committed = case["observed"]["receipt"] == "new"
        write_record(
            store,
            resource_key(ref.destination, "receipt"),
            journal.receipt if committed else journal.previous,
            expected=None,
        )
        os.utime(directory)
        write_record(store, resource_key(ref.destination, "journal"), journal, expected=None)
        if case["expected"]["status"] == "recovered":
            recover(store, ref)
            recover(store, ref)
            expected = decode(json.dumps(case["expected"]["resource"]).encode())
            assert isinstance(expected, Revision)
            assert ref.destination.read_bytes() == expected.data
        else:
            with pytest.raises((ObservationFailure, OSError)):
                recover(store, ref)
        retained = store.read(resource_key(ref.destination, "journal")) is not None
        assert retained is case["expected"]["retain_journal"]
        prepared = store.read(resource_key(ref.destination, "preparation")) is not None
        assert prepared is case["expected"]["retain_journal"]
        assert directory.exists() is case["expected"]["retain_journal"]
        assert (directory / "backup" / "resource").exists() is case["expected"]["retain_backup"]
        value = read_record(store, resource_key(ref.destination, "receipt"), Receipt)
        assert value == (
            journal.receipt if case["expected"]["receipt"] == "new" else journal.previous
        )
        if os.name == "nt" and ref.destination.exists():
            ref.destination.chmod(0o600)
