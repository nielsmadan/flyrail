import errno
import os
from pathlib import Path

from flyrail._filesystem import rename_exclusive
from flyrail._intent import Change, Intent, intent_bytes, read_intent, select
from flyrail._inventory import InventoryEntry, inventory
from flyrail._observation import ObservationFailure, Observer
from flyrail._receipts import Receipt, read_receipt, receipt_bytes, reconcile_receipts
from flyrail.bundle import Bundle
from flyrail.inspection import ResolvedTarget, TargetSnapshot, _state
from flyrail.observations import ErrorCode


def tree(root: Path, name: str, expected: tuple[InventoryEntry, ...]) -> tuple[InventoryEntry, ...]:
    observer = Observer()
    observer.managed_directory(root)
    matches = observer.directory(root, matching=name)
    if matches and matches[0].name != name:
        raise ObservationFailure(ErrorCode.UNSAFE_PATH, "skill spelling changed", root / name)
    entries = inventory(
        observer.tree(root, name, frozenset(e.path for e in expected if e.executable))
    )
    observer.finish()
    return entries


def require_tree(
    root: Path, name: str, expected: tuple[InventoryEntry, ...], *, subset: bool = False
) -> None:
    actual = tree(root, name, expected)
    valid = set(actual) <= set(expected) if subset else actual == expected
    if not valid:
        raise ObservationFailure(
            ErrorCode.RECOVERY_NEEDED, "content differs from transaction inventory", root / name
        )


def _delete_entry(path: Path, directory: bool) -> None:
    if directory:
        path.rmdir()
    else:
        path.unlink()


def delete_tree(root: Path, name: str, expected: tuple[InventoryEntry, ...]) -> None:
    actual = tree(root, name, expected)
    if not set(actual) <= set(expected):
        raise ObservationFailure(
            ErrorCode.RECOVERY_NEEDED, "content differs from transaction inventory", root / name
        )
    for entry in reversed(actual):
        observer = Observer()
        path = root / entry.path
        for parent in reversed(path.parents):
            observer.metadata(parent)
        observer.managed_directory(root)
        parent = root
        for part in entry.path.split("/"):
            matches = observer.directory(parent, matching=part)
            if matches and matches[0].name != part:
                raise ObservationFailure(
                    ErrorCode.UNSAFE_PATH, "cleanup path spelling changed", path
                )
            parent /= part
        current = inventory(
            observer.tree(
                root, entry.path, frozenset({entry.path}) if entry.executable else frozenset()
            )
        )
        if current != (entry,):
            raise ObservationFailure(
                ErrorCode.RECOVERY_NEEDED, "content differs from transaction inventory", path
            )
        observer.finish()
        _delete_entry(path, entry.is_directory)


def receipt_matches(target: ResolvedTarget, expected: Receipt) -> bool:
    observer = Observer()
    observer.managed_directory(target.state_root)
    observer.managed_directory(target.state_root / "receipts")
    path = target.state_root / "receipts" / f"{expected.bundle_id}.json"
    try:
        actual = (
            None
            if observer.metadata(path) is None
            else read_receipt(observer.file(path), expected.bundle_id)
        )
    except ValueError as error:
        raise ObservationFailure(ErrorCode.INVALID_STATE, str(error), path) from error
    observer.finish()
    return actual == expected


def _receipt(target: ResolvedTarget, identifier: str) -> Receipt | None:
    observer = Observer()
    receipts, _ = _state(observer, target.state_root)
    observer.finish()
    return next((item for item in receipts if item.bundle_id == identifier), None)


def _layout(target: ResolvedTarget, intent: Intent) -> tuple[Path, Path]:
    state = target.state_root
    return state / "staging" / intent.transaction_id, state / "backup" / intent.transaction_id


def _known_children(path: Path, allowed: set[str]) -> None:
    observer = Observer()
    observer.managed_directory(path)
    for child in observer.directory(path):
        if child.name not in allowed:
            raise ObservationFailure(ErrorCode.RECOVERY_NEEDED, "unknown recovery data", child)
    observer.finish()


