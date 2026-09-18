import os
from dataclasses import dataclass, replace
from pathlib import Path

from flyrail._codec import register
from flyrail._filesystem import rename_exclusive
from flyrail._resource_io import (
    ancestors,
    atomic,
    cleanup_revision,
    destroy,
    failure,
    move,
    observe,
    private_revision,
    protect,
    read,
    require,
    validate_state,
    write_revision,
)
from flyrail._resource_models import Ancestor, Receipt, ResourceRef, Revision, sequence
from flyrail._security import ensure_private, principals, replace_file, set_security
from flyrail.authority import hierarchy_conflicts
from flyrail.configuration import ResourcePlan
from flyrail.observations import ErrorCode


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


def paths(ref: ResourceRef) -> tuple[Path, Path, Path]:
    return (
        ref.state_root / "transaction.json",
        ref.state_root / "staging" / "resource",
        ref.state_root / "backup" / "resource",
    )


def _layout(ref: ResourceRef) -> None:
    validate_state(ref)
    for parent in ("staging", "backup"):
        root = ref.state_root / parent
        if root.exists():
            names = {path.name for path in root.iterdir()}
            if not names <= {"resource"}:
                raise failure(ErrorCode.RECOVERY_NEEDED, "unknown transaction data", root)


def transaction_ancestors(ref: ResourceRef) -> tuple[Ancestor, ...]:
    _, staged, backup = paths(ref)
    return tuple(dict.fromkeys((*ancestors(staged), *ancestors(backup))))


def _ancestors(journal: Journal) -> None:
    current = transaction_ancestors(journal.resource)
    if current != journal.ancestors:
        raise failure(
            ErrorCode.CONCURRENT_CHANGE, "resource ancestor changed", journal.resource.destination
        )
    for path in (
        journal.resource.state_root,
        paths(journal.resource)[1].parent,
        paths(journal.resource)[2].parent,
    ):
        ensure_private(path)
    conflicts = hierarchy_conflicts(
        journal.resource.authority, whole_tree=journal.resource.kind != "document"
    )
    if conflicts:
        raise failure(ErrorCode.CONFLICT, "resource hierarchy reservation changed", conflicts[0])


def _receipt(journal: Journal) -> Receipt | None:
    return read(journal.resource.state_root / "receipt.json", Receipt)


def _journal(journal: Journal) -> None:
    if read(paths(journal.resource)[0], Journal) != journal:
        raise failure(
            ErrorCode.RECOVERY_NEEDED, "transaction record changed", paths(journal.resource)[0]
        )


