import stat
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path

from flyrail._inventory import InventoryEntry, inventory
from flyrail._observation import ObservationFailure, Observer
from flyrail._receipts import Receipt, read_receipt, reconcile_receipts
from flyrail._validation import portable_path_key, validate_identifier
from flyrail.bundle import Bundle, _content_digest
from flyrail.models import BundleEntry
from flyrail.observations import (
    Conflict,
    ErrorCode,
    Installation,
    Modification,
    ModificationKind,
    Observation,
    ObservationState,
    TargetError,
    TargetInspection,
)
from flyrail.targets import Target


@dataclass(frozen=True, slots=True)
class ResolvedTarget:
    target: Target
    root: Path
    state_root: Path
    anchor: Path
    alias_of: int | None = None
    error: TargetError | None = None


def _io_error(error: OSError) -> TargetError:
    return TargetError(
        ErrorCode.IO_ERROR,
        str(error),
        None if error.filename is None else Path(error.filename),
        error.errno,
    )


def _overlaps(left: Path, right: Path) -> bool:
    first = tuple(portable_path_key(part) for part in left.parts)
    second = tuple(portable_path_key(part) for part in right.parts)
    length = min(len(first), len(second))
    return first[:length] == second[:length]


def prepare_targets(
    targets: Iterable[Target], source_roots: tuple[Path, ...] = ()
) -> tuple[ResolvedTarget, ...]:
    requested = tuple(targets)
    if any(not isinstance(target, Target) for target in requested):
        raise TypeError("targets must contain only Target values")
    if not requested:
        raise ValueError("at least one target is required")
    result: list[ResolvedTarget] = []
    identities: dict[tuple[int, int], int] = {}
    roots: dict[Path, int] = {}
    for target in requested:
        error = None
        identity = None
        anchor, root = target._anchor, target.root
        try:
            observer = Observer()
            observer.metadata(anchor)
            try:
                anchor = anchor.resolve(strict=False)
            except RuntimeError as failure:
                if not str(failure).startswith("Symlink loop from "):
                    raise
                raise ObservationFailure(
                    ErrorCode.UNSAFE_PATH, "symlink loop while resolving target", anchor
                ) from failure
            if anchor != anchor.parent:
                matches = observer.directory(anchor.parent, matching=anchor.name)
                metadata = observer.metadata(anchor)
                if matches and metadata is not None:
                    physical = observer.metadata(matches[0])
                    if physical is not None and (metadata.st_dev, metadata.st_ino) == (
                        physical.st_dev,
                        physical.st_ino,
                    ):
                        anchor = matches[0]
            root = anchor.joinpath(*target._suffix)
            if root == root.parent:
                raise ValueError("a filesystem root cannot be a skill container")
            # Managed components are checked before stat can follow them for alias detection.
            for path in (
                anchor,
                *[anchor.joinpath(*target._suffix[:n]) for n in range(1, len(target._suffix) + 1)],
            ):
                metadata = observer.metadata(path)
                if metadata is not None and not stat.S_ISDIR(metadata.st_mode):
                    raise ObservationFailure(ErrorCode.UNSAFE_PATH, "expected a directory", path)
                if path != anchor:
                    observer.managed_directory(path)
            metadata = observer.metadata(root)
            if metadata is not None:
                identity = (metadata.st_dev, metadata.st_ino)
            observer.finish()
        except OSError as failure:
            error = _io_error(failure)
        except ObservationFailure as failure:
            error = failure.error
        state = root.with_name(f".{root.name}.flyrail")
        alias = roots.get(root)
        if alias is None and identity is not None:
            alias = identities.get(identity)
        item = ResolvedTarget(target, root, state, anchor, alias, error)
        for source in source_roots:
            if _overlaps(source, root) or _overlaps(source, state):
                raise ValueError("bundle source, target and state roots must not overlap")
        if alias is None:
            for previous in result:
                if any(
                    _overlaps(left, right)
                    for left in (root, state)
                    for right in (previous.root, previous.state_root)
                ):
                    raise ValueError("target and state roots must not overlap other destinations")
            roots[root] = len(result)
            if identity is not None:
                identities[identity] = len(result)
        result.append(item)
    return tuple(result)


