import json
import os
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from state_helpers import canonical_json, journal_directory, put
from test_configuration_lifecycle import bundle, rendered

from flyrail import (
    DirectoryStore,
    ErrorCode,
    InstallationTarget,
    OperationStatus,
    ResourceAuthority,
    apply_preview,
    preview,
    remove,
    sync,
)
from flyrail import _lifecycle as lifecycle
from flyrail import _resource_io as io
from flyrail import _security as security
from flyrail._codec import encode
from flyrail._observation import ObservationFailure, Observer
from flyrail._resource_models import Receipt, ResourceRef, Revision
from flyrail._state import read_record, resource_key
from flyrail.authority import transaction_path


@pytest.mark.parametrize(
    "case",
    ["changed", "directory", "file", "open", "read", "collision", "spelling", "managed-file"],
)
def test_observation_refuses_inconsistent_filesystem_views(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, case: str
) -> None:
    path = tmp_path / "file"
    path.write_bytes(b"first")
    observer = Observer()
    with pytest.raises(ObservationFailure):
        if case == "changed":
            observer.metadata(path)
            path.write_bytes(b"different")
            observer.finish()
        elif case == "directory":
            observer.directory(path)
        elif case == "managed-file":
            observer.managed_directory(path)
        elif case == "file":
            observer.file(tmp_path)
        elif case in {"open", "read"}:
            original = os.fstat
            count = 0

            def fstat(fd: int) -> os.stat_result:
                nonlocal count
                count += 1
                if count == (1 if case == "open" else 2):
                    path.write_bytes(b"changed while observing")
                return original(fd)

            monkeypatch.setattr(os, "fstat", fstat)
            observer.file(path)
        else:

            def children(parent: Path) -> Iterator[Path]:
                return iter(
                    [parent / "FILE", parent / "file"] if case == "collision" else [parent / "FILE"]
                )

            monkeypatch.setattr(Path, "iterdir", children)
            if case == "collision":
                observer.directory(tmp_path)
            else:
                observer.managed_directory(path)
    assert path.read_bytes() == (
        b"changed while observing"
        if case in {"open", "read"}
        else b"different"
        if case == "changed"
        else b"first"
    )


@pytest.mark.parametrize("change", ["ancestor-mode", "ancestor-xattr", "added-ancestor"])
def test_immutable_preview_binds_full_ancestor_metadata(tmp_path: Path, change: str) -> None:
    parent = tmp_path / "project"
    parent.mkdir()
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))
    path = parent / "child" / "AGENTS.md" if change == "added-ancestor" else parent / "AGENTS.md"
    plan = preview(bundle(), rendered(path), target)
    if change == "ancestor-mode":
        if os.name == "nt":
            pytest.skip("POSIX ancestor mode evidence")
        parent.chmod(0o700)
    elif change == "added-ancestor":
        path.parent.mkdir()
    else:
        if os.name == "nt":
            pytest.skip("POSIX ancestor xattr evidence")
        import shutil
        import subprocess
        import sys

        if sys.platform == "darwin":
            executable = shutil.which("xattr")
            assert executable is not None
            subprocess.run(  # noqa: S603
                [executable, "-w", "flyrail.ancestor", "evidence", str(parent)], check=True
            )
        elif sys.platform == "linux":
            os.setxattr(parent, "user.flyrail", b"evidence")
        else:
            pytest.skip("POSIX ancestor xattr evidence")
    result = apply_preview(plan)
    assert result.status is OperationStatus.FAILED and result.error is not None
    assert result.error.code is ErrorCode.CONCURRENT_CHANGE
    assert not path.exists()


