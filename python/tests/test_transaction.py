import os
import stat
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import IO, BinaryIO

import pytest
from test_inspection import make_bundle, materialize, snapshot

from flyrail import BundleEntry, ErrorCode, OperationStatus, Target, install, update
from flyrail import _transaction as transaction
from flyrail._filesystem import rename_exclusive
from flyrail._inventory import inventory
from flyrail._observation import ObservationFailure


@pytest.mark.skipif(os.name == "nt", reason="POSIX mode barrier")
@pytest.mark.parametrize("permissive_state", [False, True])
def test_private_management_directories_exist_before_staging_and_backup(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, permissive_state: bool
) -> None:
    old = make_bundle(tmp_path / "old", executable=True)
    new = make_bundle(tmp_path / "new", data=b"replacement", executable=True)
    public = tmp_path / "public"
    public.mkdir(mode=0o755)
    public.chmod(0o755)
    target = Target.directory(public / "skills")
    target.root.mkdir(mode=0o700)
    state = public / ".skills.flyrail"
    containers = (state, state / "receipts", state / "staging", state / "backup")
    if permissive_state:
        for path in containers:
            path.mkdir(mode=0o755)
            path.chmod(0o755)
        (state / "lock").write_bytes(b"permanent")
        (state / "lock").chmod(0o640)
    original_open = Path.open
    staged_files: list[Path] = []
    backups: list[Path] = []

    def check_management() -> None:
        assert stat.S_IMODE(public.stat().st_mode) == 0o755
        assert stat.S_IMODE(target.root.stat().st_mode) == 0o700
        for path in containers:
            assert stat.S_IMODE(path.stat().st_mode) == (0o755 if permissive_state else 0o700)

    def checked_open(path: Path, mode: str = "r", buffering: int = -1) -> IO[bytes] | IO[str]:
        if mode == "xb":
            relative = path.relative_to(state / "staging")
            stage = state / "staging" / relative.parts[0]
            backup = state / "backup" / relative.parts[0]
            check_management()
            for barrier in (stage, stage / "skills", backup):
                assert stat.S_IMODE(barrier.stat().st_mode) == 0o700
            staged_files.append(path)
        return original_open(path, mode, buffering=buffering)

    def checked_rename(source: Path, destination: Path) -> None:
        if destination.parent.parent == state / "backup":
            check_management()
            assert stat.S_IMODE(destination.parent.stat().st_mode) == 0o700
            assert (source / "SKILL.md").read_bytes() == b"original"
            rename_exclusive(source, destination)
            assert stat.S_IMODE(destination.parent.stat().st_mode) == 0o700
            assert (destination / "SKILL.md").read_bytes() == b"original"
            backups.append(destination)
        else:
            rename_exclusive(source, destination)

    previous_umask = os.umask(0o022)
    try:
        monkeypatch.setattr(Path, "open", checked_open)
        monkeypatch.setattr(transaction, "rename_exclusive", checked_rename)
        assert install(old, [target])[0].status is OperationStatus.APPLIED
        lock_before = (state / "lock").stat()
        assert update(new, [target])[0].status is OperationStatus.APPLIED
    finally:
        os.umask(previous_umask)

    assert len(staged_files) == 4
    assert len(backups) == 1
    check_management()
    lock_after = (state / "lock").stat()
    assert (lock_after.st_ino, lock_after.st_mode) == (lock_before.st_ino, lock_before.st_mode)
    assert stat.S_IMODE((target.root / "review/SKILL.md").stat().st_mode) == 0o644
    assert stat.S_IMODE((target.root / "review/run").stat().st_mode) == 0o755