def _check_layout(target: ResolvedTarget, intent: Intent) -> tuple[Path, Path]:
    stage, backup = _layout(target, intent)
    for path in (stage.parent, backup.parent):
        _known_children(path, {intent.transaction_id})
    _known_children(stage, {"skills", "receipt.json"})
    _known_children(stage / "skills", {c.name for c in intent.changes if c.new != c.old})
    _known_children(backup, {c.name for c in intent.changes if c.new != c.old})
    return stage, backup


def _verify_receipt(path: Path, expected: Receipt, code: ErrorCode) -> None:
    observer = Observer()
    try:
        actual = read_receipt(observer.file(path), expected.bundle_id)
    except (ValueError, TypeError) as error:
        raise ObservationFailure(code, str(error), path) from error
    if actual != expected:
        raise ObservationFailure(code, "staged receipt changed", path)
    observer.finish()


def _unlink_receipt(path: Path, expected: Receipt) -> None:
    if Observer().metadata(path) is None:
        return
    _verify_receipt(path, expected, ErrorCode.RECOVERY_NEEDED)
    path.unlink()


def _verify_intent(target: ResolvedTarget, intent: Intent) -> None:
    path = target.state_root / "transaction.json"
    observer = Observer()
    try:
        current = read_intent(observer.file(path))
    except (ValueError, TypeError) as error:
        raise ObservationFailure(ErrorCode.RECOVERY_NEEDED, str(error), path) from error
    if current != intent:
        raise ObservationFailure(ErrorCode.RECOVERY_NEEDED, "transaction changed", path)
    observer.finish()


def cleanup(target: ResolvedTarget, intent: Intent) -> None:
    _verify_intent(target, intent)
    stage, backup = _check_layout(target, intent)
    for change in intent.changes:
        delete_tree(backup, change.name, change.old)
        delete_tree(stage / "skills", change.name, change.new)
    _unlink_receipt(stage / "receipt.json", intent.new)
    for path in (stage / "skills", stage, backup):
        if Observer().metadata(path) is not None:
            path.rmdir()
    _verify_intent(target, intent)
    (target.state_root / "transaction.json").unlink()


def rollback(target: ResolvedTarget, intent: Intent) -> None:
    stage, backup = _check_layout(target, intent)
    for change in reversed(intent.changes):
        if change.old == change.new:
            require_tree(target.root, change.name, change.old)
            continue
        saved = tree(backup, change.name, change.old)
        staged = tree(stage / "skills", change.name, change.new)
        actual = tree(
            target.root, change.name, change.new if saved or not change.old else change.old
        )
        if saved:
            require_tree(backup, change.name, change.old)
            if actual:
                if actual != change.new or staged:
                    raise ObservationFailure(
                        ErrorCode.RECOVERY_NEEDED,
                        "ambiguous rollback destination",
                        target.root / change.name,
                    )
                rename_exclusive(target.root / change.name, stage / "skills" / change.name)
            rename_exclusive(backup / change.name, target.root / change.name)
        elif actual == change.old:
            require_tree(stage / "skills", change.name, change.new, subset=True)
        elif not change.old and actual == change.new and not staged:
            rename_exclusive(target.root / change.name, stage / "skills" / change.name)
        else:
            raise ObservationFailure(
                ErrorCode.RECOVERY_NEEDED,
                "cannot restore complete previous content",
                target.root / change.name,
            )
    cleanup(target, intent)


def recover(target: ResolvedTarget) -> None:
    observer = Observer()
    receipts, pending = _state(observer, target.state_root)
    observer.finish()
    if not pending:
        return
    path = target.state_root / "transaction.json"
    observer = Observer()
    if observer.metadata(path) is None:
        raise ObservationFailure(
            ErrorCode.RECOVERY_NEEDED, "unrecognized preparation data retained", pending[0]
        )
    try:
        intent = read_intent(observer.file(path))
    except (ValueError, TypeError) as error:
        raise ObservationFailure(ErrorCode.RECOVERY_NEEDED, str(error), path) from error
    observer.finish()
    current = next((item for item in receipts if item.bundle_id == intent.new.bundle_id), None)
    if current not in (intent.previous, intent.new):
        raise ObservationFailure(
            ErrorCode.RECOVERY_NEEDED, "receipt disagrees with transaction", path
        )
    try:
        reconcile_receipts(
            (*(r for r in receipts if r.bundle_id != intent.new.bundle_id), intent.new)
        )
        if intent.previous is not None:
            reconcile_receipts(
                (*(r for r in receipts if r.bundle_id != intent.new.bundle_id), intent.previous)
            )
    except ValueError as error:
        raise ObservationFailure(ErrorCode.RECOVERY_NEEDED, str(error), path) from error
    if current == intent.new:
        cleanup(target, intent)
    else:
        rollback(target, intent)