@pytest.mark.parametrize(
    "damage",
    [
        "resource",
        "previous",
        "receipt",
        "schema",
        "revision",
        "ancestors",
        "unknown-transaction-data",
        "different-receipt",
    ],
)
def test_corrupt_journal_retains_full_evidence_and_reports_incomplete(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, damage: str
) -> None:
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"foreign")
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))
    store = target.store

    def crash(store: Any, journal: Any) -> None:
        raise SystemExit("after preparation")

    with monkeypatch.context() as patch:
        patch.setattr(lifecycle, "publish", crash)
        with pytest.raises(SystemExit):
            sync(bundle(), rendered(path), target)
    destination = ResourceAuthority(path).destination
    journal = resource_key(destination, "journal")
    stored = store.read(journal)
    assert stored is not None
    state = journal_directory(store, destination)
    raw = json.loads(stored)
    fields = raw["fields"]
    if damage in {"resource", "previous", "receipt"}:
        fields[damage] = []
    elif damage == "schema":
        fields["schema_version"] = True
    elif damage == "revision":
        fields["backup"]["fields"]["nodes"] = []
    elif damage == "ancestors":
        fields["ancestors"] = [[]]
    elif damage == "unknown-transaction-data":
        (state / "unknown").write_bytes(b"retained unknown")
    else:
        header = read_record(store, resource_key(destination, "authority"), ResourceRef)
        assert header is not None
        put(store, resource_key(destination, "receipt"), encode(Receipt(header, "f" * 32)))
    put(store, journal, canonical_json(raw))
    result = remove("team", target)
    assert result.status is OperationStatus.INCOMPLETE
    if damage in {"unknown-transaction-data", "different-receipt"}:
        assert result.resources[0].recovery_paths == (state,)
    else:
        assert result.error is not None and result.error.code is ErrorCode.INVALID_STATE
        assert result.resources[0].recovery_paths == ()
    assert (state / "staging" / "resource").is_file()
    if damage == "unknown-transaction-data":
        assert (state / "unknown").read_bytes() == b"retained unknown"
    retained = store.read(journal)
    assert path.read_bytes() == b"foreign" and retained is not None
    assert json.loads(retained) == raw


def test_unsupported_staged_security_is_retained_without_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    destination = tmp_path / "AGENTS.md"
    destination.write_bytes(b"foreign")
    original = security.security

    def changed(path: Path, *, ancestor: bool = False) -> bytes:
        return (
            b"different"
            if path.name == "resource" and path.parent.name == "staging"
            else original(path, ancestor=ancestor)
        )

    monkeypatch.setattr(io, "security", changed)
    result = sync(
        bundle(), rendered(destination), InstallationTarget(DirectoryStore(tmp_path / "index"))
    )
    assert result.status is OperationStatus.FAILED and result.resources[0].recovery_paths == ()
    assert result.error is not None and result.error.code is ErrorCode.UNSUPPORTED
    assert destination.read_bytes() == b"foreign"


@pytest.mark.parametrize("location", ["index", "authority"])
def test_management_outputs_cannot_overlap_bundle_sources(tmp_path: Path, location: str) -> None:
    from skill_helpers import make_bundle

    destination = tmp_path / "AGENTS.md"
    source = (
        tmp_path / "source"
        if location == "index"
        else transaction_path(ResourceAuthority(destination).destination, "a" * 32)
    )
    snapshot = make_bundle(source)
    target = InstallationTarget(
        DirectoryStore(source / "index" if location == "index" else tmp_path / "index")
    )
    original = {
        path.relative_to(source): path.read_bytes() for path in source.rglob("*") if path.is_file()
    }
    with pytest.raises(ValueError, match="overlaps bundle source"):
        sync(snapshot, rendered(destination), target)
    assert {
        path.relative_to(source): path.read_bytes() for path in source.rglob("*") if path.is_file()
    } == original
    assert not destination.exists()


def test_staging_content_changed_before_journaling_is_not_published(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    destination = tmp_path / "AGENTS.md"
    destination.write_bytes(b"foreign")
    original = io.observe

    def observe(path: Path) -> Revision:
        if path.name == "resource" and path.parent.name == "staging" and path.exists():
            path.write_bytes(b"changed while staging")
        return original(path)

    monkeypatch.setattr(io, "observe", observe)
    result = sync(
        bundle(), rendered(destination), InstallationTarget(DirectoryStore(tmp_path / "index"))
    )
    assert result.status is OperationStatus.FAILED and result.error is not None
    assert result.error.code is ErrorCode.CONCURRENT_CHANGE
    assert destination.read_bytes() == b"foreign"
