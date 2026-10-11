import errno
import os
from collections.abc import Callable
from pathlib import Path

import pytest
from state_helpers import bundle, journal_directory, rendered, target, transaction_directories

import flyrail._lifecycle as lifecycle
import flyrail._resource_io as resource_io
import flyrail._resource_transaction as transaction
from flyrail import (
    Acquisition,
    Bundle,
    BundleIdentity,
    DirectoryStore,
    ErrorCode,
    Family,
    InstallationResult,
    InstallationTarget,
    InstructionArtifact,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    SectionContent,
    SQLiteStore,
    TreeContent,
    inspect_installation,
    recover_installation,
    remove,
    sync,
)
from flyrail._resource_models import Revision
from flyrail._state import resource_key
from flyrail.authority import _management_name, transaction_path
from flyrail.models import BundleEntry

pytestmark = pytest.mark.integration


def test_nothing_persistent_is_left_beside_the_destination(tmp_path: Path) -> None:
    selected, destination = target(tmp_path), tmp_path / "AGENTS.md"
    destination.write_text("foreign\n")
    steps: tuple[Callable[[], InstallationResult], ...] = (
        lambda: sync(bundle(), rendered(destination, "one"), selected),
        lambda: sync(bundle("2"), rendered(destination, "two"), selected),
        lambda: remove(bundle().id, selected),
    )
    for step in steps:
        assert step().status is OperationStatus.APPLIED
        assert sorted(path.name for path in tmp_path.iterdir()) == ["AGENTS.md", "state"]
    assert destination.read_text() == "foreign\n"


def test_an_unwritable_store_fails_without_raising(tmp_path: Path) -> None:
    selected, destination = target(tmp_path), tmp_path / "AGENTS.md"
    (tmp_path / "state").mkdir()
    (tmp_path / "state").chmod(0o500)
    try:
        result = sync(bundle(), rendered(destination), selected)
    finally:
        (tmp_path / "state").chmod(0o700)
    assert result.status is OperationStatus.FAILED
    assert result.error is not None and result.error.code is ErrorCode.IO_ERROR
    assert not destination.exists()


@pytest.mark.skipif(os.name == "nt", reason="POSIX private modes")
@pytest.mark.parametrize("kind", ["directory", "sqlite"])
def test_a_store_others_can_write_fails_without_raising(tmp_path: Path, kind: str) -> None:
    destination = tmp_path / "AGENTS.md"
    location = tmp_path / ("state" if kind == "directory" else "state.sqlite3")
    if kind == "directory":
        location.mkdir(mode=0o700)
        location.chmod(0o777)
    else:
        location.touch(mode=0o600)
        location.chmod(0o666)
    store = DirectoryStore(location) if kind == "directory" else SQLiteStore(location)
    result = sync(bundle(), rendered(destination), InstallationTarget(store))
    assert result.status is OperationStatus.FAILED
    assert result.error is not None and result.error.code is ErrorCode.IO_ERROR
    assert result.error.path == location
    assert [path.relative_to(tmp_path) for path in tmp_path.rglob("*")] == [Path(location.name)]


class _FailingStore(DirectoryStore):
    def __init__(self, path: Path, fragment: str) -> None:
        super().__init__(path)
        self.fragment = fragment
        self.armed = False

    def write(self, key: str, value: bytes, *, expected: bytes | None) -> bool:
        if self.armed and self.fragment in key:
            raise OSError(errno.EIO, "injected store failure")
        return super().write(key, value, expected=expected)


@pytest.mark.parametrize("fragment", ["index/", "/authority", "/journal", "/receipt"])
def test_a_store_failure_at_each_protocol_point_keeps_the_destination_consistent(
    tmp_path: Path, fragment: str
) -> None:
    store = _FailingStore(tmp_path / "state", fragment)
    selected, destination = InstallationTarget(store), tmp_path / "AGENTS.md"
    second = tmp_path / "CLAUDE.md" if fragment == "/authority" else destination
    assert sync(bundle(), rendered(destination, "one"), selected).status is OperationStatus.APPLIED
    before = destination.read_bytes()
    store.armed = True
    failed = sync(bundle("2"), rendered(second, "two"), selected)
    store.armed = False
    assert failed.status in {OperationStatus.FAILED, OperationStatus.INCOMPLETE}
    assert destination.read_bytes() == before
    assert transaction_directories(destination) == []
    if second != destination:
        assert not second.exists()
    assert sync(bundle("2"), rendered(second, "two"), selected).status is OperationStatus.APPLIED


