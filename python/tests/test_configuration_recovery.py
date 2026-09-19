import errno
import multiprocessing as mp
import os
from multiprocessing.connection import Connection
from pathlib import Path

import pytest
from skill_helpers import snapshot
from test_configuration_lifecycle import bundle, receipt, rendered

from flyrail import (
    Acquisition,
    Dependency,
    ErrorCode,
    Family,
    FileContent,
    InstallationTarget,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    ResourceAuthority,
    SectionContent,
    inspect_installation,
    recover_installation,
    remove,
    sync,
)
from flyrail import _lifecycle as lifecycle
from flyrail import _resource_transaction as tx
from flyrail._resource_io import move as native_move
from flyrail._resource_io import observe
from flyrail._resource_models import Revision
from flyrail._security import replace_file as native_replace_file
from flyrail.configuration import ResourcePlan

pytestmark = pytest.mark.integration


def live_writer(root: str, channel: Connection) -> None:
    original = tx.prepare

    def prepare(plan: ResourcePlan) -> tx.Journal:
        journal = original(plan)
        channel.send("journal-ready")
        if not channel.poll(20):
            raise RuntimeError("writer barrier timed out")
        channel.recv()
        return journal

    pytest.MonkeyPatch().setattr(lifecycle, "prepare", prepare)
    try:
        result = sync(
            bundle("2"),
            rendered(Path(root) / "AGENTS.md", "second"),
            InstallationTarget(Path(root) / "index"),
        )
        channel.send(str(result.status))
    finally:
        channel.close()


def test_live_public_writer_blocks_other_index_until_source_free_recovery(tmp_path: Path) -> None:
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"# foreign\n")
    target = InstallationTarget(tmp_path / "index")
    other_target = InstallationTarget(tmp_path / "other-index")
    other = RenderedBundle(
        [
            RenderedArtifact(
                "other", Family.INSTRUCTIONS, path, SectionContent("other", "other text")
            )
        ]
    )
    assert sync(bundle(), rendered(path), target).status is OperationStatus.APPLIED
    assert sync(bundle(identifier="other"), other, other_target).status is OperationStatus.APPLIED
    installed = path.read_bytes()
    authority = ResourceAuthority(path)
    ctx = mp.get_context("spawn")
    parent, child = ctx.Pipe()
    process = ctx.Process(target=live_writer, args=(str(tmp_path), child))
    process.start()
    child.close()
    try:
        assert parent.poll(15) and parent.recv() == "journal-ready"
        assert process.is_alive()
        journal = authority.state_root / "transaction.json"
        assert journal.is_file()
        before = snapshot(tmp_path)
        competing = remove("other", other_target, lock_timeout=0)
        assert competing.status is OperationStatus.FAILED
        assert competing.error is not None and competing.error.code is ErrorCode.BUSY
        assert process.is_alive()
        assert snapshot(tmp_path) == before
        assert path.read_bytes() == installed
        assert inspect_installation("team", target).resources[0].recovery_paths

        process.terminate()
        process.join(10)
        assert process.exitcode is not None and process.exitcode != 0
        recovered = recover_installation("team", target)
        assert recovered.status is OperationStatus.APPLIED and recovered.error is None
        assert path.read_bytes() == installed
        assert recovered.observation.resources[0].recovery_paths == ()
        assert remove("team", target).status is OperationStatus.APPLIED
        assert remove("other", other_target).status is OperationStatus.APPLIED
        assert path.read_bytes() == b"# foreign\n"
    finally:
        if process.is_alive():
            process.kill()
            process.join(10)
        parent.close()


def crash_worker(root: str, boundary: str) -> None:
    path = Path(root) / "AGENTS.md"
    target = InstallationTarget(Path(root) / "index")
    original_prepare = tx.prepare
    original_move = native_move
    original_publish_receipt = tx.publish_receipt

    def prepare(plan: ResourcePlan) -> tx.Journal:
        journal = original_prepare(plan)
        if boundary == "prepared":
            os._exit(77)
        return journal

    def move(source: Path, destination: Path, revision: Revision) -> None:
        original_move(source, destination, revision)
        if boundary == "backup" and destination.parent.name == "backup":
            os._exit(77)

    def publish_receipt(journal: tx.Journal) -> None:
        if boundary == "published":
            os._exit(77)
        original_publish_receipt(journal)
        if boundary == "committed":
            os._exit(77)

    pytest.MonkeyPatch().setattr(lifecycle, "prepare", prepare)
    pytest.MonkeyPatch().setattr(tx, "move", move)
    tx.publish_receipt = publish_receipt
    sync(bundle("2"), rendered(path, "second"), target)


@pytest.mark.parametrize("boundary", ["prepared", "backup", "published", "committed"])
def test_real_process_crash_recovers_full_revision_and_source_free_membership(
    tmp_path: Path, boundary: str
) -> None:
    if os.name == "nt" and boundary == "backup":
        pytest.skip("ReplaceFileW publishes and backs up in one native operation")
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"# foreign\n")
    target = InstallationTarget(tmp_path / "index")
    assert sync(bundle(), rendered(path), target).status is OperationStatus.APPLIED
    process = mp.get_context("spawn").Process(target=crash_worker, args=(str(tmp_path), boundary))
    process.start()
    process.join(15)
    try:
        assert process.exitcode == 77
        status = inspect_installation("team", target)
        assert status.pending and status.resources[0].recovery_paths
        assert remove("team", target).status is OperationStatus.APPLIED
        assert path.read_bytes() == b"# foreign\n"
        assert remove("team", target).status is OperationStatus.UNCHANGED
    finally:
        if process.is_alive():
            process.kill()
            process.join()


