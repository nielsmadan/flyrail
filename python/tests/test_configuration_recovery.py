import errno
import multiprocessing as mp
import os
import sys
from collections.abc import Callable
from dataclasses import replace
from multiprocessing.connection import Connection
from pathlib import Path

import pytest
from skill_helpers import snapshot
from state_helpers import journal_directory, transaction_directories
from test_configuration_lifecycle import bundle, receipt, rendered
from test_state_resources import _crash_at

from flyrail import (
    Acquisition,
    Dependency,
    DirectoryStore,
    ErrorCode,
    Family,
    FileContent,
    InstallationTarget,
    MemoryStore,
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
from flyrail._observation import ObservationFailure
from flyrail._resource_io import move as native_move
from flyrail._resource_io import observe, require_unchanged_since
from flyrail._resource_io import write_revision as native_write_revision
from flyrail._resource_models import Revision
from flyrail._resource_plan import plan_resource as native_plan_resource
from flyrail._security import replace_file as native_replace_file
from flyrail._state import choose_transaction_directory, resource_key
from flyrail.configuration import ResourcePlan
from flyrail.stores import StateStore

pytestmark = pytest.mark.integration


def live_writer(root: str, channel: Connection) -> None:
    original = tx.prepare

    def prepare(store: StateStore, plan: ResourcePlan) -> tx.Journal:
        journal = original(store, plan)
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
            InstallationTarget(DirectoryStore(Path(root) / "state")),
        )
        channel.send(str(result.status))
    finally:
        channel.close()


def test_live_public_writer_blocks_other_index_until_source_free_recovery(tmp_path: Path) -> None:
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"# foreign\n")
    store = DirectoryStore(tmp_path / "state")
    target = InstallationTarget(store)
    other_target = InstallationTarget(store, (("index", "other"),))
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
        assert store.read(resource_key(authority.destination, "journal")) is not None
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
    target = InstallationTarget(DirectoryStore(Path(root) / "index"))
    original_prepare = tx.prepare
    original_move = native_move
    original_publish_receipt = tx.publish_receipt

    def prepare(store: StateStore, plan: ResourcePlan) -> tx.Journal:
        journal = original_prepare(store, plan)
        if boundary == "prepared":
            os._exit(77)
        return journal

    def move(source: Path, destination: Path, revision: Revision) -> None:
        original_move(source, destination, revision)
        if boundary == "backup" and destination.parent.name == "backup":
            os._exit(77)

    def publish_receipt(store: StateStore, journal: tx.Journal) -> None:
        if boundary == "published":
            os._exit(77)
        original_publish_receipt(store, journal)
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
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))
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
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))
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

    def prepare(store: StateStore, plan: ResourcePlan) -> tx.Journal:
        journal = original_prepare(store, plan)
        if seam == "prepare":
            append_foreign(path)
        elif seam == "staging":
            stage = tx.paths(tx.transaction_directory(store, journal))[0]
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

    def publish_receipt(store: StateStore, journal: tx.Journal) -> None:
        nonlocal injected
        if seam == "published":
            append_foreign(path)
        if seam == "receipt":
            injected = True
            raise OSError(errno.EIO, "receipt unavailable")
        original_publish_receipt(store, journal)

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
        locations = [
            ref.destination,
            *tx.paths(journal_directory(target.store, ref.destination)),
        ]
        assert any(item.exists() and foreign in item.read_bytes() for item in locations)
        assert target.store.read(resource_key(ref.destination, "journal")) is not None


def test_later_edits_after_receipt_commit_survive_cleanup_retry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "AGENTS.md"
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))
    original_cleanup = tx.cleanup

    def fail(store: StateStore, journal: tx.Journal) -> None:
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
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))

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

    def fail(store: StateStore, journal: tx.Journal) -> None:
        if journal.resource.destination == path:
            raise OSError(errno.EIO, "reference publication failed")
        original_publish(store, journal)

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
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))
    before = observe(path)
    journals = []
    original = tx.prepare

    def capture(store: StateStore, plan: ResourcePlan) -> tx.Journal:
        journal = original(store, plan)
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
    assert receipt(path, target.store).claims[0].disposition == "adopted"


