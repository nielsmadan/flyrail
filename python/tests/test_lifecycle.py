import errno
import os
import shutil
from dataclasses import FrozenInstanceError
from pathlib import Path

import pytest
from test_inspection import make_bundle, materialize, snapshot

from flyrail import ErrorCode, OperationStatus, Target, inspect, install, uninstall, update
from flyrail import _transaction as transaction
from flyrail._intent import Intent
from flyrail._receipts import read_receipt
from flyrail.inspection import ResolvedTarget


def test_lifecycle_bytes_repeat_labels_content_retirement_and_source_free_uninstall(
    tmp_path: Path,
) -> None:
    old = make_bundle(
        tmp_path / "source",
        version="9",
        names=("review", "retired"),
        data=b"\x00\xff\r\n",
        executable=True,
    )
    target = Target.directory(tmp_path / "skills")
    result = install(old, [target])[0]
    assert result.status is OperationStatus.APPLIED
    assert result.observation.is_current
    assert (target.root / "review/SKILL.md").read_bytes() == b"\x00\xff\r\n"
    assert (target.root / "review/empty").is_dir()
    if os.name != "nt":
        assert (target.root / "review/run").stat().st_mode & 0o111
    before = snapshot(target.root)
    lock_identity = (result.state_root / "lock").stat().st_ino
    assert install(old, [target])[0].status is OperationStatus.UNCHANGED
    assert update(old, [target])[0].status is OperationStatus.UNCHANGED
    label = make_bundle(
        tmp_path / "label",
        version="1",
        names=("review", "retired"),
        data=b"\x00\xff\r\n",
        executable=True,
    )
    refused = install(label, [target])[0]
    assert refused.error is not None and refused.error.code is ErrorCode.UPDATE_REQUIRED
    assert update(label, [target])[0].status is OperationStatus.APPLIED
    assert snapshot(target.root) == before
    changed = make_bundle(tmp_path / "changed", version="1", data=b"new")
    assert update(changed, [target])[0].observation.is_current
    assert sorted(p.name for p in target.root.iterdir()) == ["review"]
    assert (target.root / "review/SKILL.md").read_bytes() == b"new"
    shutil.rmtree(tmp_path / "source")
    shutil.rmtree(tmp_path / "label")
    shutil.rmtree(tmp_path / "changed")
    removed = uninstall("team", [target])[0]
    assert removed.status is OperationStatus.APPLIED
    assert removed.observation.installed is None
    assert removed.observation.version_matches is None
    assert list(target.root.iterdir()) == []
    tombstone = read_receipt((removed.state_root / "receipts/team.json").read_bytes(), "team")
    assert tombstone.removed and tombstone.transaction_id
    assert (removed.state_root / "lock").stat().st_ino == lock_identity
    assert uninstall("team", [target])[0].status is OperationStatus.UNCHANGED
    assert install(changed, [target])[0].observation.is_current
    with pytest.raises(FrozenInstanceError):
        result.status = OperationStatus.FAILED  # type: ignore[misc]


@pytest.mark.parametrize(
    "kind", ["added", "changed", "missing", "missing-root", "type", "root-file"]
)
@pytest.mark.parametrize("operation", ["update", "uninstall"])
def test_modifications_refuse_whole_bundle_and_explicit_override(
    tmp_path: Path, kind: str, operation: str
) -> None:
    old = make_bundle(tmp_path / "old", names=("review", "retired"))
    new = make_bundle(tmp_path / "new", data=b"new")
    target = Target.directory(tmp_path / "skills")
    assert install(old, [target])[0].status is OperationStatus.APPLIED
    skill = target.root / "retired"
    if kind == "added":
        (skill / "added").write_bytes(b"local")
    elif kind == "changed":
        (skill / "SKILL.md").write_bytes(b"local")
    elif kind == "missing":
        (skill / "SKILL.md").unlink()
    elif kind in {"missing-root", "root-file"}:
        shutil.rmtree(skill)
        if kind == "root-file":
            skill.write_bytes(b"local")
    else:
        (skill / "SKILL.md").unlink()
        (skill / "SKILL.md").mkdir()
    before = snapshot(target.root)
    refused = update(new, [target])[0] if operation == "update" else uninstall("team", [target])[0]
    assert refused.status is OperationStatus.FAILED
    assert refused.error is not None and refused.error.code is ErrorCode.MODIFIED
    assert snapshot(target.root) == before
    applied = (
        update(new, [target], replace_modified=True)[0]
        if operation == "update"
        else uninstall("team", [target], replace_modified=True)[0]
    )
    assert applied.status is OperationStatus.APPLIED
    assert sorted(p.name for p in target.root.iterdir()) == (
        ["review"] if operation == "update" else []
    )


