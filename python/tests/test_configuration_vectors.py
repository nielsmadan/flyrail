import json
import os
from dataclasses import replace
from pathlib import Path
from typing import Any

import pytest
from conformance import FIXTURES

from flyrail import (
    Claim,
    InstallationTarget,
    OperationStatus,
    ResourceAuthority,
    ResourceRef,
    remove,
)
from flyrail._codec import decode, encode, record
from flyrail._filesystem import target_lock
from flyrail._observation import ObservationFailure
from flyrail._resource_io import atomic, observe
from flyrail._resource_models import Generation, Index, Receipt, Revision
from flyrail._resource_transaction import Journal, recover, transaction_ancestors

VECTORS = json.loads((FIXTURES / "configurations.json").read_text())


def relocated(raw: object, root: Path) -> object:
    if isinstance(raw, list):
        return [relocated(item, root) for item in raw]
    if not isinstance(raw, dict):
        return raw
    if raw.keys() == {"path"}:
        original = raw["path"]
        name = original.rsplit("/", 1)[1]
        if name.startswith(".flyrail-"):
            return {
                "path": str(
                    ResourceAuthority(
                        root / ("config.json" if "config.json" in str(raw) else "AGENTS.md")
                    ).state_root
                )
            }
        return {"path": str(root / name)}
    result: dict[str, Any] = {key: relocated(value, root) for key, value in raw.items()}
    if result.get("type") == "ResourceRef":
        fields = result["fields"]
        path = Path(fields["destination"]["path"])
        fields["state_root"] = {"path": str(ResourceAuthority(path).state_root)}
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
        target = InstallationTarget(root / "index")
        owner = value.claims[0].bundle_id
        generation = Generation(
            "1", "a" * 64, "b" * 64, ((value.resource, (value.claims[0].claim_id,)),)
        )
        with target_lock(value.resource.state_root, 0), target_lock(target.index_root, 0):
            atomic(value.resource.state_root / "authority.json", value.resource)
            atomic(value.resource.state_root / "receipt.json", value)
            atomic(
                target.index_root / "index.json", Index(owner, target.context_digest, generation)
            )
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
            retained = decode((value.resource.state_root / "receipt.json").read_bytes())
            assert retained == value


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
    with target_lock(ref.state_root, 0):
        atomic(ref.state_root / "authority.json", ref)
        for folder in ["staging", "backup"]:
            (ref.state_root / folder).mkdir(mode=0o700)
        for key, path in [
            ("resource", ref.destination),
            ("staged", ref.state_root / "staging" / "resource"),
            ("backup", ref.state_root / "backup" / "resource"),
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
        journal = replace(journal, ancestors=transaction_ancestors(ref))
        committed = case["observed"]["receipt"] == "new"
        atomic(ref.state_root / "receipt.json", journal.receipt if committed else journal.previous)
        atomic(ref.state_root / "transaction.json", journal)
        if case["expected"]["status"] == "recovered":
            recover(ref)
            recover(ref)
            expected = decode(json.dumps(case["expected"]["resource"]).encode())
            assert isinstance(expected, Revision)
            assert ref.destination.read_bytes() == expected.data
        else:
            with pytest.raises((ObservationFailure, OSError)):
                recover(ref)
        assert (ref.state_root / "transaction.json").exists() is case["expected"]["retain_journal"]
        assert (ref.state_root / "backup" / "resource").exists() is case["expected"][
            "retain_backup"
        ]
        value = decode((ref.state_root / "receipt.json").read_bytes())
        assert value == (
            journal.receipt if case["expected"]["receipt"] == "new" else journal.previous
        )
        if os.name == "nt" and ref.destination.exists():
            ref.destination.chmod(0o600)
