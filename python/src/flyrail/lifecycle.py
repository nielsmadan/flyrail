import errno
import math
import uuid
from collections.abc import Iterable
from dataclasses import replace
from pathlib import Path

from flyrail._filesystem import target_lock
from flyrail._observation import ObservationFailure, Observer
from flyrail._receipts import Receipt, bundle_receipt
from flyrail._transaction import cleanup, prepare, publish, receipt_matches, recover
from flyrail._validation import validate_identifier
from flyrail.bundle import Bundle
from flyrail.inspection import (
    ResolvedTarget,
    _io_error,
    observe_identity,
    prepare_targets,
    scan_identity,
)
from flyrail.models import OperationStatus
from flyrail.observations import ErrorCode, Observation, ObservationState, TargetError, TargetResult
from flyrail.targets import Target


def _error(error: OSError | ObservationFailure) -> TargetError:
    if isinstance(error, ObservationFailure):
        return error.error
    result = _io_error(error)
    if error.errno in {errno.ENOTSUP, errno.ENOSYS, errno.EXDEV, errno.EINVAL}:
        return replace(result, code=ErrorCode.UNSUPPORTED)
    return result


def _observe(identifier: str, target: ResolvedTarget, bundle: Bundle | None) -> Observation:
    try:
        return observe_identity(identifier, target, bundle)
    except (OSError, ObservationFailure) as error:
        return Observation(ObservationState.UNKNOWN, error=_error(error))


def _pending(target: ResolvedTarget) -> tuple[Path, ...]:
    paths: list[Path] = []
    for name in ("transaction.json", "staging", "backup"):
        path = target.state_root / name
        try:
            observer = Observer()
            if observer.metadata(path) is not None and (
                name == "transaction.json" or observer.directory(path)
            ):
                paths.append(path)
        except (OSError, ObservationFailure):
            paths.append(path)
    return tuple(paths)


def _operate(
    identifier: str,
    target: ResolvedTarget,
    bundle: Bundle | None,
    operation: str,
    replace_modified: bool,
    timeout: float,
) -> tuple[OperationStatus, Observation, TargetError | None, tuple[Path, ...]]:
    new: Receipt | None = None
    committed = False
    failure: TargetError | None = None
    paths: tuple[Path, ...] = ()
    try:
        with target_lock(target.state_root, timeout):
            try:
                observe_identity(identifier, target, bundle)
                recover(target)
                snapshot = scan_identity(identifier, target, bundle)
                observation = snapshot.observation
                if observation.conflicts:
                    raise ObservationFailure(
                        ErrorCode.CONFLICT,
                        "skill is untracked or owned by another bundle",
                        target.root / observation.conflicts[0].path,
                    )
                if (
                    any(item.bundle_id == identifier for item in observation.modifications)
                    and not replace_modified
                ):
                    raise ObservationFailure(
                        ErrorCode.MODIFIED, "owned content has local modifications", target.root
                    )
                if observation.is_current or (bundle is None and observation.installed is None):
                    return OperationStatus.UNCHANGED, observation, None, ()
                if operation == "install" and observation.installed is not None:
                    raise ObservationFailure(
                        ErrorCode.UPDATE_REQUIRED,
                        "owned revision requires explicit update",
                        target.root,
                    )
                transaction_id = uuid.uuid4().hex
                new = (
                    Receipt(identifier, transaction_id, None, None, ())
                    if bundle is None
                    else bundle_receipt(bundle, transaction_id)
                )
                intent = prepare(target, bundle, new, snapshot)
                publish(target, intent)
                committed = True
                cleanup(target, intent)
                return OperationStatus.APPLIED, _observe(identifier, target, bundle), None, ()
            except (OSError, ObservationFailure) as error:
                failure = _error(error)
                if new is not None:
                    try:
                        if not committed:
                            committed = receipt_matches(target, new)
                        recover(target)
                    except (OSError, ObservationFailure) as recovery_error:
                        if not committed:
                            detail = _error(recovery_error)
                            failure = replace(
                                failure, message=f"{failure.message}; recovery: {detail.message}"
                            )
                paths = _pending(target)
                status = (
                    OperationStatus.APPLIED
                    if committed
                    else (OperationStatus.INCOMPLETE if paths else OperationStatus.FAILED)
                )
                return status, _observe(identifier, target, bundle), failure, paths
    except (OSError, ObservationFailure) as error:
        detail = _error(error)
        failure = (
            detail
            if failure is None
            else replace(failure, message=f"{failure.message}; lock: {detail.message}")
        )
        if committed:
            paths = _pending(target)
        status = (
            OperationStatus.APPLIED
            if committed
            else (OperationStatus.INCOMPLETE if paths else OperationStatus.FAILED)
        )
        return status, _observe(identifier, target, bundle), failure, paths


def _mutate(
    bundle: Bundle | None,
    identifier: str,
    targets: Iterable[Target],
    operation: str,
    replace_modified: bool,
    lock_timeout: float,
) -> tuple[TargetResult, ...]:
    if not isinstance(replace_modified, bool):
        raise TypeError("replace_modified must be a boolean")
    if isinstance(lock_timeout, bool) or not isinstance(lock_timeout, int | float):
        raise TypeError("lock_timeout must be a number of seconds")
    if lock_timeout < 0 or not math.isfinite(lock_timeout):
        raise ValueError("lock_timeout must be finite and nonnegative")
    resolved = prepare_targets(targets, () if bundle is None else bundle.source_roots)
    results: list[TargetResult] = []
    error: TargetError | None
    paths: tuple[Path, ...]
    for target in resolved:
        if target.alias_of is not None:
            results.append(
                replace(
                    results[target.alias_of],
                    target=target.target,
                    root=target.root,
                    state_root=target.state_root,
                    alias_of=target.alias_of,
                )
            )
            continue
        if target.error is not None:
            status, observation, error, paths = (
                OperationStatus.FAILED,
                Observation(ObservationState.UNKNOWN, error=target.error),
                target.error,
                (),
            )
        else:
            status, observation, error, paths = _operate(
                identifier, target, bundle, operation, replace_modified, lock_timeout
            )
        results.append(
            TargetResult(
                target.target,
                target.root,
                target.state_root,
                target.alias_of,
                status,
                observation,
                error,
                paths,
            )
        )
    return tuple(results)


def install(
    bundle: Bundle, targets: Iterable[Target], *, lock_timeout: float = 0
) -> tuple[TargetResult, ...]:
    if not isinstance(bundle, Bundle):
        raise TypeError("bundle must be a Bundle snapshot")
    return _mutate(bundle, bundle.id, targets, "install", False, lock_timeout)


def update(
    bundle: Bundle,
    targets: Iterable[Target],
    *,
    replace_modified: bool = False,
    lock_timeout: float = 0,
) -> tuple[TargetResult, ...]:
    if not isinstance(bundle, Bundle):
        raise TypeError("bundle must be a Bundle snapshot")
    return _mutate(bundle, bundle.id, targets, "update", replace_modified, lock_timeout)


def uninstall(
    bundle_id: str,
    targets: Iterable[Target],
    *,
    replace_modified: bool = False,
    lock_timeout: float = 0,
) -> tuple[TargetResult, ...]:
    validate_identifier(bundle_id, "bundle id")
    return _mutate(None, bundle_id, targets, "uninstall", replace_modified, lock_timeout)