def test_update_missing_foreign_receipts_and_independent_target_results(tmp_path: Path) -> None:
    own = make_bundle(tmp_path / "own")
    foreign = make_bundle(tmp_path / "foreign", identifier="foreign", names=("other",))
    target = Target.directory(tmp_path / "skills")
    assert update(own, [target])[0].observation.is_current
    assert install(foreign, [target])[0].observation.is_current
    (target.root / "other/run").write_bytes(b"local")
    new = make_bundle(tmp_path / "new", data=b"new")
    assert update(new, [target])[0].observation.is_current
    assert (target.root / "other/run").read_bytes() == b"local"
    blocked = Target.directory(tmp_path / "blocked")
    materialize(own, blocked, receipt=False)
    bad = Target.directory(tmp_path / "file")
    bad.root.touch()
    good = Target.directory(tmp_path / "good")
    results = install(own, [blocked, good, good, bad])
    assert [r.status for r in results] == [
        OperationStatus.FAILED,
        OperationStatus.APPLIED,
        OperationStatus.APPLIED,
        OperationStatus.FAILED,
    ]
    assert results[0].error is not None and results[0].error.code is ErrorCode.CONFLICT
    assert results[2].alias_of == 1 and results[2].observation is results[1].observation
    assert results[3].error is not None and results[3].error.code is ErrorCode.UNSAFE_PATH
    collision = make_bundle(tmp_path / "collision", identifier="collision")
    shutil.rmtree(target.root / "review")
    rejected = update(collision, [target], replace_modified=True)[0]
    assert rejected.error is not None and rejected.error.code is ErrorCode.CONFLICT
    assert uninstall("team", [target], replace_modified=True)[0].status is OperationStatus.APPLIED
    assert (target.root / "other/run").read_bytes() == b"local"