def test_cleanup_read_volume_scales_with_inventory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    original_open = Path.open
    bytes_read = 0
    read_calls = 0

    class MeteredFile:
        def __init__(self, stream: BinaryIO) -> None:
            self.stream = stream

        def fileno(self) -> int:
            return self.stream.fileno()

        def read(self) -> bytes:
            nonlocal bytes_read, read_calls
            data = self.stream.read()
            bytes_read += len(data)
            read_calls += 1
            return data

    @contextmanager
    def measured_open(path: Path, mode: str = "r") -> Iterator[MeteredFile]:
        assert mode == "rb"
        with original_open(path, "rb") as stream:
            yield MeteredFile(stream)

    volumes: list[int] = []
    payload = b"x" * 4096
    for file_count in (25, 100):
        root = tmp_path / str(file_count)
        (root / "review").mkdir(parents=True)
        entries = [BundleEntry("review")]
        for index in range(file_count):
            relative = f"review/{index:03}.bin"
            (root / relative).write_bytes(payload)
            entries.append(BundleEntry(relative, payload))
        expected = inventory(entries)
        bytes_read = read_calls = 0
        with monkeypatch.context() as patch:
            patch.setattr(Path, "open", measured_open)
            transaction.delete_tree(root, "review", expected)
        assert list(root.iterdir()) == []
        assert file_count <= read_calls <= 4 * file_count
        assert file_count * len(payload) <= bytes_read <= 4 * file_count * len(payload)
        volumes.append(bytes_read)
    assert volumes[1] <= 5 * volumes[0]


def test_cleanup_validates_complete_inventory_before_deleting(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "cleanup")
    materialize(bundle, target, receipt=False)
    (target.root / "review/SKILL.md").write_bytes(b"changed")
    before = snapshot(target.root)
    with pytest.raises(ObservationFailure) as failure:
        transaction.delete_tree(target.root, "review", inventory(bundle.entries))
    assert failure.value.error.code is ErrorCode.RECOVERY_NEEDED
    assert snapshot(target.root) == before


@pytest.mark.parametrize(
    "damage",
    [
        "bytes",
        "type",
        "spelling",
        "collision",
        "unknown",
        pytest.param("executable", marks=pytest.mark.skipif(os.name == "nt", reason="POSIX modes")),
        pytest.param(
            "ancestor", marks=pytest.mark.skipif(os.name == "nt", reason="POSIX symlinks")
        ),
    ],
)
def test_cleanup_revalidates_remaining_entries_before_delete(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, damage: str
) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "cleanup")
    materialize(bundle, target, receipt=False)
    skill = target.root / "review"
    changed = skill / "SKILL.md"
    original_delete = transaction._delete_entry
    original_iterdir = Path.iterdir
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "SKILL.md").write_bytes(b"original")
    before_outside = snapshot(outside)
    deleted = False

    def colliding_entries(path: Path) -> Iterator[Path]:
        children = list(original_iterdir(path))
        if deleted and path == skill:
            children.append(skill / "skill.md")
        return iter(children)

    def edit_after_delete(path: Path, directory: bool) -> None:
        nonlocal deleted
        original_delete(path, directory)
        if path != skill / "run":
            return
        deleted = True
        if damage == "bytes":
            changed.write_bytes(b"modified")
        elif damage == "type":
            changed.unlink()
            changed.mkdir()
        elif damage == "spelling":
            changed.rename(skill / "skill.md")
        elif damage == "executable":
            changed.chmod(0o755)
        elif damage == "unknown":
            (skill / "unexpected").write_bytes(b"keep")
        elif damage == "ancestor":
            skill.rename(target.root / "saved")
            skill.symlink_to(outside, target_is_directory=True)

    monkeypatch.setattr(transaction, "_delete_entry", edit_after_delete)
    if damage == "collision":
        monkeypatch.setattr(Path, "iterdir", colliding_entries)
    with pytest.raises(ObservationFailure) as failure:
        transaction.delete_tree(target.root, "review", inventory(bundle.entries))
    assert deleted
    assert failure.value.error.code in {ErrorCode.UNSAFE_PATH, ErrorCode.RECOVERY_NEEDED}
    assert snapshot(outside) == before_outside
    if damage == "bytes":
        assert changed.read_bytes() == b"modified"
    elif damage == "type":
        assert changed.is_dir()
    elif damage == "spelling":
        assert (skill / "skill.md").read_bytes() == b"original"
    elif damage == "unknown":
        assert (skill / "unexpected").read_bytes() == b"keep"
    elif damage == "ancestor":
        assert (target.root / "saved/SKILL.md").read_bytes() == b"original"
        assert skill.is_symlink()
    else:
        assert changed.read_bytes() == b"original"
        if damage == "executable":
            assert changed.stat().st_mode & 0o111