REPAIRED = b"# mine\n\n<!-- flyrail:guide:start -->\none\n<!-- flyrail:guide:end -->\n"
DRIFTED = b"# mine\n\n<!-- flyrail:guide:start -->\nEDITED\n<!-- flyrail:guide:end -->\n"


class Crash(BaseException):
    pass


def drifted_installation(tmp_path: Path) -> tuple[Path, InstallationTarget]:
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"# mine\n")
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))
    assert sync(bundle(), rendered(path, "one"), target).status is OperationStatus.APPLIED
    path.write_bytes(path.read_bytes().replace(b"one", b"EDITED"))
    assert path.read_bytes() == DRIFTED
    return path, target


def crash_after(
    original: Callable[..., object], when: Callable[..., bool]
) -> Callable[..., object]:
    def crashing(*args: object) -> object:
        result = original(*args)
        if when(*args):
            raise Crash
        return result

    return crashing


def into_backup(source: Path, destination: Path, revision: Revision) -> bool:
    return destination.parent.name == "backup"


@pytest.mark.parametrize("point", ["staged", "backup", "published", "committed"])
def test_interrupted_drift_repair_converges_on_next_sync(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, point: str
) -> None:
    path, target = drifted_installation(tmp_path)
    name, when = {
        "staged": ("write_revision", lambda *args: True),
        "backup": ("move", into_backup),
        "published": ("rename_exclusive", lambda *args: True),
        "committed": ("publish_receipt", lambda *args: True),
    }[point]
    with monkeypatch.context() as patch:
        patch.setattr(tx, name, crash_after(getattr(tx, name), when))
        with pytest.raises(Crash):
            sync(bundle(), rendered(path, "one"), target, replace_modified=True)
    assert path.exists() is (point != "backup")

    result = sync(bundle(), rendered(path, "one"), target, replace_modified=True)

    assert result.status is OperationStatus.APPLIED
    assert path.read_bytes() == REPAIRED
    assert inspect_installation("team", target).resources[0].recovery_paths == ()
    assert sync(bundle(), rendered(path, "one"), target).status is OperationStatus.UNCHANGED