def prepare(plan: ResourcePlan) -> Journal:
    ref = plan.resource
    if ancestors(paths(ref)[0]) != plan.ancestors:
        raise failure(ErrorCode.CONCURRENT_CHANGE, "resource ancestor changed", ref.destination)
    require(ref.state_root, plan.state_revision)
    require(ref.destination, plan.before)
    if plan.require_existing and not plan.before.nodes:
        raise failure(
            ErrorCode.CONCURRENT_CHANGE, "required existing resource disappeared", ref.destination
        )
    if read(ref.state_root / "receipt.json", Receipt) != plan.previous:
        raise failure(
            ErrorCode.CONCURRENT_CHANGE, "receipt changed during preparation", ref.state_root
        )
    journal_path, staged_path, backup_path = paths(ref)
    ensure_private(ref.state_root)
    storage = []
    for path in (staged_path.parent, backup_path.parent):
        previous = next(
            (node for node in plan.state_revision.nodes if node.path == path.name), None
        )
        if previous is None:
            path.mkdir(mode=0o700)
            expected = observe(path)
        else:
            expected = Revision((replace(previous, path=""),))
        require(path, expected)
        ensure_private(path)
        storage.append((path, expected))
    storage_ancestors = transaction_ancestors(ref)
    for path, expected in storage:
        require(path, expected)
    if (
        tuple(
            item
            for item in storage_ancestors
            if item.path not in (staged_path.parent, backup_path.parent)
        )
        != plan.ancestors
    ):
        raise failure(ErrorCode.CONCURRENT_CHANGE, "resource ancestor changed", ref.destination)
    staged = plan.before if plan.after == plan.before else write_revision(staged_path, plan.after)
    published = staged
    backup = plan.before
    if os.name == "nt" and plan.before != plan.after and plan.before.nodes:
        template = (
            staged.nodes[0].security if staged.nodes else observe(ref.state_root).nodes[0].security
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
    _ancestors(journal)
    require(ref.destination, plan.before)
    atomic(journal_path, journal)
    return journal


def publish_receipt(journal: Journal) -> None:
    _journal(journal)
    _layout(journal.resource)
    _ancestors(journal)
    require(paths(journal.resource)[1], Revision())
    require(journal.resource.destination, journal.published)
    require(
        paths(journal.resource)[2],
        journal.backup if journal.before != journal.staged else Revision(),
    )
    for name in ("authority.json.next", "transaction.json.next", "receipt.json.next"):
        path = journal.resource.state_root / name
        if path.exists():
            raise failure(
                ErrorCode.RECOVERY_NEEDED, "unexpected metadata preparation before commit", path
            )
    if _receipt(journal) != journal.previous:
        raise failure(
            ErrorCode.CONCURRENT_CHANGE,
            "receipt changed before commit",
            journal.resource.state_root,
        )
    atomic(journal.resource.state_root / "receipt.json", journal.receipt)


def publish(journal: Journal) -> None:
    ref = journal.resource
    _, staged, backup = paths(ref)
    _journal(journal)
    _ancestors(journal)
    require(ref.destination, journal.before)
    if journal.before != journal.staged:
        require(staged, journal.staged)
        if journal.backup != journal.before:
            protect(ref.destination, journal.before, journal.backup.nodes[0].security)
            require(ref.destination, journal.backup)
            _ancestors(journal)
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
    _ancestors(journal)
    require(ref.destination, journal.published)
    publish_receipt(journal)


def cleanup(journal: Journal) -> None:
    _journal(journal)
    _layout(journal.resource)
    _ancestors(journal)
    _, staged, backup = paths(journal.resource)
    for path, revisions in (
        (staged, (journal.staged, journal.published)),
        (backup, (journal.backup, journal.before)),
    ):
        current = observe(path)
        if current.nodes:
            if not any(cleanup_revision(current, revision) for revision in revisions):
                raise failure(ErrorCode.RECOVERY_NEEDED, "unexpected cleanup revision", path)
            destroy(path, current)
    next_receipt = journal.resource.state_root / "receipt.json.next"
    if next_receipt.exists():
        if read(next_receipt, Receipt) != journal.receipt:
            raise failure(ErrorCode.RECOVERY_NEEDED, "staged receipt changed", next_receipt)
        next_receipt.unlink()
    _journal(journal)
    paths(journal.resource)[0].unlink()


def rollback(journal: Journal) -> None:
    ref = journal.resource
    _, stage_path, backup_path = paths(ref)
    _layout(ref)
    _ancestors(journal)
    actual, staged, backup = observe(ref.destination), observe(stage_path), observe(backup_path)
    if actual == journal.before and not backup.nodes:
        cleanup(journal)
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
    cleanup(journal)


def recover(ref: ResourceRef) -> None:
    pending = validate_state(ref)
    if not pending:
        return
    journal_path = paths(ref)[0]
    journal = read(journal_path, Journal)
    if journal is None:
        candidate = read(ref.state_root / "transaction.json.next", Journal)
        if candidate is not None and candidate.resource == ref:
            os.replace(ref.state_root / "transaction.json.next", journal_path)
            journal = candidate
    if journal is None or journal.resource != ref:
        raise failure(
            ErrorCode.RECOVERY_NEEDED, "unrecognized preparation data retained", pending[0]
        )
    current = _receipt(journal)
    if current == journal.receipt:
        cleanup(journal)
    elif current == journal.previous:
        rollback(journal)
    else:
        raise failure(
            ErrorCode.RECOVERY_NEEDED, "receipt disagrees with transaction", ref.state_root
        )
