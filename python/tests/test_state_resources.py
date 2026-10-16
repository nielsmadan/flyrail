import errno
import os
import re
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest
from state_helpers import bundle, journal_directory, rendered, target, transaction_directories

import flyrail._lifecycle as lifecycle
import flyrail._resource_io as resource_io
import flyrail._resource_transaction as transaction
import flyrail._state as state
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
    MemoryStore,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    SectionContent,
    SQLiteStore,
    StateStore,
    TreeContent,
    apply_preview,
    inspect_installation,
    preview,
    recover_installation,
    remove,
    sync,
)
from flyrail._codec import decode, encode
from flyrail._resource_models import Preparation, Revision
from flyrail._state import destination_key, resource_key
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
        assert transaction_directories(destination, selected.store) == []
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


@pytest.mark.parametrize(
    "fragment", ["index/", "/authority", "/preparation", "/journal", "/receipt"]
)
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
    assert transaction_directories(destination, store) == []
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
        if removed == "directory" and _transaction_name(path.name):
            raise SystemExit("interrupted cleanup")
        if path.name == removed and _transaction_name(path.parent.name):
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
    assert transaction_directories(destination, selected.store) == []
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


class _Crash:
    fired = False

    def __call__(self) -> None:
        if not self.fired:
            self.fired = True
            raise SystemExit("interrupted preparation")


def _current(module: object, name: str) -> Callable[..., Any]:
    return getattr(module, name)  # type: ignore[no-any-return]


def _transaction_name(name: str) -> bool:
    return _management_name(name) or re.fullmatch("[0-9a-f]{64}-[0-9a-f]{32}", name) is not None


def _crash_at(monkeypatch: pytest.MonkeyPatch, point: str) -> _Crash:
    crash = _Crash()
    if point == "after-preparation":
        original_mkdir = Path.mkdir

        def mkdir(path: Path, *args: Any, **kwargs: Any) -> None:
            if _transaction_name(path.name):
                crash()
            original_mkdir(path, *args, **kwargs)

        monkeypatch.setattr(Path, "mkdir", mkdir)
    elif point == "after-directory":
        original_write = resource_io.write_revision

        def write_revision(path: Path, revision: Revision) -> Revision:
            crash()
            return original_write(path, revision)

        monkeypatch.setattr(transaction, "write_revision", write_revision)
    elif point == "after-backup-move":
        original_move = _current(transaction, "move")

        def move(source: Path, destination: Path, revision: Revision) -> None:
            original_move(source, destination, revision)
            if destination.parent.name == "backup":
                crash()

        monkeypatch.setattr(transaction, "move", move)
    elif point == "after-publish":
        original_rename = _current(transaction, "rename_exclusive")

        def rename_exclusive(source: Path, destination: Path) -> None:
            original_rename(source, destination)
            crash()

        monkeypatch.setattr(transaction, "rename_exclusive", rename_exclusive)
    else:
        original_record = _current(transaction, "write_record")
        suffix = "/receipt" if point == "after-commit" else "/journal"

        def write_record(store: StateStore, key: str, value: object, *, expected: object) -> None:
            if key.endswith(suffix) and point != "after-commit":
                crash()
            original_record(store, key, value, expected=expected)
            if key.endswith(suffix) and point == "after-commit":
                crash()

        monkeypatch.setattr(transaction, "write_record", write_record)
    return crash


@pytest.mark.parametrize("point", ["after-preparation", "after-directory", "after-staging"])
def test_a_crash_before_the_journal_is_cleaned_up_by_the_next_run(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, point: str
) -> None:
    selected, destination = target(tmp_path), tmp_path / "AGENTS.md"
    destination.write_bytes(b"# Foreign\n")
    assert sync(bundle(), rendered(destination, "one"), selected).status is OperationStatus.APPLIED
    crash = _crash_at(monkeypatch, point)
    with pytest.raises(SystemExit):
        sync(bundle("2"), rendered(destination, "two"), selected)
    assert crash.fired
    preparation = selected.store.read(resource_key(destination, "preparation"))
    assert preparation is not None
    assert selected.store.read(resource_key(destination, "journal")) is None
    observation = inspect_installation(bundle().id, selected)
    recorded = decode(preparation)
    assert isinstance(recorded, Preparation)
    assert [r.recovery_paths for r in observation.resources] == [(recorded.directory,)]
    assert (
        sync(bundle("2"), rendered(destination, "two"), selected).status is OperationStatus.APPLIED
    )
    assert selected.store.read(resource_key(destination, "preparation")) is None
    assert sorted(p.name for p in tmp_path.iterdir()) == ["AGENTS.md", "state"]
    assert transaction_directories(destination, selected.store) == []