def test_invalid_requests_do_not_write_any_target(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    before = snapshot(tmp_path)
    for timeout in (-1, float("nan"), float("inf")):
        with pytest.raises(ValueError):
            install(bundle, [target], lock_timeout=timeout)
    with pytest.raises(TypeError):
        install(bundle, [target], lock_timeout=True)
    with pytest.raises(TypeError):
        update(bundle, [target], replace_modified=1)  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        update(None, [target])  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        install(None, [target])  # type: ignore[arg-type]
    with pytest.raises(ValueError):
        uninstall("../team", [target])
    with pytest.raises(ValueError):
        install(bundle, [])
    with pytest.raises(ValueError):
        install(bundle, [target, Target.directory(target.root / "nested")])
    with pytest.raises(ValueError):
        install(bundle, [target, Target.directory(tmp_path / "source/target")])
    with pytest.raises(TypeError):
        install(bundle, [target, "bad"])  # type: ignore[list-item]
    assert snapshot(tmp_path) == before


def test_precommit_failure_rolls_back_and_keeps_independent_success(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    old = make_bundle(tmp_path / "old", names=("review", "retired"))
    new = make_bundle(tmp_path / "new", data=b"new")
    target = Target.directory(tmp_path / "skills")
    install(old, [target])
    original = transaction.publish_receipt

    def fail(selected: ResolvedTarget, intent: Intent) -> None:
        if selected.root == target.root:
            raise OSError(errno.EIO, "injected receipt failure")
        original(selected, intent)

    monkeypatch.setattr(transaction, "publish_receipt", fail)
    results = update(new, [target, Target.directory(tmp_path / "good")])
    assert [r.status for r in results] == [OperationStatus.FAILED, OperationStatus.APPLIED]
    assert inspect(old, [target])[0].observation.is_current
    assert results[0].recovery_paths == ()


def test_postcommit_modified_backup_is_preserved_and_reported(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    old = make_bundle(tmp_path / "old")
    new = make_bundle(tmp_path / "new", data=b"new")
    target = Target.directory(tmp_path / "skills")
    install(old, [target])
    original = transaction.publish_receipt

    def edit_after_commit(selected: ResolvedTarget, intent: Intent) -> None:
        original(selected, intent)
        (selected.state_root / "backup" / intent.transaction_id / "review/SKILL.md").write_bytes(
            b"keep unexpected"
        )

    monkeypatch.setattr(transaction, "publish_receipt", edit_after_commit)
    result = update(new, [target])[0]
    assert result.status is OperationStatus.APPLIED
    assert result.error is not None and result.error.code is ErrorCode.RECOVERY_NEEDED
    assert result.recovery_paths
    assert (target.root / "review/SKILL.md").read_bytes() == b"new"
    backup = next((result.state_root / "backup").iterdir()) / "review/SKILL.md"
    assert backup.read_bytes() == b"keep unexpected"
    repeated = uninstall("team", [target])[0]
    assert repeated.status is OperationStatus.INCOMPLETE
    assert backup.read_bytes() == b"keep unexpected"


@pytest.mark.skipif(os.name == "nt", reason="POSIX filesystem permissions")
@pytest.mark.parametrize("raise_after_commit", [False, True])
def test_unreadable_backup_does_not_hide_receipt_commit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, raise_after_commit: bool
) -> None:
    old = make_bundle(tmp_path / "old")
    new = make_bundle(tmp_path / "new", data=b"new")
    target = Target.directory(tmp_path / "skills")
    installed = install(old, [target])[0]
    backup = installed.state_root / "backup"
    mode = backup.stat().st_mode
    original = transaction.publish_receipt

    def make_backup_unreadable(selected: ResolvedTarget, intent: Intent) -> None:
        original(selected, intent)
        backup.chmod(0)
        if os.access(backup, os.R_OK):
            pytest.skip("current user bypasses filesystem permission checks")
        if raise_after_commit:
            raise OSError(errno.EIO, "receipt published before error", selected.state_root)

    monkeypatch.setattr(transaction, "publish_receipt", make_backup_unreadable)
    try:
        result = update(new, [target])[0]
        assert result.status is OperationStatus.APPLIED
        assert result.error is not None
        assert result.error.code is ErrorCode.IO_ERROR
        assert result.error.errno == (errno.EIO if raise_after_commit else errno.EACCES)
        assert result.recovery_paths == (
            result.state_root / "transaction.json",
            result.state_root / "staging",
            backup,
        )
        receipt = read_receipt((result.state_root / "receipts/team.json").read_bytes(), "team")
        assert receipt.content_digest == new.content_digest
        assert (target.root / "review/SKILL.md").read_bytes() == b"new"
        repeated = uninstall("team", [target])[0]
        assert repeated.status is OperationStatus.INCOMPLETE
        assert repeated.recovery_paths == result.recovery_paths
    finally:
        backup.chmod(mode)
    assert (backup / receipt.transaction_id / "review/SKILL.md").read_bytes() == b"original"
    assert update(new, [target])[0].status is OperationStatus.UNCHANGED


@pytest.mark.parametrize("phase", ["committed", "rollback", "preparation"])
def test_unlock_error_preserves_commit_and_pending_work(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, phase: str
) -> None:
    from flyrail import _filesystem as filesystem

    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    original_lock = filesystem._lock
    original_rename = filesystem.rename_exclusive

    def fail_unlock(fd: int, *, release: bool = False) -> None:
        if release:
            raise OSError(errno.EIO, "unlock failed")
        original_lock(fd)

    def fail_receipt(selected: ResolvedTarget, intent: Intent) -> None:
        raise PermissionError(errno.EACCES, "receipt blocked", selected.state_root)

    def fail_intent(source: Path, destination: Path) -> None:
        if source.name == "intent.json":
            raise PermissionError(errno.EACCES, "intent blocked", destination)
        original_rename(source, destination)

    monkeypatch.setattr(filesystem, "_lock", fail_unlock)
    if phase == "rollback":
        monkeypatch.setattr(transaction, "publish_receipt", fail_receipt)
    elif phase == "preparation":
        monkeypatch.setattr(transaction, "rename_exclusive", fail_intent)
    result = install(bundle, [target])[0]
    assert (
        result.status
        is {
            "committed": OperationStatus.APPLIED,
            "rollback": OperationStatus.FAILED,
            "preparation": OperationStatus.INCOMPLETE,
        }[phase]
    )
    assert result.error is not None
    assert result.error.code is ErrorCode.IO_ERROR
    assert "unlock failed" in result.error.message
    if phase == "committed":
        receipt = read_receipt((result.state_root / "receipts/team.json").read_bytes(), "team")
        assert receipt.content_digest == bundle.content_digest
        assert (target.root / "review/SKILL.md").read_bytes() == b"original"
        assert result.observation.is_current
        assert result.error.errno == errno.EIO
    else:
        assert result.error.errno == errno.EACCES
        assert (
            "receipt blocked" if phase == "rollback" else "intent blocked"
        ) in result.error.message
        assert list(target.root.iterdir()) == []
    assert result.recovery_paths == (
        (result.state_root / "staging", result.state_root / "backup")
        if phase == "preparation"
        else ()
    )


def test_initial_observed_revision_is_revalidated_before_staging(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from flyrail import Bundle, lifecycle
    from flyrail._receipts import Receipt
    from flyrail.inspection import TargetSnapshot

    old = make_bundle(tmp_path / "old")
    new = make_bundle(tmp_path / "new", data=b"new")
    target = Target.directory(tmp_path / "skills")
    install(old, [target])
    original = transaction.prepare

    def edit_then_prepare(
        selected: ResolvedTarget, bundle: Bundle | None, receipt: Receipt, observed: TargetSnapshot
    ) -> Intent:
        (selected.root / "review/run").write_bytes(b"arrived after observation")
        return original(selected, bundle, receipt, observed)

    monkeypatch.setattr(lifecycle, "prepare", edit_then_prepare)
    result = update(new, [target], replace_modified=True)[0]
    assert result.status is OperationStatus.FAILED
    assert (target.root / "review/run").read_bytes() == b"arrived after observation"
    assert (target.root / "review/SKILL.md").read_bytes() == b"original"
    assert result.recovery_paths == ()


def test_equal_observed_override_updates_receipt_without_rewrite_and_rolls_back(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    old = make_bundle(tmp_path / "old")
    new = make_bundle(tmp_path / "new", data=b"new")
    target = Target.directory(tmp_path / "skills")
    install(old, [target])
    (target.root / "review/SKILL.md").write_bytes(b"new")
    original = transaction.publish_receipt

    def fail(selected: ResolvedTarget, intent: Intent) -> None:
        raise OSError(errno.EIO, "before receipt")

    monkeypatch.setattr(transaction, "publish_receipt", fail)
    before = snapshot(target.root)
    assert update(new, [target], replace_modified=True)[0].status is OperationStatus.FAILED
    assert snapshot(target.root) == before
    monkeypatch.setattr(transaction, "publish_receipt", original)
    assert update(new, [target], replace_modified=True)[0].observation.is_current
    assert snapshot(target.root) == before


def test_unsupported_rename_and_target_io_failures_are_per_target(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    bundle = make_bundle(tmp_path / "source")
    blocked = Target.directory(tmp_path / "blocked")
    good = Target.directory(tmp_path / "good")
    from flyrail._filesystem import rename_exclusive

    original = rename_exclusive

    def unsupported(source: Path, destination: Path) -> None:
        if destination == blocked.root.with_name(".blocked.flyrail") / "transaction.json":
            raise OSError(errno.ENOTSUP, "filesystem lacks exclusive rename", destination)
        original(source, destination)

    monkeypatch.setattr(transaction, "rename_exclusive", unsupported)
    results = install(bundle, [blocked, good])
    assert results[0].status is OperationStatus.INCOMPLETE
    assert results[0].error is not None and results[0].error.code is ErrorCode.UNSUPPORTED
    assert list(blocked.root.iterdir()) == []
    assert results[1].observation.is_current


def test_cross_volume_state_is_explicitly_unsupported(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from types import SimpleNamespace

    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    state = target.root.with_name(".skills.flyrail")
    original = Path.stat

    def separate_device(
        path: Path, *, follow_symlinks: bool = True
    ) -> os.stat_result | SimpleNamespace:
        metadata = original(path, follow_symlinks=follow_symlinks)
        if follow_symlinks and path == state / "staging":
            return SimpleNamespace(st_dev=metadata.st_dev + 1)
        return metadata

    monkeypatch.setattr(Path, "stat", separate_device)
    result = install(bundle, [target])[0]
    assert result.status is OperationStatus.FAILED
    assert result.error is not None and result.error.code is ErrorCode.UNSUPPORTED
    assert list(target.root.iterdir()) == []


def test_postcommit_io_cleanup_failure_is_applied_with_pending_paths(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    install(bundle, [target])
    original = transaction._delete_entry

    def fail(path: Path, directory: bool) -> None:
        raise PermissionError(errno.EACCES, "cleanup blocked", path)

    monkeypatch.setattr(transaction, "_delete_entry", fail)
    result = uninstall("team", [target])[0]
    assert result.status is OperationStatus.APPLIED
    assert result.error is not None and result.error.code is ErrorCode.IO_ERROR
    assert result.recovery_paths
    assert list(target.root.iterdir()) == []
    monkeypatch.setattr(transaction, "_delete_entry", original)
    assert uninstall("team", [target])[0].status is OperationStatus.UNCHANGED
    assert list((result.state_root / "backup").iterdir()) == []


def test_late_destination_collision_preserves_both_revisions(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from flyrail._filesystem import rename_exclusive

    old = make_bundle(tmp_path / "old")
    new = make_bundle(tmp_path / "new", data=b"new")
    target = Target.directory(tmp_path / "skills")
    install(old, [target])

    def collide(source: Path, destination: Path) -> None:
        if destination == target.root / "review":
            destination.mkdir()
            (destination / "SKILL.md").write_bytes(b"concurrent writer")
        rename_exclusive(source, destination)

    monkeypatch.setattr(transaction, "rename_exclusive", collide)
    result = update(new, [target])[0]
    assert result.status is OperationStatus.INCOMPLETE
    assert (target.root / "review/SKILL.md").read_bytes() == b"concurrent writer"
    backup = next((result.state_root / "backup").iterdir())
    stage = next((result.state_root / "staging").iterdir())
    assert (backup / "review/SKILL.md").read_bytes() == b"original"
    assert (stage / "skills/review/SKILL.md").read_bytes() == b"new"


def test_lock_permission_failure_preserves_independent_success(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    original = Path.open
    lock = target.root.with_name(".skills.flyrail") / "lock"

    def denied(path: Path, mode: str = "r") -> object:
        if path == lock:
            raise PermissionError(errno.EACCES, "permission denied", path)
        return original(path, mode)

    monkeypatch.setattr(Path, "open", denied)
    results = install(bundle, [target, Target.directory(tmp_path / "good")])
    assert results[0].status is OperationStatus.FAILED
    assert results[0].error is not None and results[0].error.errno == errno.EACCES
    assert results[1].observation.is_current


def test_pretty_staged_receipt_can_publish(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    import json

    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    original = transaction.publish_receipt

    def pretty(selected: ResolvedTarget, intent: Intent) -> None:
        path = selected.state_root / "staging" / intent.transaction_id / "receipt.json"
        receipt = json.loads(path.read_bytes())
        receipt["entries"].reverse()
        path.write_text(json.dumps(receipt, indent=2))
        original(selected, intent)

    monkeypatch.setattr(transaction, "publish_receipt", pretty)
    assert install(bundle, [target])[0].observation.is_current
