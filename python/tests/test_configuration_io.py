import json
import os
import threading
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from test_configuration_lifecycle import bundle, rendered

from flyrail import (
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
from flyrail._filesystem import target_lock
from flyrail._observation import ObservationFailure, Observer
from flyrail._resource_models import Ancestor, Receipt, ResourceRef, Revision


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


def test_lock_timeout_and_later_retry_keep_one_permanent_lock_inode(tmp_path: Path) -> None:
    state = tmp_path / "index"
    with target_lock(state, 0):
        inode = (state / "lock").stat().st_ino
        with pytest.raises(ObservationFailure) as caught, target_lock(state, 0.025):
            pytest.fail("contending lock entered")
        assert caught.value.error.code is ErrorCode.BUSY
    entered = threading.Event()
    release = threading.Event()

    def worker() -> None:
        with target_lock(state, 1):
            entered.set()
            release.wait(2)

    thread = threading.Thread(target=worker)
    thread.start()
    assert entered.wait(2)
    release.set()
    with target_lock(state, 1):
        assert (state / "lock").stat().st_ino == inode
    thread.join(2)
    assert not thread.is_alive()


@pytest.mark.parametrize("seam", ["existing-next", "payload", "ancestor"])
def test_metadata_atomic_write_preserves_original_on_stale_preparation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, seam: str
) -> None:
    path = tmp_path / "metadata.json"
    path.write_bytes(encode(Revision()))
    prior = path.read_bytes()
    if seam == "existing-next":
        path.with_name(path.name + ".next").write_bytes(b"retained")
    original_observe = io.observe
    original_ancestors = io.ancestors
    if seam == "payload":

        def changed(selected: Path) -> Revision:
            if selected.name.endswith(".next"):
                selected.write_bytes(b"intervening metadata")
            return original_observe(selected)

        monkeypatch.setattr(io, "observe", changed)
    if seam == "ancestor":
        calls = 0

        def parents(selected: Path) -> tuple[Ancestor, ...]:
            nonlocal calls
            calls += 1
            values = original_ancestors(selected)
            return values[:-1] if calls >= 4 else values

        monkeypatch.setattr(io, "ancestors", parents)
    with pytest.raises(ObservationFailure):
        io.atomic(path, Revision())
    assert path.read_bytes() == prior
    assert path.with_name(path.name + ".next").is_file()


@pytest.mark.parametrize("change", ["ancestor-mode", "ancestor-xattr", "added-ancestor"])
def test_immutable_preview_binds_full_ancestor_metadata(tmp_path: Path, change: str) -> None:
    parent = tmp_path / "project"
    parent.mkdir()
    target = InstallationTarget(tmp_path / "index")
    path = parent / "child" / "AGENTS.md" if change == "added-ancestor" else parent / "AGENTS.md"
    plan = preview(bundle(), rendered(path), target)
    if change == "ancestor-mode":
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
        "unknown-state",
        "different-receipt",
    ],
)
def test_corrupt_journal_retains_full_evidence_and_reports_incomplete(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, damage: str
) -> None:
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"foreign")
    target = InstallationTarget(tmp_path / "index")

    def crash(journal: Any) -> None:
        raise SystemExit("after preparation")

    with monkeypatch.context() as patch:
        patch.setattr(lifecycle, "publish", crash)
        with pytest.raises(SystemExit):
            sync(bundle(), rendered(path), target)
    state = ResourceAuthority(path).state_root
    journal = state / "transaction.json"
    raw = json.loads(journal.read_bytes())
    fields = raw["fields"]
    if damage in {"resource", "previous", "receipt"}:
        fields[damage] = []
    elif damage == "schema":
        fields["schema_version"] = True
    elif damage == "revision":
        fields["backup"]["fields"]["nodes"] = []
    elif damage == "ancestors":
        fields["ancestors"] = [[]]
    elif damage == "unknown-state":
        (state / "unknown").write_bytes(b"retained unknown")
    else:
        header = io.read(state / "authority.json", ResourceRef)
        assert header is not None
        (state / "receipt.json").write_bytes(encode(Receipt(header, "f" * 32)))
    journal.write_text(json.dumps(raw))
    result = remove("team", target)
    if damage == "unknown-state":
        assert result.status is OperationStatus.FAILED
    else:
        assert result.status is OperationStatus.INCOMPLETE and result.resources[0].recovery_paths
    assert path.read_bytes() == b"foreign" and json.loads(journal.read_bytes()) == raw


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
    result = sync(bundle(), rendered(destination), InstallationTarget(tmp_path / "index"))
    assert result.status is OperationStatus.INCOMPLETE and result.resources[0].recovery_paths
    assert destination.read_bytes() == b"foreign"


@pytest.mark.parametrize("location", ["index", "authority"])
def test_management_outputs_cannot_overlap_bundle_sources(tmp_path: Path, location: str) -> None:
    from skill_helpers import make_bundle

    destination = tmp_path / "AGENTS.md"
    source = (
        tmp_path / "source" if location == "index" else ResourceAuthority(destination).state_root
    )
    snapshot = make_bundle(source)
    target = InstallationTarget(source / "index" if location == "index" else tmp_path / "index")
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
    result = sync(bundle(), rendered(destination), InstallationTarget(tmp_path / "index"))
    assert result.status is OperationStatus.INCOMPLETE and result.error is not None
    assert result.error.code is ErrorCode.CONCURRENT_CHANGE
    assert destination.read_bytes() == b"foreign"