def test_failed_repair_staging_does_not_block_the_next_repair(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path, target = drifted_installation(tmp_path)

    def fail(destination: Path, revision: Revision) -> Revision:
        native_write_revision(destination, revision)
        raise OSError(errno.EIO, "staging interrupted")

    with monkeypatch.context() as patch:
        patch.setattr(tx, "write_revision", fail)
        failed = sync(bundle(), rendered(path, "one"), target, replace_modified=True)
    assert failed.status is OperationStatus.FAILED
    assert transaction_directories(path, target.store) == []

    result = sync(bundle(), rendered(path, "one"), target, replace_modified=True)

    assert result.status is OperationStatus.APPLIED
    assert path.read_bytes() == REPAIRED


def test_drift_repair_commits_a_fresh_transaction_and_noop_keeps_receipt(
    tmp_path: Path,
) -> None:
    path, target = drifted_installation(tmp_path)
    drifted = receipt(path, target.store)

    repaired = sync(bundle(), rendered(path, "one"), target, replace_modified=True)

    assert repaired.status is OperationStatus.APPLIED
    assert repaired.resources[0].status is OperationStatus.APPLIED
    assert path.read_bytes() == REPAIRED
    current = receipt(path, target.store)
    assert current.transaction_id != drifted.transaction_id
    assert replace(current, transaction_id=drifted.transaction_id) == drifted
    assert sync(bundle(), rendered(path, "one"), target).status is OperationStatus.UNCHANGED
    assert receipt(path, target.store) == current


def test_journal_repeating_previous_receipt_is_refused_with_evidence_retained(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path, target = drifted_installation(tmp_path)

    def reuse_previous(*args: object, **kwargs: object) -> ResourcePlan:
        plan = native_plan_resource(*args, **kwargs)  # type: ignore[arg-type]
        assert plan.previous is not None and plan.after != plan.before
        directory = choose_transaction_directory(
            target.store, plan.resource.destination, plan.previous.transaction_id
        )
        return replace(plan, receipt=plan.previous, transaction_directory=directory)

    with monkeypatch.context() as patch:
        patch.setattr(lifecycle, "plan_resource", reuse_previous)
        patch.setattr(tx, "move", crash_after(native_move, into_backup))
        with pytest.raises(Crash):
            sync(bundle(), rendered(path, "one"), target, replace_modified=True)
    journal_key = resource_key(ResourceAuthority(path).destination, "journal")
    journal = target.store.read(journal_key)
    directory = journal_directory(target.store, path)
    staged, backup = tx.paths(directory)
    evidence = (observe(staged), observe(backup))
    assert path.exists() is False and backup.read_bytes() == DRIFTED

    result = recover_installation("team", target)

    assert result.status is OperationStatus.INCOMPLETE
    assert result.error is not None and result.error.code is ErrorCode.RECOVERY_NEEDED
    assert result.resources[0].recovery_paths == (directory,)
    assert path.exists() is False
    assert (observe(staged), observe(backup)) == evidence
    assert target.store.read(journal_key) == journal


def _replacement_setup(
    tmp_path: Path, location: str
) -> tuple[Path, StateStore, InstallationTarget]:
    project = tmp_path / "project"
    project.mkdir()
    destination = project / "AGENTS.md"
    destination.write_bytes(b"# Foreign\n")
    store: StateStore = DirectoryStore(tmp_path / "state") if location == "store" else MemoryStore()
    selected = InstallationTarget(store)
    assert sync(bundle(), rendered(destination, "one"), selected).status is OperationStatus.APPLIED
    return destination, store, selected


def _crash_before_publication(monkeypatch: pytest.MonkeyPatch) -> None:
    def interrupted(store: StateStore, journal: tx.Journal) -> None:
        raise SystemExit("interrupted publication")

    monkeypatch.setattr(lifecycle, "publish", interrupted)


@pytest.mark.parametrize("revision", ["published", "previous"])
@pytest.mark.parametrize("location", ["store", "beside"])
def test_recovery_refuses_a_destination_replaced_after_preparation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, revision: str, location: str
) -> None:
    destination, store, selected = _replacement_setup(tmp_path, location)
    original = destination.read_bytes()
    crash = _crash_at(
        monkeypatch, "after-publish" if revision == "published" else "after-backup-move"
    )
    with pytest.raises(SystemExit):
        sync(bundle("2"), rendered(destination, "two"), selected)
    assert crash.fired
    directory = journal_directory(store, destination)
    if revision == "previous":
        backup = directory / "backup" / "resource"
        destination.write_bytes(backup.read_bytes())
        destination.chmod(backup.stat().st_mode & 0o777)
    later = directory.stat().st_mtime_ns + 60_000_000_000
    os.utime(destination, ns=(later, later))
    replaced = destination.read_bytes()
    result = recover_installation(bundle().id, selected)
    assert result.status is OperationStatus.INCOMPLETE
    assert result.error is not None and result.error.code is ErrorCode.RECOVERY_NEEDED
    assert destination.read_bytes() == replaced
    assert directory.is_dir() and store.read(resource_key(destination, "journal")) is not None
    if revision == "previous":
        assert replaced == original


@pytest.mark.parametrize("location", ["store", "beside"])
def test_recovery_refuses_the_previous_revision_rewritten_in_place_after_preparation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, location: str
) -> None:
    destination, store, selected = _replacement_setup(tmp_path, location)
    original = destination.read_bytes()
    inode = destination.stat().st_ino
    with monkeypatch.context() as patch:
        _crash_before_publication(patch)
        with pytest.raises(SystemExit):
            sync(bundle("2"), rendered(destination, "two"), selected)
    directory = journal_directory(store, destination)
    with destination.open("r+b") as stream:
        stream.truncate(0)
        stream.write(original)
    later = directory.stat().st_mtime_ns + 60_000_000_000
    os.utime(destination, ns=(later, later))
    assert destination.stat().st_ino == inode
    result = recover_installation(bundle().id, selected)
    assert result.status is OperationStatus.INCOMPLETE
    assert result.error is not None and result.error.code is ErrorCode.RECOVERY_NEEDED
    assert result.error.message == "destination revision changed after preparation"
    assert result.error.path == destination
    assert destination.read_bytes() == original
    assert directory.is_dir() and store.read(resource_key(destination, "journal")) is not None


