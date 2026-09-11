import json
import os
import stat
import subprocess
import sys
import time
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

import pytest
from test_inspection import make_bundle, snapshot

from flyrail import ErrorCode, OperationStatus, Target, inspect, install, uninstall, update
from flyrail._receipts import bundle_receipt, receipt_bytes

_CHILD = r"""
import sys, time
from pathlib import Path
from flyrail import Bundle, Target, update, uninstall
from flyrail import _transaction as tx
from flyrail._filesystem import target_lock
source, root, signal, boundary = map(Path, sys.argv[1:])
root = root.resolve()
boundary = str(boundary)
removal = boundary.startswith("uninstall-")
boundary = boundary.removeprefix("uninstall-")
def pause():
    signal.write_text("ready")
    while True:
        time.sleep(0.02)
rename = tx.rename_exclusive
def move(a, b):
    if boundary == "preparation" and a.name == "intent.json":
        pause()
    rename(a, b)
    if boundary == "intent" and b.name == "transaction.json":
        pause()
    if boundary == "backup" and a.parent == root:
        pause()
    if boundary == "published" and b == root / "review":
        pause()
    if boundary == "return" and a.parent == root:
        pause()
    if boundary == "restore" and b.parent == root:
        pause()
tx.rename_exclusive = move
publish = tx.publish_receipt
def receipt(target, intent):
    publish(target, intent)
    if boundary == "commit":
        pause()
tx.publish_receipt = receipt
delete = tx._delete_entry
def remove(path, directory):
    delete(path, directory)
    if boundary in {"cleanup", "rollback-cleanup"}:
        pause()
tx._delete_entry = remove
unlink = Path.unlink
def unlink_path(path, *args, **kwargs):
    if boundary == "final-cleanup" and path.name == "transaction.json":
        pause()
    return unlink(path, *args, **kwargs)
Path.unlink = unlink_path
if boundary == "lock":
    with target_lock(root.with_name("." + root.name + ".flyrail"), 0):
        pause()
elif boundary in {"return", "restore", "rollback-cleanup", "final-cleanup"}:
    uninstall("absent", [Target.directory(root)])
elif removal:
    uninstall("team", [Target.directory(root)])
else:
    update(Bundle.from_directory(source), [Target.directory(root)])
raise RuntimeError("boundary was never reached")
"""


@contextmanager
def child_at(
    source: Path, target: Target, signal: Path, boundary: str
) -> Iterator[subprocess.Popen[bytes]]:
    environment = os.environ.copy()
    environment["PYTHONPATH"] = str(Path(__file__).resolve().parents[1] / "src")
    with (signal.with_suffix(".log")).open("wb") as output:
        child = subprocess.Popen(  # noqa: S603
            [sys.executable, "-c", _CHILD, str(source), str(target.root), str(signal), boundary],
            stdout=output,
            stderr=subprocess.STDOUT,
            env=environment,
        )
        try:
            deadline = time.monotonic() + 10
            while not signal.exists():
                if child.poll() is not None or time.monotonic() >= deadline:
                    raise AssertionError(signal.with_suffix(".log").read_text())
                time.sleep(0.01)
            yield child
        finally:
            child.kill()
            child.wait(timeout=10)


@pytest.mark.parametrize("boundary", ["intent", "backup", "published", "commit", "cleanup"])
def test_killed_transaction_recovers_from_receipt_boundary(tmp_path: Path, boundary: str) -> None:
    old = make_bundle(tmp_path / "old", names=("review", "zebra"))
    new = make_bundle(tmp_path / "new", names=("review", "zebra"), data=b"replacement")
    target = Target.directory(tmp_path / "skills")
    assert install(old, [target])[0].observation.is_current
    with child_at(tmp_path / "new", target, tmp_path / "ready", boundary):
        busy = uninstall("absent", [target])[0]
        assert busy.error is not None and busy.error.code is ErrorCode.BUSY
        observed = inspect(new, [target])[0].observation
        assert observed.recovery_paths
    recovered = uninstall("absent", [target])[0]
    assert recovered.status is OperationStatus.UNCHANGED
    expected = new if boundary in {"commit", "cleanup"} else old
    assert inspect(expected, [target])[0].observation.is_current
    assert uninstall("absent", [target])[0].status is OperationStatus.UNCHANGED
    assert list((recovered.state_root / "staging").iterdir()) == []
    assert list((recovered.state_root / "backup").iterdir()) == []


