import os
from dataclasses import dataclass, replace
from pathlib import Path

from flyrail._codec import register
from flyrail._filesystem import rename_exclusive
from flyrail._observation import ObservationFailure, Observer
from flyrail._resource_io import (
    ancestors,
    cleanup_revision,
    destroy,
    failure,
    move,
    observe,
    private_revision,
    protect,
    require,
    write_revision,
)
from flyrail._resource_models import Ancestor, Receipt, ResourceRef, Revision, sequence
from flyrail._security import ensure_private, principals, replace_file, set_security
from flyrail._state import (
    delete_record,
    hierarchy_conflicts,
    read_record,
    resource_key,
    write_record,
)
from flyrail.authority import transaction_path
from flyrail.configuration import ResourcePlan
from flyrail.observations import ErrorCode
from flyrail.stores import StateStore


@dataclass(frozen=True, slots=True)
class Journal:
    resource: ResourceRef
    previous: Receipt | None
    receipt: Receipt
    before: Revision
    staged: Revision
    published: Revision
    backup: Revision
    ancestors: tuple[Ancestor, ...]
    schema_version: int = 2

    def __post_init__(self) -> None:
        if type(self.resource) is not ResourceRef or type(self.receipt) is not Receipt:
            raise TypeError("journal requires resource and receipt")
        if self.previous is not None and type(self.previous) is not Receipt:
            raise TypeError("journal previous receipt is invalid")
        if self.receipt.resource != self.resource or (
            self.previous is not None and self.previous.resource != self.resource
        ):
            raise ValueError("journal receipt resources disagree")
        sequence((self.before, self.staged, self.published, self.backup), Revision)
        object.__setattr__(
            self,
            "ancestors",
            sequence(self.ancestors, Ancestor),
        )
        if type(self.schema_version) is not int or self.schema_version != 2:
            raise ValueError("journal schema must be 2")
        if (
            self.before.content_digest != self.backup.content_digest
            or self.staged.content_digest != self.published.content_digest
        ):
            raise ValueError("journal private revisions must retain complete content")


register(Journal)


def transaction_directory(journal: Journal) -> Path:
    return transaction_path(journal.resource.destination, journal.receipt.transaction_id)


def paths(directory: Path) -> tuple[Path, Path]:
    return directory / "staging" / "resource", directory / "backup" / "resource"


def _journal_key(ref: ResourceRef) -> str:
    return resource_key(ref.destination, "journal")


def _receipt_key(ref: ResourceRef) -> str:
    return resource_key(ref.destination, "receipt")


def _layout(directory: Path) -> None:
    observer = Observer()
    observer.managed_directory(directory)
    for child in observer.directory(directory):
        if child.name not in {"staging", "backup"}:
            raise failure(ErrorCode.RECOVERY_NEEDED, "unknown transaction data", child)
        if not {path.name for path in observer.directory(child)} <= {"resource"}:
            raise failure(ErrorCode.RECOVERY_NEEDED, "unknown transaction data", child)
    observer.finish()


def transaction_ancestors(directory: Path) -> tuple[Ancestor, ...]:
    staged, backup = paths(directory)
    return tuple(dict.fromkeys((*ancestors(staged), *ancestors(backup))))


def _transient(path: Path, directory: Path) -> bool:
    return path == directory or directory in path.parents


def _ancestors(store: StateStore, journal: Journal) -> None:
    ref = journal.resource
    directory = transaction_directory(journal)
    current = transaction_ancestors(directory)
    retained = tuple(
        item
        for item in journal.ancestors
        if item in current or not _transient(item.path, directory)
    )
    if current != retained:
        raise failure(ErrorCode.CONCURRENT_CHANGE, "resource ancestor changed", ref.destination)
    for path in (directory, directory / "staging", directory / "backup"):
        if path.exists():
            ensure_private(path)
    conflicts = hierarchy_conflicts(store, ref.authority, whole_tree=ref.kind != "document")
    if conflicts:
        raise failure(ErrorCode.CONFLICT, "resource hierarchy reservation changed", conflicts[0])


def _receipt(store: StateStore, journal: Journal) -> Receipt | None:
    return read_record(store, _receipt_key(journal.resource), Receipt)


def _journal(store: StateStore, journal: Journal) -> None:
    if read_record(store, _journal_key(journal.resource), Journal) != journal:
        raise failure(
            ErrorCode.RECOVERY_NEEDED,
            "transaction record changed",
            transaction_directory(journal),
        )


def _remove_directories(
    directory: Path, *, ignored: tuple[type[OSError], ...] = (FileNotFoundError,)
) -> None:
    for path in (directory / "staging", directory / "backup", directory):
        try:
            path.rmdir()
        except ignored:
            continue


def _discard(directory: Path, revisions: tuple[Revision, ...]) -> None:
    staged = paths(directory)[0]
    try:
        current = observe(staged)
        if current.nodes and current in revisions:
            destroy(staged, current)
    except (OSError, ObservationFailure, ValueError):
        return
    _remove_directories(directory, ignored=(OSError,))