@pytest.mark.parametrize("seam", ["prepare", "backup", "staging", "published", "receipt"])
def test_foreign_edits_at_publication_boundaries_are_never_overwritten(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, seam: str
) -> None:
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"# foreign\n")
    target = InstallationTarget(tmp_path / "index")
    assert sync(bundle(), rendered(path), target).status is OperationStatus.APPLIED
    original_prepare = tx.prepare
    original_move = native_move
    original_replace = native_replace_file
    original_publish_receipt = tx.publish_receipt
    foreign = b"# unexpected foreign credential\n"
    injected = False

    def append_foreign(destination: Path) -> None:
        nonlocal injected
        destination.write_bytes(destination.read_bytes() + foreign)
        injected = True

    def prepare(plan: ResourcePlan) -> tx.Journal:
        journal = original_prepare(plan)
        if seam == "prepare":
            append_foreign(path)
        elif seam == "staging":
            stage = tx.paths(journal.resource)[1]
            append_foreign(stage)
        return journal

    def move(source: Path, destination: Path, revision: Revision) -> None:
        original_move(source, destination, revision)
        if seam == "backup" and destination.parent.name == "backup":
            append_foreign(destination)

    def replace_file(destination: Path, staged: Path, backup: Path) -> None:
        original_replace(destination, staged, backup)
        if seam == "backup":
            append_foreign(backup)

    def publish_receipt(journal: tx.Journal) -> None:
        nonlocal injected
        if seam == "published":
            append_foreign(path)
        if seam == "receipt":
            injected = True
            raise OSError(errno.EIO, "receipt unavailable")
        original_publish_receipt(journal)

    monkeypatch.setattr(lifecycle, "prepare", prepare)
    monkeypatch.setattr(tx, "move", move)
    monkeypatch.setattr(tx, "replace_file", replace_file)
    monkeypatch.setattr(tx, "publish_receipt", publish_receipt)
    result = sync(bundle("2"), rendered(path, "second"), target)
    assert injected
    if seam == "receipt":
        assert result.status is OperationStatus.FAILED
        assert (
            path.read_bytes()
            == b"# foreign\n\n<!-- flyrail:guide:start -->\ngenerated\n<!-- flyrail:guide:end -->\n"
        )
    else:
        assert result.status is OperationStatus.INCOMPLETE
        ref = result.resources[0].resource
        locations = [ref.destination, tx.paths(ref)[1], tx.paths(ref)[2]]
        assert any(item.exists() and foreign in item.read_bytes() for item in locations)
        assert tx.paths(ref)[0].exists()


def test_later_edits_after_receipt_commit_survive_cleanup_retry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "AGENTS.md"
    target = InstallationTarget(tmp_path / "index")
    original_cleanup = tx.cleanup

    def fail(journal: tx.Journal) -> None:
        path.write_bytes(path.read_bytes() + b"user after commit\n")
        raise OSError(errno.EIO, "cleanup failed")

    monkeypatch.setattr(lifecycle, "cleanup", fail)
    result = sync(bundle(), rendered(path), target)
    assert result.resources[0].status is OperationStatus.APPLIED
    assert b"user after commit" in path.read_bytes()
    monkeypatch.setattr(lifecycle, "cleanup", original_cleanup)
    assert remove("team", target).status is OperationStatus.APPLIED
    assert path.read_bytes() == b"user after commit\n"


def test_partial_index_union_and_dependency_retirement_preserve_referenced_old_asset(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "AGENTS.md"
    old_asset = tmp_path / "asset-v1"
    new_asset = tmp_path / "asset-v2"
    target = InstallationTarget(tmp_path / "index")

    def revision(asset: Path) -> RenderedBundle:
        return RenderedBundle(
            [
                RenderedArtifact("asset", Family.HOOKS, asset, FileContent(asset.name.encode())),
                *rendered(path, asset.name).artifacts,
            ],
            [Dependency("guide", "asset")],
        )

    assert sync(bundle(), revision(old_asset), target).status is OperationStatus.APPLIED
    original_publish = tx.publish

    def fail(journal: tx.Journal) -> None:
        if journal.resource.destination == path:
            raise OSError(errno.EIO, "reference publication failed")
        original_publish(journal)

    monkeypatch.setattr(lifecycle, "publish", fail)
    result = sync(bundle("2"), revision(new_asset), target)
    assert result.status is OperationStatus.PARTIAL
    assert new_asset.read_bytes() == b"asset-v2" and old_asset.read_bytes() == b"asset-v1"
    assert b"asset-v1" in path.read_bytes()
    status = inspect_installation("team", target)
    assert status.pending and len(status.resources) == 3
    monkeypatch.setattr(lifecycle, "publish", original_publish)
    assert remove("team", target).status is OperationStatus.APPLIED
    assert old_asset.exists() is False and new_asset.exists() is False and path.exists() is False


def test_equal_byte_adoption_journals_ownership_transition(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"<!-- flyrail:guide:start -->\ngenerated\n<!-- flyrail:guide:end -->\n")
    target = InstallationTarget(tmp_path / "index")
    before = observe(path)
    journals = []
    original = tx.prepare

    def capture(plan: ResourcePlan) -> tx.Journal:
        journal = original(plan)
        journals.append(journal)
        return journal

    monkeypatch.setattr(lifecycle, "prepare", capture)
    assert (
        sync(bundle(), rendered(path), target, acquisition=Acquisition.ADOPT).status
        is OperationStatus.APPLIED
    )
    assert (
        len(journals) == 1 and journals[0].previous is None and len(journals[0].receipt.claims) == 1
    )
    assert observe(path) == before
    assert receipt(path).claims[0].disposition == "adopted"