def _victim(root: Path) -> Path:
    victim = root / "victim"
    for folder in (victim, victim / "staging", victim / "backup"):
        folder.mkdir(mode=0o700)
    (victim / "staging" / "resource").write_bytes(b"victim")
    return victim


def _unrecognized(pending: Path, root: Path, damage: str) -> Path:
    if damage == "symlink":
        victim = root / "victim"
        pending.rename(victim)
        pending.symlink_to(victim, target_is_directory=True)
        return victim / "staging" / "resource"
    if damage == "staging-link":
        victim = _victim(root)
        (pending / "staging" / "resource").unlink()
        (pending / "staging").rmdir()
        (pending / "staging").symlink_to(victim / "staging", target_is_directory=True)
        return victim / "staging" / "resource"
    if damage == "nonprivate":
        pending.chmod(0o755)
        return pending / "staging" / "resource"
    (pending / damage).write_bytes(b"retained")
    return pending / damage


@pytest.mark.skipif(os.name == "nt", reason="POSIX links and private modes")
@pytest.mark.parametrize(
    "damage", ["unknown", "backup/resource", "symlink", "staging-link", "nonprivate"]
)
def test_unrecognized_data_beside_a_preparation_is_retained(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, damage: str
) -> None:
    selected, destination = target(tmp_path), tmp_path / "AGENTS.md"
    assert sync(bundle(), rendered(destination, "one"), selected).status is OperationStatus.APPLIED
    before = destination.read_bytes()
    with monkeypatch.context() as patch:
        _crash_at(patch, "after-staging")
        with pytest.raises(SystemExit):
            sync(bundle("2"), rendered(destination, "two"), selected)
    key = resource_key(destination, "preparation")
    preparation = selected.store.read(key)
    pending = journal_directory(selected.store, destination)
    evidence = _unrecognized(pending, tmp_path, damage)
    retained = evidence.read_bytes()
    result = recover_installation(bundle().id, selected)
    assert result.status is OperationStatus.INCOMPLETE
    assert result.error is not None and result.error.code is ErrorCode.RECOVERY_NEEDED
    assert result.error.path == pending
    assert [item.recovery_paths for item in result.resources] == [(pending,)]
    assert selected.store.read(key) == preparation
    assert evidence.read_bytes() == retained
    assert destination.read_bytes() == before


def _tree_state(directory: Path) -> list[tuple[str, int, bytes | None]]:
    return sorted(
        (
            path.relative_to(directory).as_posix(),
            path.lstat().st_mode,
            path.read_bytes() if path.is_file() else None,
        )
        for path in directory.rglob("*")
    )