def pending_recovery(store: StateStore, ref: ResourceRef) -> tuple[Path, ...]:
    journal = read_record(store, _journal_key(ref), Journal)
    return () if journal is None else (transaction_directory(journal),)


def prepare(store: StateStore, plan: ResourcePlan) -> Journal:
    ref = plan.resource
    directory = transaction_path(ref.destination, plan.receipt.transaction_id)
    if ancestors(directory / "staging") != plan.ancestors:
        raise failure(ErrorCode.CONCURRENT_CHANGE, "resource ancestor changed", ref.destination)
    require(ref.destination, plan.before)
    if plan.require_existing and not plan.before.nodes:
        raise failure(
            ErrorCode.CONCURRENT_CHANGE, "required existing resource disappeared", ref.destination
        )
    if read_record(store, _receipt_key(ref), Receipt) != plan.previous:
        raise failure(
            ErrorCode.CONCURRENT_CHANGE, "receipt changed during preparation", ref.destination
        )
    retained = read_record(store, _journal_key(ref), Journal)
    if retained is not None:
        raise failure(
            ErrorCode.RECOVERY_NEEDED,
            "transaction record retained",
            transaction_directory(retained),
        )
    try:
        directory.mkdir(mode=0o700)
    except FileExistsError as error:
        raise failure(
            ErrorCode.RECOVERY_NEEDED, "unrecognized transaction data retained", directory
        ) from error
    written: Revision | None = None
    try:
        storage_ancestors = _storage(plan, directory)
        if plan.after != plan.before:
            written = write_revision(paths(directory)[0], plan.after)
        journal = _journal_for(
            store, plan, directory, storage_ancestors, plan.before if written is None else written
        )
    except Exception:
        _discard(directory, () if written is None else (written,))
        raise
    try:
        write_record(store, _journal_key(ref), journal, expected=None)
    except Exception:
        _discard(directory, (journal.staged, journal.published))
        raise
    return journal


def _storage(plan: ResourcePlan, directory: Path) -> tuple[Ancestor, ...]:
    staged_path, backup_path = paths(directory)
    ensure_private(directory)
    storage = []
    for path in (staged_path.parent, backup_path.parent):
        path.mkdir(mode=0o700)
        expected = observe(path)
        ensure_private(path)
        storage.append((path, expected))
    storage_ancestors = transaction_ancestors(directory)
    for path, expected in storage:
        require(path, expected)
    created = (directory, staged_path.parent, backup_path.parent)
    if tuple(item for item in storage_ancestors if item.path not in created) != plan.ancestors:
        raise failure(
            ErrorCode.CONCURRENT_CHANGE, "resource ancestor changed", plan.resource.destination
        )
    return storage_ancestors


def _journal_for(
    store: StateStore,
    plan: ResourcePlan,
    directory: Path,
    storage_ancestors: tuple[Ancestor, ...],
    staged: Revision,
) -> Journal:
    ref = plan.resource
    staged_path = paths(directory)[0]
    published = staged
    backup = plan.before
    if os.name == "nt" and plan.before != plan.after and plan.before.nodes:
        template = (
            staged.nodes[0].security if staged.nodes else observe(directory).nodes[0].security
        )
        if any(principals(node.security) != principals(template) for node in plan.before.nodes):
            import errno

            raise OSError(
                errno.ENOTSUP, "resource ownership metadata cannot be preserved", ref.destination
            )
        if ref.kind != "document":
            for node in plan.before.nodes:
                original = ref.destination / node.path if node.path else ref.destination
                ensure_private(original)
                counterpart = next((item for item in staged.nodes if item.path == node.path), None)
                if counterpart is not None:
                    selected = staged_path / node.path if node.path else staged_path
                    set_security(selected, node.security)
            staged = observe(staged_path)
            published = staged
        else:
            backup = private_revision(plan.before, template)
        if ref.kind == "document" and plan.before.data is not None and staged.data is not None:
            published = Revision(
                (replace(staged.nodes[0], security=plan.before.nodes[0].security),)
            )
    journal = Journal(
        ref,
        plan.previous,
        replace(
            plan.receipt,
            created_directories=tuple(
                node
                for node in published.nodes
                if any(marker.path == node.path for marker in plan.receipt.created_directories)
            ),
        ),
        plan.before,
        staged,
        published,
        backup,
        storage_ancestors,
    )
    _ancestors(store, journal)
    require(ref.destination, plan.before)
    return journal


def publish_receipt(store: StateStore, journal: Journal) -> None:
    _journal(store, journal)
    _layout(transaction_directory(journal))
    _ancestors(store, journal)
    staged, backup = paths(transaction_directory(journal))
    require(staged, Revision())
    require(journal.resource.destination, journal.published)
    require(backup, journal.backup if journal.before != journal.staged else Revision())
    if _receipt(store, journal) != journal.previous:
        raise failure(
            ErrorCode.CONCURRENT_CHANGE,
            "receipt changed before commit",
            journal.resource.destination,
        )
    write_record(store, _receipt_key(journal.resource), journal.receipt, expected=journal.previous)