def test_an_interrupted_transaction_is_reported_and_recovered(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    selected, destination = target(tmp_path), tmp_path / "AGENTS.md"

    def interrupted(store: object, journal: object) -> None:
        raise OSError(errno.EIO, "interrupted")

    monkeypatch.setattr(transaction, "publish_receipt", interrupted)
    monkeypatch.setattr(lifecycle, "recover", lambda store, ref: None)
    sync(bundle(), rendered(destination), selected)
    monkeypatch.undo()
    pending = journal_directory(selected.store, destination)
    observation = inspect_installation(bundle().id, selected)
    assert [resource.recovery_paths for resource in observation.resources] == [(pending,)]
    assert sync(bundle(), rendered(destination), selected).status is OperationStatus.APPLIED
    assert sorted(path.name for path in tmp_path.iterdir()) == ["AGENTS.md", "state"]


@pytest.mark.parametrize("removed", ["staging", "directory"])
def test_cleanup_interrupted_after_removing_transaction_directories_is_recovered(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, removed: str
) -> None:
    selected, destination = target(tmp_path), tmp_path / "AGENTS.md"
    assert sync(bundle(), rendered(destination, "one"), selected).status is OperationStatus.APPLIED
    original = Path.rmdir

    def rmdir(path: Path) -> None:
        original(path)
        if removed == "directory" and _management_name(path.name):
            raise SystemExit("interrupted cleanup")
        if path.name == removed and _management_name(path.parent.name):
            raise SystemExit("interrupted cleanup")

    with monkeypatch.context() as patch:
        patch.setattr(Path, "rmdir", rmdir)
        with pytest.raises(SystemExit):
            sync(bundle("2"), rendered(destination, "two"), selected)
    pending = journal_directory(selected.store, destination)
    observation = inspect_installation(bundle().id, selected)
    assert [resource.recovery_paths for resource in observation.resources] == [(pending,)]
    assert recover_installation(bundle().id, selected).status is OperationStatus.APPLIED
    assert selected.store.read(resource_key(destination, "journal")) is None
    assert transaction_directories(destination) == []
    assert b"two" in destination.read_bytes()
    assert (
        sync(bundle("2"), rendered(destination, "two"), selected).status is OperationStatus.APPLIED
    )


@pytest.mark.parametrize(
    "layout",
    [(), ("staging/",), ("staging/", "backup/"), ("staging/", "backup/", "staging/resource")],
)
def test_a_transaction_directory_without_a_journal_is_left_untouched(
    tmp_path: Path, layout: tuple[str, ...]
) -> None:
    selected, destination = target(tmp_path), tmp_path / "AGENTS.md"
    assert sync(bundle(), rendered(destination, "one"), selected).status is OperationStatus.APPLIED
    unrecorded = transaction_path(destination, "f" * 32)
    unrecorded.mkdir(mode=0o700)
    for name in layout:
        if name.endswith("/"):
            (unrecorded / name).mkdir(mode=0o700)
        else:
            (unrecorded / name).write_bytes(b"retained")
    observation = inspect_installation(bundle().id, selected)
    assert [resource.recovery_paths for resource in observation.resources] == [()]
    assert recover_installation(bundle().id, selected).status is OperationStatus.UNCHANGED
    assert (
        sync(bundle("2"), rendered(destination, "two"), selected).status is OperationStatus.APPLIED
    )
    assert b"two" in destination.read_bytes()
    assert transaction_directories(destination) == [unrecorded]
    assert sorted(
        path.relative_to(unrecorded).as_posix() for path in unrecorded.rglob("*")
    ) == sorted(name.rstrip("/") for name in layout)


def test_a_lost_store_makes_installed_content_foreign(tmp_path: Path) -> None:
    destination = tmp_path / "AGENTS.md"
    sync(bundle(), rendered(destination), target(tmp_path))
    fresh = target(tmp_path / "replacement")
    conflicted = sync(bundle(), rendered(destination), fresh)
    assert conflicted.status is OperationStatus.FAILED
    errors = [conflicted.error, *(item.error for item in conflicted.resources)]
    assert ErrorCode.CONFLICT in [error.code for error in errors if error is not None]
    adopted = sync(bundle(), rendered(destination), fresh, acquisition=Acquisition.ADOPT)
    assert adopted.status is OperationStatus.APPLIED


def _tree(directory: Path) -> list[tuple[str, bytes | None]]:
    return sorted(
        (path.relative_to(directory).as_posix(), path.read_bytes() if path.is_file() else None)
        for path in directory.rglob("*")
    )


@pytest.mark.parametrize("pause", ["after-directory", "after-prepare"])
def test_another_store_leaves_a_live_transaction_directory_untouched(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, pause: str
) -> None:
    destination = tmp_path / "AGENTS.md"
    destination.write_text("# project\n")
    first = InstallationTarget(DirectoryStore(tmp_path / "state-a"))
    second = InstallationTarget(DirectoryStore(tmp_path / "state-b"))
    other_bundle = Bundle.from_artifacts(
        BundleIdentity("other", "1"), [InstructionArtifact("notes", "text")]
    )
    other = RenderedBundle(
        [
            RenderedArtifact(
                "notes", Family.INSTRUCTIONS, destination, SectionContent("notes", "from B")
            )
        ]
    )
    seen: list[tuple[list[tuple[str, bytes | None]], InstallationResult]] = []
    armed = True

    def run_second(directory: Path) -> None:
        nonlocal armed
        if armed:
            armed = False
            before = _tree(directory)
            seen.append((before, sync(other_bundle, other, second)))
            assert _tree(directory) == before

    original_storage, original_publish = transaction._storage, transaction.publish

    def storage(plan: object, directory: Path) -> object:
        run_second(directory)
        return original_storage(plan, directory)  # type: ignore[arg-type]

    def publish(store: object, journal: transaction.Journal) -> None:
        run_second(transaction.transaction_directory(journal))
        original_publish(store, journal)  # type: ignore[arg-type]

    if pause == "after-directory":
        monkeypatch.setattr(transaction, "_storage", storage)
    else:
        monkeypatch.setattr(lifecycle, "publish", publish)
    result = sync(bundle(), rendered(destination, "from A"), first)
    monkeypatch.undo()
    ((contents, from_second),) = seen
    assert from_second.status is OperationStatus.APPLIED
    assert [item.recovery_paths for item in from_second.resources] == [()]
    assert b"from B" in destination.read_bytes()
    assert result.error is not None and result.error.code is ErrorCode.CONCURRENT_CHANGE
    assert (contents == []) is (pause == "after-directory")
    assert recover_installation("other", second).status is OperationStatus.UNCHANGED


class _UnreadableAuthorities(DirectoryStore):
    armed = False

    def read(self, key: str) -> bytes | None:
        if self.armed and key.endswith("/authority"):
            raise OSError(errno.EIO, "injected store read failure")
        return super().read(key)


@pytest.mark.parametrize("kind", ["document", "tree"])
def test_a_store_read_failure_after_staging_discards_the_staged_revision(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, kind: str
) -> None:
    store = _UnreadableAuthorities(tmp_path / "state")
    selected = InstallationTarget(store)
    destination = tmp_path / ("AGENTS.md" if kind == "document" else "out")

    def desired(text: str) -> RenderedBundle:
        if kind == "document":
            return rendered(destination, text)
        tree = TreeContent([BundleEntry("a.txt", text.encode())])
        return RenderedBundle([RenderedArtifact("guide", Family.INSTRUCTIONS, destination, tree)])

    assert sync(bundle(), desired("one"), selected).status is OperationStatus.APPLIED
    before = resource_io.observe(destination)
    original = resource_io.write_revision

    def write_revision(path: Path, revision: Revision) -> Revision:
        result = original(path, revision)
        store.armed = True
        return result

    with monkeypatch.context() as patch:
        patch.setattr(transaction, "write_revision", write_revision)
        failed = sync(bundle("2"), desired("two"), selected)
    store.armed = False
    assert failed.status is OperationStatus.FAILED
    assert failed.error is not None and failed.error.code is ErrorCode.IO_ERROR
    assert resource_io.observe(destination) == before
    assert transaction_directories(destination) == []
    assert sync(bundle("2"), desired("two"), selected).status is OperationStatus.APPLIED
    assert b"two" in (destination if kind == "document" else destination / "a.txt").read_bytes()