def test_a_journal_without_its_preparation_is_retained_as_invalid_state(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    selected, destination = target(tmp_path), tmp_path / "AGENTS.md"
    assert sync(bundle(), rendered(destination, "one"), selected).status is OperationStatus.APPLIED

    def crash(store: StateStore, journal: transaction.Journal) -> None:
        raise SystemExit("interrupted after the publish move")

    with monkeypatch.context() as patch:
        patch.setattr(transaction, "publish_receipt", crash)
        with pytest.raises(SystemExit):
            sync(bundle("2"), rendered(destination, "two"), selected)
    pending = journal_directory(selected.store, destination)
    preparation_key = resource_key(destination, "preparation")
    journal_key = resource_key(destination, "journal")
    recorded = selected.store.read(preparation_key)
    assert recorded is not None
    assert selected.store.delete(preparation_key, expected=recorded)
    journal, published, layout = (
        selected.store.read(journal_key),
        destination.read_bytes(),
        _tree_state(pending),
    )
    assert b"two" in published
    observation = inspect_installation(bundle().id, selected)
    recovered = recover_installation(bundle().id, selected)
    synced = sync(bundle("2"), rendered(destination, "two"), selected)
    assert [item.error.code for item in observation.resources if item.error] == [
        ErrorCode.INVALID_STATE
    ]
    for result in (recovered, synced):
        assert result.status is OperationStatus.INCOMPLETE
        assert result.error is not None and result.error.code is ErrorCode.INVALID_STATE
    assert selected.store.read(journal_key) == journal
    assert _tree_state(pending) == layout
    assert destination.read_bytes() == published


def test_cleanup_removes_the_preparation_after_the_journal(tmp_path: Path) -> None:
    selected, destination = target(tmp_path), tmp_path / "AGENTS.md"
    assert sync(bundle(), rendered(destination), selected).status is OperationStatus.APPLIED
    assert [
        k for k in selected.store.keys("resource/") if k.endswith(("/journal", "/preparation"))
    ] == []


def test_a_cleanup_interrupted_before_deleting_the_preparation_is_recovered(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    selected, destination = target(tmp_path), tmp_path / "AGENTS.md"
    assert sync(bundle(), rendered(destination, "one"), selected).status is OperationStatus.APPLIED
    crash, original = _Crash(), state.delete_record

    def delete_record(store: StateStore, key: str, *, expected: object) -> None:
        if key.endswith("/preparation"):
            crash()
        original(store, key, expected=expected)

    with monkeypatch.context() as patch:
        patch.setattr(transaction, "delete_record", delete_record)
        with pytest.raises(SystemExit):
            sync(bundle("2"), rendered(destination, "two"), selected)
    assert selected.store.read(resource_key(destination, "journal")) is None
    assert selected.store.read(resource_key(destination, "preparation")) is not None
    assert recover_installation(bundle().id, selected).status is OperationStatus.APPLIED
    assert selected.store.read(resource_key(destination, "preparation")) is None
    assert b"two" in destination.read_bytes()
    assert sorted(p.name for p in tmp_path.iterdir()) == ["AGENTS.md", "state"]
    assert transaction_directories(destination, selected.store) == []


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

    def publish(store: StateStore, journal: transaction.Journal) -> None:
        run_second(transaction.transaction_directory(store, journal))
        original_publish(store, journal)

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
    assert transaction_directories(destination, store) == []
    assert sync(bundle("2"), desired("two"), selected).status is OperationStatus.APPLIED
    assert b"two" in (destination if kind == "document" else destination / "a.txt").read_bytes()


def _record_listings(monkeypatch: pytest.MonkeyPatch, project: Path) -> list[list[str]]:
    listings: list[list[str]] = []

    def recorded(original: Callable[..., Any]) -> Callable[..., Any]:
        def wrapper(*args: Any, **kwargs: Any) -> Any:
            listings.append(sorted(p.name for p in project.iterdir()))
            return original(*args, **kwargs)

        return wrapper

    for module, name in (
        (transaction, "write_record"),
        (transaction, "delete_record"),
        (transaction, "rename_exclusive"),
        (resource_io, "write_record"),
        (resource_io, "rename_exclusive"),
        (os, "replace"),
    ):
        monkeypatch.setattr(module, name, recorded(getattr(module, name)))
    return listings


@pytest.mark.parametrize(
    "point",
    ["after-directory", "after-staging", "after-backup-move", "after-publish", "after-commit"],
)
def test_a_same_filesystem_store_never_creates_anything_beside_the_destination(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, point: str
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    destination = project / "AGENTS.md"
    destination.write_bytes(b"# Foreign\n")
    selected = InstallationTarget(DirectoryStore(tmp_path / "state"))
    listings = _record_listings(monkeypatch, project)
    assert sync(bundle(), rendered(destination, "one"), selected).status is OperationStatus.APPLIED
    crash = _crash_at(monkeypatch, point)
    with pytest.raises(SystemExit):
        sync(bundle("2"), rendered(destination, "two"), selected)
    assert crash.fired
    assert listings
    assert all(set(listing) <= {"AGENTS.md"} for listing in listings)
    assert {p.name for p in project.iterdir()} <= {"AGENTS.md"}
    assert (
        sync(bundle("2"), rendered(destination, "two"), selected).status is OperationStatus.APPLIED
    )
    assert all(set(listing) <= {"AGENTS.md"} for listing in listings)
    assert sorted(p.name for p in project.iterdir()) == ["AGENTS.md"]
    assert transaction_directories(destination, selected.store) == []
    assert (tmp_path / "state" / "staging").is_dir()
    assert b"two" in destination.read_bytes()


def _capture_transaction_directories(
    monkeypatch: pytest.MonkeyPatch, destination: Path
) -> list[Path]:
    directories: list[Path] = []
    real = _current(transaction, "write_record")

    def capture(store_: StateStore, key: str, value: object, *, expected: object | None) -> None:
        if key.endswith("/journal"):
            preparation = decode(store_.read(resource_key(destination, "preparation")) or b"")
            assert isinstance(preparation, Preparation)
            directories.append(preparation.directory)
            assert preparation.directory.is_dir()
        real(store_, key, value, expected=expected)

    monkeypatch.setattr(transaction, "write_record", capture)
    return directories


def test_a_store_without_a_staging_directory_falls_back_beside_the_destination(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    destination = tmp_path / "AGENTS.md"
    directories = _capture_transaction_directories(monkeypatch, destination)
    selected = InstallationTarget(MemoryStore())
    assert sync(bundle(), rendered(destination), selected).status is OperationStatus.APPLIED
    assert [d.parent for d in directories] == [tmp_path]
    assert directories[0].name.startswith(".flyrail-") and directories[0].name.endswith(".txn")
    assert sorted(p.name for p in tmp_path.iterdir()) == ["AGENTS.md"]


def test_a_store_on_another_filesystem_falls_back_beside_the_destination(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    destination = project / "AGENTS.md"
    store_root = tmp_path / "state"
    real_device = state._device
    monkeypatch.setattr(
        state,
        "_device",
        lambda path: -1 if store_root in (path, *path.parents) else real_device(path),
    )
    directories = _capture_transaction_directories(monkeypatch, destination)
    selected = InstallationTarget(DirectoryStore(store_root))
    assert sync(bundle(), rendered(destination), selected).status is OperationStatus.APPLIED
    assert [d.parent for d in directories] == [project]
    assert _management_name(directories[0].name)
    assert sorted(p.name for p in project.iterdir()) == ["AGENTS.md"]


def test_a_same_filesystem_store_stages_in_its_staging_directory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    destination = tmp_path / "AGENTS.md"
    directories = _capture_transaction_directories(monkeypatch, destination)
    selected = target(tmp_path)
    assert sync(bundle(), rendered(destination), selected).status is OperationStatus.APPLIED
    assert [d.parent for d in directories] == [tmp_path / "state" / "staging"]
    assert directories[0].name.startswith(destination_key(destination) + "-")
    assert list((tmp_path / "state" / "staging").iterdir()) == []


@pytest.mark.skipif(os.name == "nt", reason="POSIX private modes")
@pytest.mark.parametrize("named", ["elsewhere", "another-transaction", "another-destination"])
def test_a_preparation_naming_a_foreign_directory_is_refused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, named: str
) -> None:
    selected, destination = target(tmp_path), tmp_path / "AGENTS.md"
    assert sync(bundle(), rendered(destination, "one"), selected).status is OperationStatus.APPLIED
    before = destination.read_bytes()
    with monkeypatch.context() as patch:
        _crash_at(patch, "after-staging")
        with pytest.raises(SystemExit):
            sync(bundle("2"), rendered(destination, "two"), selected)
    key = resource_key(destination, "preparation")
    recorded = decode(selected.store.read(key) or b"")
    assert isinstance(recorded, Preparation)
    decoy = {
        "elsewhere": tmp_path / "decoy",
        "another-transaction": recorded.directory.with_name(
            f"{destination_key(destination)}-{'f' * 32}"
        ),
        "another-destination": transaction_path(tmp_path / "CLAUDE.md", recorded.transaction_id),
    }[named]
    victim = _victim(tmp_path)
    victim.rename(decoy)
    corrupted = encode(Preparation(recorded.resource, recorded.transaction_id, decoy))
    assert selected.store.write(key, corrupted, expected=encode(recorded))
    for result in (
        recover_installation(bundle().id, selected),
        sync(bundle("2"), rendered(destination, "two"), selected),
    ):
        assert result.status is OperationStatus.INCOMPLETE
        assert result.error is not None and result.error.code is ErrorCode.RECOVERY_NEEDED
        assert all(item.recovery_paths == () for item in result.resources)
    assert selected.store.read(key) == corrupted
    assert (decoy / "staging" / "resource").read_bytes() == b"victim"
    assert destination.read_bytes() == before


def _crash_then_move_store(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, point: str
) -> tuple[InstallationTarget, Path, Preparation]:
    destination = tmp_path / "AGENTS.md"
    original = target(tmp_path)
    assert sync(bundle(), rendered(destination, "one"), original).status is OperationStatus.APPLIED
    with monkeypatch.context() as patch:
        _crash_at(patch, point)
        with pytest.raises(SystemExit):
            sync(bundle("2"), rendered(destination, "two"), original)
    recorded = decode(original.store.read(resource_key(destination, "preparation")) or b"")
    assert isinstance(recorded, Preparation)
    assert original.store.read(resource_key(destination, "journal")) is None
    (tmp_path / "state").rename(tmp_path / "moved")
    return InstallationTarget(DirectoryStore(tmp_path / "moved"), ()), destination, recorded


@pytest.mark.parametrize("operation", ["recover", "sync"])
def test_a_preparation_left_behind_by_a_moved_store_is_released(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, operation: str
) -> None:
    moved, destination, recorded = _crash_then_move_store(
        tmp_path, monkeypatch, "after-preparation"
    )
    assert not os.path.lexists(recorded.directory)
    key = resource_key(destination, "preparation")
    if operation == "recover":
        assert recover_installation(bundle().id, moved).status is OperationStatus.APPLIED
        assert moved.store.read(key) is None
    result = sync(bundle("2"), rendered(destination, "two"), moved)
    assert result.status is OperationStatus.APPLIED
    assert moved.store.read(key) is None
    assert b"two" in destination.read_bytes()
    assert transaction_directories(destination, moved.store) == []


@pytest.mark.skipif(os.name == "nt", reason="POSIX private modes")
def test_a_moved_store_refuses_a_preparation_whose_old_directory_still_exists(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    moved, destination, recorded = _crash_then_move_store(tmp_path, monkeypatch, "after-staging")
    before = destination.read_bytes()
    recorded.directory.parent.mkdir(parents=True, mode=0o700)
    (tmp_path / "moved" / "staging" / recorded.directory.name).rename(recorded.directory)
    retained = _tree_state(recorded.directory)
    key = resource_key(destination, "preparation")
    preparation = moved.store.read(key)
    for result in (
        recover_installation(bundle().id, moved),
        sync(bundle("2"), rendered(destination, "two"), moved),
    ):
        assert result.status is OperationStatus.INCOMPLETE
        assert result.error is not None and result.error.code is ErrorCode.RECOVERY_NEEDED
        assert all(item.recovery_paths == () for item in result.resources)
    assert moved.store.read(key) == preparation
    assert _tree_state(recorded.directory) == retained
    assert destination.read_bytes() == before


@pytest.mark.parametrize("planned", ["under-locks", "before-locks"])
def test_a_staging_directory_another_writer_creates_while_locking_is_not_approved(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, planned: str
) -> None:
    selected, destination = target(tmp_path), tmp_path / "AGENTS.md"
    staging = tmp_path / "state" / "staging"
    proposal = preview(bundle(), rendered(destination), selected)
    original = Path.mkdir

    def mkdir(path: Path, *args: Any, **kwargs: Any) -> None:
        original(path, *args, **kwargs)
        if path == staging:
            raise FileExistsError(errno.EEXIST, "created by another writer", os.fspath(path))

    monkeypatch.setattr(Path, "mkdir", mkdir)
    result = (
        sync(bundle(), rendered(destination), selected)
        if planned == "under-locks"
        else apply_preview(proposal)
    )
    monkeypatch.undo()
    if planned == "under-locks":
        assert result.status is OperationStatus.APPLIED
    else:
        assert result.status is OperationStatus.FAILED
        assert result.error is not None and result.error.code is ErrorCode.CONCURRENT_CHANGE
        assert not destination.exists()
    assert staging.is_dir()


class _UnreadableRecords(DirectoryStore):
    unreadable: tuple[str, ...] = ()

    def read(self, key: str) -> bytes | None:
        if key.endswith(self.unreadable):
            raise OSError(errno.EIO, "injected store read failure")
        return super().read(key)


@pytest.mark.parametrize(
    ("unreadable", "reported"),
    [(("/journal",), True), (("/journal", "/preparation"), False)],
)
def test_an_unreadable_journal_reports_only_the_directory_its_preparation_names(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    unreadable: tuple[str, ...],
    reported: bool,
) -> None:
    store = _UnreadableRecords(tmp_path / "state")
    selected, destination = InstallationTarget(store), tmp_path / "AGENTS.md"
    assert sync(bundle(), rendered(destination, "one"), selected).status is OperationStatus.APPLIED
    directories = _capture_transaction_directories(monkeypatch, destination)
    original = _current(lifecycle, "publish")

    def publish(store_: StateStore, journal: transaction.Journal) -> None:
        store.unreadable = unreadable
        original(store_, journal)

    monkeypatch.setattr(lifecycle, "publish", publish)
    result = sync(bundle("2"), rendered(destination, "two"), selected)
    monkeypatch.undo()
    store.unreadable = ()
    (directory,) = directories
    assert directory.parent == tmp_path / "state" / "staging"
    assert [item.recovery_paths for item in result.resources] == [(directory,) if reported else ()]
    assert result.status is (OperationStatus.INCOMPLETE if reported else OperationStatus.FAILED)
    assert recover_installation(bundle().id, selected).status is OperationStatus.APPLIED
    assert (
        sync(bundle("2"), rendered(destination, "two"), selected).status is OperationStatus.APPLIED
    )
    assert b"two" in destination.read_bytes()


def _mount_line(identifier: int, path: Path) -> str:
    point = path.as_posix().replace("\\", "\\134").replace(" ", "\\040")
    return f"{identifier} 1 8:1 / {point} rw - ext4 /dev/sda1 rw"


@pytest.mark.parametrize("mounts", ["bind", "shared"])
def test_a_store_on_another_mount_of_the_same_device_falls_back_beside_the_destination(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mounts: str
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    destination = project / "AGENTS.md"
    store_root = tmp_path / "state"
    lines = ["1 0 8:1 / / rw - ext4 /dev/sda1 rw", _mount_line(2, tmp_path)]
    if mounts == "bind":
        lines += [_mount_line(3, store_root), _mount_line(4, project)]
    monkeypatch.setattr(state, "_mountinfo", lambda: "\n".join(lines) + "\n")
    directories = _capture_transaction_directories(monkeypatch, destination)
    selected = InstallationTarget(DirectoryStore(store_root))
    assert sync(bundle(), rendered(destination), selected).status is OperationStatus.APPLIED
    expected = project if mounts == "bind" else store_root / "staging"
    assert [d.parent for d in directories] == [expected]
    assert sorted(p.name for p in project.iterdir()) == ["AGENTS.md"]
    assert transaction_directories(destination, selected.store) == []