def publish(store: StateStore, journal: Journal) -> None:
    ref = journal.resource
    staged, backup = paths(transaction_directory(journal))
    _journal(store, journal)
    _ancestors(store, journal)
    require(ref.destination, journal.before)
    if journal.before != journal.staged:
        require(staged, journal.staged)
        if journal.backup != journal.before:
            protect(ref.destination, journal.before, journal.backup.nodes[0].security)
            require(ref.destination, journal.backup)
            _ancestors(store, journal)
            require(ref.destination, journal.backup)
        if os.name == "nt" and journal.before.data is not None and journal.staged.data is not None:
            replace_file(ref.destination, staged, backup)
            require(backup, journal.backup)
            actual = observe(ref.destination)
            if actual not in (journal.staged, journal.published):
                raise failure(
                    ErrorCode.CONCURRENT_CHANGE,
                    "published resource revision changed",
                    ref.destination,
                )
            set_security(ref.destination, journal.published.nodes[0].security)
        else:
            if journal.before.nodes:
                move(ref.destination, backup, journal.backup)
                require(backup, journal.backup)
            if journal.staged.nodes:
                require(staged, journal.staged)
                rename_exclusive(staged, ref.destination)
        require(ref.destination, journal.published)
    _ancestors(store, journal)
    require(ref.destination, journal.published)
    publish_receipt(store, journal)


def cleanup(store: StateStore, journal: Journal) -> None:
    directory = transaction_directory(journal)
    _journal(store, journal)
    _layout(directory)
    _ancestors(store, journal)
    staged, backup = paths(directory)
    for path, revisions in (
        (staged, (journal.staged, journal.published)),
        (backup, (journal.backup, journal.before)),
    ):
        current = observe(path)
        if current.nodes:
            if not any(cleanup_revision(current, revision) for revision in revisions):
                raise failure(ErrorCode.RECOVERY_NEEDED, "unexpected cleanup revision", path)
            destroy(path, current)
    _remove_directories(directory)
    delete_record(store, _journal_key(journal.resource), expected=journal)


def rollback(store: StateStore, journal: Journal) -> None:
    ref = journal.resource
    directory = transaction_directory(journal)
    stage_path, backup_path = paths(directory)
    _layout(directory)
    _ancestors(store, journal)
    actual, staged, backup = observe(ref.destination), observe(stage_path), observe(backup_path)
    if actual == journal.before and not backup.nodes:
        cleanup(store, journal)
        return
    if staged.nodes and staged not in (journal.staged, journal.published):
        raise failure(ErrorCode.RECOVERY_NEEDED, "unexpected staged revision", stage_path)
    if backup.nodes and backup not in (journal.before, journal.backup):
        raise failure(ErrorCode.RECOVERY_NEEDED, "unexpected backup revision", backup_path)
    if journal.before == journal.staged:
        require(ref.destination, journal.before)
    elif backup.nodes:
        if actual.nodes:
            if actual not in (journal.published, journal.staged) or staged.nodes:
                raise failure(
                    ErrorCode.RECOVERY_NEEDED, "unrecognized published revision", ref.destination
                )
            if actual != journal.staged:
                protect(ref.destination, actual, journal.staged.nodes[0].security)
                require(ref.destination, journal.staged)
                actual = journal.staged
            move(ref.destination, stage_path, actual)
        move(backup_path, ref.destination, backup)
        if backup != journal.before:
            for node in journal.before.nodes:
                set_security(
                    ref.destination / node.path if node.path else ref.destination, node.security
                )
            require(ref.destination, journal.before)
    elif actual == journal.before:
        pass
    elif actual == journal.backup:
        for node in journal.before.nodes:
            set_security(
                ref.destination / node.path if node.path else ref.destination, node.security
            )
        require(ref.destination, journal.before)
    elif not journal.before.nodes and actual == journal.published and not staged.nodes:
        move(ref.destination, stage_path, actual)
    else:
        raise failure(
            ErrorCode.RECOVERY_NEEDED, "cannot restore complete previous revision", ref.destination
        )
    cleanup(store, journal)


def recover(store: StateStore, ref: ResourceRef) -> None:
    journal = read_record(store, _journal_key(ref), Journal)
    if journal is None:
        return
    if journal.resource != ref:
        raise failure(
            ErrorCode.RECOVERY_NEEDED,
            "transaction record disagrees",
            transaction_directory(journal),
        )
    if journal.receipt == journal.previous:
        raise failure(
            ErrorCode.RECOVERY_NEEDED,
            "transaction record repeats the previous receipt",
            transaction_directory(journal),
        )
    current = _receipt(store, journal)
    if current == journal.receipt:
        cleanup(store, journal)
    elif current == journal.previous:
        rollback(store, journal)
    else:
        raise failure(
            ErrorCode.RECOVERY_NEEDED, "receipt disagrees with transaction", ref.destination
        )