@pytest.mark.parametrize("point", ["before-publication", "after-backup-move", "after-publish"])
@pytest.mark.parametrize("location", ["store", "beside"])
def test_recovery_accepts_its_own_untouched_revisions(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, point: str, location: str
) -> None:
    destination, store, selected = _replacement_setup(tmp_path, location)
    original = destination.read_bytes()
    with monkeypatch.context() as patch:
        if point == "before-publication":
            _crash_before_publication(patch)
        else:
            _crash_at(patch, point)
        with pytest.raises(SystemExit):
            sync(bundle("2"), rendered(destination, "two"), selected)
    assert store.read(resource_key(destination, "journal")) is not None
    recovered = recover_installation(bundle().id, selected)
    assert recovered.status is OperationStatus.APPLIED
    assert destination.read_bytes() == original
    assert (
        sync(bundle("2"), rendered(destination, "two"), selected).status is OperationStatus.APPLIED
    )
    assert b"two" in destination.read_bytes()
    assert transaction_directories(destination, store) == []


@pytest.mark.parametrize("removed", ["staging", "directory"])
@pytest.mark.parametrize("location", ["store", "beside"])
def test_an_interrupted_rollback_cleanup_still_recovers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, removed: str, location: str
) -> None:
    destination, store, selected = _replacement_setup(tmp_path, location)
    original = destination.read_bytes()
    with monkeypatch.context() as patch:
        _crash_at(patch, "after-publish")
        with pytest.raises(SystemExit):
            sync(bundle("2"), rendered(destination, "two"), selected)
    directory = journal_directory(store, destination)
    interrupted = directory if removed == "directory" else directory / "staging"
    native_rmdir = Path.rmdir

    def rmdir(path: Path) -> None:
        native_rmdir(path)
        if path == interrupted:
            raise SystemExit("interrupted cleanup")

    with monkeypatch.context() as patch:
        patch.setattr(Path, "rmdir", rmdir)
        with pytest.raises(SystemExit):
            recover_installation(bundle().id, selected)
    assert store.read(resource_key(destination, "journal")) is not None
    assert recover_installation(bundle().id, selected).status is OperationStatus.APPLIED
    assert destination.read_bytes() == original
    assert store.read(resource_key(destination, "journal")) is None
    assert transaction_directories(destination, store) == []


@pytest.mark.parametrize("platform", ["darwin", "linux"])
def test_birth_time_is_compared_only_on_macos(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, platform: str
) -> None:
    if sys.platform != "darwin":
        pytest.skip("birth time is readable only on macOS")
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"# mine\n")
    revision = observe(path)
    actual = path.lstat()
    boundary = actual.st_mtime_ns
    born = boundary + 60_000_000_000
    replaced = os.stat_result(
        tuple(actual)[:10],
        {"st_mtime_ns": boundary, "st_birthtime": born / 1e9, "st_birthtime_ns": born},
    )
    native_lstat = Path.lstat
    monkeypatch.setattr(
        Path, "lstat", lambda selected: replaced if selected == path else native_lstat(selected)
    )
    monkeypatch.setattr(sys, "platform", platform)
    if platform == "darwin":
        with pytest.raises(ObservationFailure) as refused:
            require_unchanged_since(path, revision, boundary)
        assert refused.value.error.code is ErrorCode.RECOVERY_NEEDED
    else:
        require_unchanged_since(path, revision, boundary)