def _state(observer: Observer, root: Path) -> tuple[tuple[Receipt, ...], tuple[Path, ...]]:
    observer.managed_directory(root)
    children = observer.directory(root)
    recovery: list[Path] = []
    receipts: list[Receipt] = []
    for child in children:
        if child.name == "receipts":
            for file in observer.directory(child):
                try:
                    if file.suffix != ".json":
                        raise ValueError("receipt filenames must end in .json")
                    validate_identifier(file.stem, "receipt filename")
                    receipts.append(read_receipt(observer.file(file), file.stem))
                except ValueError as error:
                    raise ObservationFailure(ErrorCode.INVALID_STATE, str(error), file) from error
        elif child.name == "lock":
            metadata = observer.metadata(child)
            if metadata is None or not stat.S_ISREG(metadata.st_mode):
                raise ObservationFailure(
                    ErrorCode.INVALID_STATE, "lock must be a regular file", child
                )
        elif child.name == "transaction.json":
            metadata = observer.metadata(child)
            if metadata is None or not stat.S_ISREG(metadata.st_mode):
                raise ObservationFailure(
                    ErrorCode.INVALID_STATE, "transaction.json must be a regular file", child
                )
            recovery.append(child)
        elif child.name in {"staging", "backup"}:
            if observer.directory(child):
                recovery.append(child)
        else:
            raise ObservationFailure(
                ErrorCode.INVALID_STATE, "unknown management state entry", child
            )
    result = tuple(receipts)
    try:
        reconcile_receipts(result)
    except ValueError as error:
        raise ObservationFailure(ErrorCode.INVALID_STATE, str(error), root) from error
    return result, tuple(recovery)


def _changes(
    bundle_id: str, expected: tuple[InventoryEntry, ...], actual: tuple[InventoryEntry, ...]
) -> tuple[Modification, ...]:
    before = {entry.path: entry for entry in expected}
    after = {entry.path: entry for entry in actual}
    result: list[Modification] = []
    for path in sorted(before.keys() | after.keys(), key=lambda value: value.encode("utf-8")):
        old, new = before.get(path), after.get(path)
        kinds: list[ModificationKind] = []
        if old is None:
            kinds.append(ModificationKind.ADDED)
        elif new is None:
            kinds.append(ModificationKind.MISSING)
        elif old.is_directory != new.is_directory:
            kinds.append(ModificationKind.TYPE_CHANGED)
        else:
            if old.size != new.size or old.sha256 != new.sha256:
                kinds.append(ModificationKind.CONTENT_CHANGED)
            if old.executable != new.executable:
                kinds.append(ModificationKind.EXECUTABLE_CHANGED)
        result.extend(Modification(bundle_id, path, kind) for kind in kinds)
    return tuple(result)


def _installation(receipt: Receipt) -> Installation:
    if receipt.version is None or receipt.content_digest is None:
        raise ValueError("removed receipt has no installation")
    return Installation(
        receipt.bundle_id,
        receipt.version,
        receipt.content_digest,
        receipt.transaction_id,
        receipt.entries,
    )


@dataclass(frozen=True, slots=True)
class TargetSnapshot:
    observation: Observation
    receipts: tuple[Receipt, ...]
    entries: tuple[InventoryEntry, ...]