def prepare(
    target: ResolvedTarget, bundle: Bundle | None, new: Receipt, snapshot: TargetSnapshot
) -> Intent:
    previous = next((r for r in snapshot.receipts if r.bundle_id == new.bundle_id), None)
    old_entries = () if previous is None else previous.entries
    names = set(new.skills) | (set() if previous is None else set(previous.skills))
    changes: list[Change] = []
    for name in sorted(names):
        old = select(snapshot.entries, name)
        require_tree(target.root, name, old)
        desired = select(new.entries, name)
        if old != desired or select(old_entries, name) != desired:
            changes.append(Change(name, old, desired))
    intent = Intent(previous, new, tuple(changes))
    read_intent(intent_bytes(intent))
    target.root.mkdir(parents=True, exist_ok=True)
    state = target.state_root
    for path in (state / "receipts", state / "staging", state / "backup"):
        path.mkdir(mode=0o700, exist_ok=True)
    device = target.root.stat().st_dev
    for path in (state, state / "receipts", state / "staging", state / "backup"):
        if path.stat().st_dev != device:
            raise OSError(errno.ENOTSUP, "target and management state must share a volume", path)
    stage, backup = _layout(target, intent)
    stage.mkdir(mode=0o700)
    (stage / "skills").mkdir(mode=0o700)
    backup.mkdir(mode=0o700)
    changed = {c.name for c in changes if c.old != c.new}
    if bundle is not None:
        for entry in bundle.entries:
            if entry.path.split("/", 1)[0] not in changed:
                continue
            path = stage / "skills" / entry.path
            if entry.data is None:
                path.mkdir()
            else:
                with path.open("xb") as stream:
                    stream.write(entry.data)
                if os.name != "nt":
                    path.chmod(0o755 if entry.executable else 0o644)
    (stage / "receipt.json").write_bytes(receipt_bytes(new))
    for change in changes:
        require_tree(target.root, change.name, change.old)
        if change.old != change.new:
            require_tree(stage / "skills", change.name, change.new)
    if _receipt(target, new.bundle_id) != previous:
        raise ObservationFailure(
            ErrorCode.CONCURRENT_CHANGE, "receipt changed during preparation", state
        )
    (stage / "intent.json").write_bytes(intent_bytes(intent))
    rename_exclusive(stage / "intent.json", state / "transaction.json")
    return intent


def publish_receipt(target: ResolvedTarget, intent: Intent) -> None:
    if _receipt(target, intent.new.bundle_id) != intent.previous:
        raise ObservationFailure(
            ErrorCode.CONCURRENT_CHANGE, "receipt changed before commit", target.state_root
        )
    stage, _ = _layout(target, intent)
    _verify_receipt(stage / "receipt.json", intent.new, ErrorCode.CONCURRENT_CHANGE)
    os.replace(
        stage / "receipt.json", target.state_root / "receipts" / f"{intent.new.bundle_id}.json"
    )


def publish(target: ResolvedTarget, intent: Intent) -> None:
    stage, backup = _layout(target, intent)
    for change in intent.changes:
        require_tree(target.root, change.name, change.old)
        if change.old == change.new:
            continue
        if change.old:
            rename_exclusive(target.root / change.name, backup / change.name)
            require_tree(backup, change.name, change.old)
        if change.new:
            require_tree(stage / "skills", change.name, change.new)
            rename_exclusive(stage / "skills" / change.name, target.root / change.name)
    for change in intent.changes:
        require_tree(target.root, change.name, change.new)
    for name in intent.new.skills:
        require_tree(target.root, name, select(intent.new.entries, name))
    publish_receipt(target, intent)