@pytest.mark.parametrize("boundary", ["return", "restore", "rollback-cleanup", "final-cleanup"])
def test_recovery_itself_survives_kill(tmp_path: Path, boundary: str) -> None:
    old = make_bundle(tmp_path / "old")
    make_bundle(tmp_path / "new", data=b"new")
    target = Target.directory(tmp_path / "skills")
    install(old, [target])
    with child_at(tmp_path / "new", target, tmp_path / "first", "published"):
        pass
    with child_at(tmp_path / "new", target, tmp_path / "second", boundary):
        pass
    assert uninstall("absent", [target])[0].status is OperationStatus.UNCHANGED
    assert inspect(old, [target])[0].observation.is_current
    assert uninstall("absent", [target])[0].status is OperationStatus.UNCHANGED


def test_killed_first_install_restores_absence(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    with child_at(tmp_path / "source", target, tmp_path / "ready", "published"):
        pass
    assert uninstall("absent", [target])[0].status is OperationStatus.UNCHANGED
    assert list(target.root.iterdir()) == []
    assert install(bundle, [target])[0].observation.is_current


def test_orphan_preparation_and_unknown_transaction_are_retained(tmp_path: Path) -> None:
    old = make_bundle(tmp_path / "old")
    make_bundle(tmp_path / "new", data=b"new")
    target = Target.directory(tmp_path / "skills")
    install(old, [target])
    with child_at(tmp_path / "new", target, tmp_path / "ready", "preparation"):
        pass
    state = target.root.with_name(".skills.flyrail")
    before = snapshot(state)
    result = uninstall("team", [target])[0]
    assert result.status is OperationStatus.INCOMPLETE
    assert result.error is not None and result.error.code is ErrorCode.RECOVERY_NEEDED
    assert snapshot(state) == before
    (state / "transaction.json").write_bytes(b"truncated")
    invalid = uninstall("team", [target])[0]
    assert invalid.status is OperationStatus.INCOMPLETE
    assert (state / "transaction.json").read_bytes() == b"truncated"


@pytest.mark.skipif(os.name == "nt", reason="POSIX mode barrier")
@pytest.mark.parametrize("boundary", ["preparation", "backup"])
def test_killed_transaction_retains_private_staging_and_backups(
    tmp_path: Path, boundary: str
) -> None:
    old = make_bundle(tmp_path / "old")
    make_bundle(tmp_path / "new", data=b"private replacement")
    public = tmp_path / "public"
    public.mkdir(mode=0o755)
    public.chmod(0o755)
    target = Target.directory(public / "skills")
    target.root.mkdir(mode=0o700)
    assert install(old, [target])[0].status is OperationStatus.APPLIED
    state = public / ".skills.flyrail"
    for path in (state, state / "receipts", state / "staging", state / "backup"):
        path.chmod(0o755)

    def assert_private() -> None:
        assert stat.S_IMODE(public.stat().st_mode) == 0o755
        assert stat.S_IMODE(state.stat().st_mode) == 0o755
        assert stat.S_IMODE(target.root.stat().st_mode) == 0o700
        (stage,) = (state / "staging").iterdir()
        (backup,) = (state / "backup").iterdir()
        assert stat.S_IMODE(stage.stat().st_mode) == 0o700
        assert stat.S_IMODE(backup.stat().st_mode) == 0o700
        assert (stage / "skills/review/SKILL.md").read_bytes() == b"private replacement"
        if boundary == "backup":
            assert (backup / "review/SKILL.md").read_bytes() == b"original"

    previous_umask = os.umask(0o022)
    try:
        with child_at(tmp_path / "new", target, tmp_path / "ready", boundary):
            assert_private()
    finally:
        os.umask(previous_umask)
    assert_private()
    before_target, before_state = snapshot(target.root), snapshot(state)
    result = uninstall("absent", [target])[0]
    if boundary == "preparation":
        assert result.status is OperationStatus.INCOMPLETE
        assert result.error is not None and result.error.code is ErrorCode.RECOVERY_NEEDED
        assert snapshot(target.root) == before_target
        assert snapshot(state) == before_state
        assert_private()
    else:
        assert result.status is OperationStatus.UNCHANGED
        assert inspect(old, [target])[0].observation.is_current


def test_lock_is_stable_bounded_and_released_when_process_dies(tmp_path: Path) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    state = target.root.with_name(".skills.flyrail")
    with child_at(tmp_path / "source", target, tmp_path / "ready", "lock"):
        inode = (state / "lock").stat().st_ino
        started = time.monotonic()
        result = install(bundle, [target], lock_timeout=0.08)[0]
        elapsed = time.monotonic() - started
        assert result.status is OperationStatus.FAILED
        assert result.error is not None and result.error.code is ErrorCode.BUSY
        assert 0.07 <= elapsed < 1
        assert (state / "lock").stat().st_ino == inode
    assert install(bundle, [target])[0].observation.is_current
    assert (state / "lock").stat().st_ino == inode


@pytest.mark.parametrize("damage", ["new-destination", "backup", "unknown", "staged", "receipt"])
def test_uncertain_recovery_preserves_data(tmp_path: Path, damage: str) -> None:
    old = make_bundle(tmp_path / "old")
    make_bundle(tmp_path / "new", data=b"new")
    target = Target.directory(tmp_path / "skills")
    install(old, [target])
    with child_at(tmp_path / "new", target, tmp_path / "ready", "published"):
        pass
    state = target.root.with_name(".skills.flyrail")
    raw = json.loads((state / "transaction.json").read_bytes())
    identifier = raw["new"]["transaction_id"]
    if damage == "new-destination":
        (target.root / "review/run").write_bytes(b"unexpected")
    elif damage == "backup":
        (state / "backup" / identifier / "review/run").unlink()
    elif damage == "unknown":
        (state / "staging/unrecognized").mkdir()
    elif damage == "staged":
        (state / "staging" / identifier / "receipt.json").write_bytes(b"unexpected")
    elif damage == "receipt":
        receipt = state / "receipts/team.json"
        content = json.loads(receipt.read_bytes())
        content["transaction_id"] = "e" * 32
        receipt.write_text(json.dumps(content))
    result = update(old, [target])[0]
    assert result.status is OperationStatus.INCOMPLETE
    assert result.recovery_paths
    assert (state / "transaction.json").is_file()
    if damage == "new-destination":
        assert (target.root / "review/run").read_bytes() == b"unexpected"
    if damage == "staged":
        assert (state / "staging" / identifier / "receipt.json").read_bytes() == b"unexpected"


def test_recovery_preserves_new_skill_claimed_by_foreign_receipt(tmp_path: Path) -> None:
    old = make_bundle(tmp_path / "old")
    make_bundle(tmp_path / "new", names=("aardvark", "review"), data=b"new")
    foreign = make_bundle(
        tmp_path / "foreign", identifier="foreign", names=("aardvark",), data=b"new"
    )
    target = Target.directory(tmp_path / "skills")
    assert install(old, [target])[0].status is OperationStatus.APPLIED
    with child_at(tmp_path / "new", target, tmp_path / "ready", "published"):
        pass
    state = target.root.with_name(".skills.flyrail")
    foreign_receipt = state / "receipts/foreign.json"
    foreign_receipt.write_bytes(receipt_bytes(bundle_receipt(foreign, "e" * 32)))
    observed = inspect(foreign, [target])[0].observation
    assert observed.error is not None and observed.error.code is ErrorCode.RECOVERY_NEEDED
    assert observed.installed is not None and observed.installed.bundle_id == "foreign"
    assert observed.content_matches
    before_target, before_state = snapshot(target.root), snapshot(state)

    result = uninstall("absent", [target])[0]

    assert result.status is OperationStatus.INCOMPLETE
    assert result.error is not None and result.error.code is ErrorCode.RECOVERY_NEEDED
    assert result.recovery_paths
    assert (target.root / "aardvark/SKILL.md").read_bytes() == b"new"
    assert snapshot(target.root) == before_target
    assert snapshot(state) == before_state


@pytest.mark.parametrize("boundary", ["backup", "commit"])
def test_killed_uninstall_recovers_by_tombstone(tmp_path: Path, boundary: str) -> None:
    bundle = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    install(bundle, [target])
    with child_at(tmp_path / "source", target, tmp_path / "ready", "uninstall-" + boundary):
        pass
    result = uninstall("absent", [target])[0]
    assert result.status is OperationStatus.UNCHANGED
    assert uninstall("absent", [target])[0].status is OperationStatus.UNCHANGED
    if boundary == "commit":
        assert list(target.root.iterdir()) == []
        assert inspect(bundle, [target])[0].observation.installed is None
    else:
        assert inspect(bundle, [target])[0].observation.is_current


def test_valid_noncanonical_intent_recovers(tmp_path: Path) -> None:
    old = make_bundle(tmp_path / "old")
    make_bundle(tmp_path / "new", data=b"new")
    target = Target.directory(tmp_path / "skills")
    install(old, [target])
    with child_at(tmp_path / "new", target, tmp_path / "ready", "published"):
        pass
    path = target.root.with_name(".skills.flyrail") / "transaction.json"
    raw = json.loads(path.read_bytes())
    path.write_text(json.dumps(raw, indent=2))
    staged = path.parent / "staging" / raw["new"]["transaction_id"] / "receipt.json"
    receipt = json.loads(staged.read_bytes())
    receipt["entries"].reverse()
    staged.write_text(json.dumps(receipt, indent=2))
    assert uninstall("absent", [target])[0].status is OperationStatus.UNCHANGED
    assert inspect(old, [target])[0].observation.is_current