def scan_identity(
    bundle_id: str, target: ResolvedTarget, bundle: Bundle | None = None
) -> TargetSnapshot:
    observer = Observer()
    for path in (
        target.anchor,
        *[
            target.anchor.joinpath(*target.target._suffix[:n])
            for n in range(1, len(target.target._suffix) + 1)
        ],
    ):
        metadata = observer.metadata(path)
        if metadata is not None and not stat.S_ISDIR(metadata.st_mode):
            raise ObservationFailure(ErrorCode.UNSAFE_PATH, "expected a directory", path)
        if path != target.anchor:
            observer.managed_directory(path)
    root_children = observer.directory(target.root)
    by_key = {portable_path_key(path.name): path.name for path in root_children}
    receipts, recovery_paths = _state(observer, target.state_root)
    owners = reconcile_receipts(receipts)
    active = tuple(receipt for receipt in receipts if not receipt.removed)
    own = next((receipt for receipt in active if receipt.bundle_id == bundle_id), None)
    desired = set() if bundle is None else {skill.name for skill in bundle.skills}
    names = desired | owners.keys()
    actual: dict[str, tuple[BundleEntry, ...]] = {}
    conflicts: list[Conflict] = []
    intent = frozenset(
        entry.path for receipt in active for entry in receipt.entries if entry.executable
    )
    for name in sorted(names):
        spelling = by_key.get(name, name)
        actual[name] = observer.tree(target.root, spelling, intent)
        if name in desired and (
            (name in owners and owners[name] != bundle_id)
            or (name not in owners and name in by_key)
            or spelling != name
        ):
            conflicts.append(Conflict(spelling, owners.get(name)))
    modifications: list[Modification] = []
    installations: list[Installation] = []
    for receipt in active:
        entries = tuple(entry for name in receipt.skills for entry in actual[name])
        changes = _changes(receipt.bundle_id, receipt.entries, inventory(entries))
        modifications.extend(changes)
        if not changes and _content_digest(entries) != receipt.content_digest:
            raise ObservationFailure(
                ErrorCode.INVALID_STATE,
                "receipt content digest disagrees with its intact inventory",
                target.state_root / "receipts" / f"{receipt.bundle_id}.json",
            )
        installations.append(_installation(receipt))
    selected = desired | (set() if own is None else set(own.skills))
    entries = tuple(entry for name in sorted(selected) for entry in actual[name])
    observer.finish()
    observation = Observation(
        ObservationState.RECOVERY_NEEDED
        if recovery_paths
        else (ObservationState.INSTALLED if own is not None else ObservationState.ABSENT),
        None if own is None else _installation(own),
        tuple(installations),
        None if own is None or bundle is None else own.version == bundle.version,
        None if own is None or bundle is None else own.content_digest == bundle.content_digest,
        None if bundle is None else _content_digest(entries) == bundle.content_digest,
        tuple(modifications),
        tuple(conflicts),
        TargetError(
            ErrorCode.RECOVERY_NEEDED,
            "management state requires mutation-time recovery",
            recovery_paths[0],
        )
        if recovery_paths
        else None,
        recovery_paths,
    )

    return TargetSnapshot(observation, receipts, inventory(entries))


def observe_identity(
    bundle_id: str, target: ResolvedTarget, bundle: Bundle | None = None
) -> Observation:
    return scan_identity(bundle_id, target, bundle).observation


def observe(bundle: Bundle, target: ResolvedTarget) -> Observation:
    return observe_identity(bundle.id, target, bundle)


def inspect(bundle: Bundle, targets: Iterable[Target]) -> tuple[TargetInspection, ...]:
    if not isinstance(bundle, Bundle):
        raise TypeError("bundle must be a Bundle snapshot")
    resolved = prepare_targets(targets, bundle.source_roots)
    results: list[TargetInspection] = []
    for target in resolved:
        if target.alias_of is not None:
            observation = results[target.alias_of].observation
        elif target.error is not None:
            observation = Observation(ObservationState.UNKNOWN, error=target.error)
        else:
            try:
                observation = observe(bundle, target)
            except OSError as error:
                observation = Observation(ObservationState.UNKNOWN, error=_io_error(error))
            except ObservationFailure as error:
                observation = Observation(ObservationState.UNKNOWN, error=error.error)
        results.append(
            TargetInspection(
                target.target, target.root, target.state_root, target.alias_of, observation
            )
        )
    return tuple(results)
