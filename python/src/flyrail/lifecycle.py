from collections.abc import Iterable
from dataclasses import replace

from flyrail._lifecycle import (
    apply_preview,
    inspect_installation,
    preview,
    preview_removal,
    recover_installation,
    remove,
    sync,
)
from flyrail._validation import validate_identifier
from flyrail.bundle import Bundle
from flyrail.inspection import prepare_skills, skill_observation
from flyrail.models import OperationStatus
from flyrail.observations import ErrorCode, Observation, ObservationState, TargetError, TargetResult
from flyrail.targets import Target

__all__ = [
    "apply_preview",
    "inspect_installation",
    "install",
    "preview",
    "preview_removal",
    "recover_installation",
    "remove",
    "sync",
    "uninstall",
    "update",
]


def _skills(
    bundle: Bundle | None,
    identifier: str,
    targets: Iterable[Target],
    operation: str,
    replace_modified: bool,
    lock_timeout: float,
) -> tuple[TargetResult, ...]:
    from flyrail._lifecycle import _timeout

    _timeout(lock_timeout)
    if not isinstance(replace_modified, bool):
        raise TypeError("replace_modified must be a boolean")
    results: list[TargetResult] = []
    for item, rendered in prepare_skills(bundle, identifier, targets):
        if item.alias_of is not None:
            results.append(
                replace(results[item.alias_of], target=item.target, alias_of=item.alias_of)
            )
            continue
        if isinstance(item.installation, TargetError):
            results.append(
                TargetResult(
                    item.target,
                    item.root,
                    item.state_root,
                    None,
                    OperationStatus.FAILED,
                    Observation(ObservationState.UNKNOWN, error=item.installation),
                    item.installation,
                )
            )
            continue
        prior = inspect_installation(identifier, item.installation)
        if (
            operation == "install"
            and prior.version is not None
            and bundle is not None
            and (prior.bundle_digest != bundle.content_digest or prior.version != bundle.version)
        ):
            error = TargetError(
                ErrorCode.UPDATE_REQUIRED, "owned revision requires explicit update", item.root
            )
            results.append(
                TargetResult(
                    item.target,
                    item.root,
                    item.state_root,
                    None,
                    OperationStatus.FAILED,
                    skill_observation(prior, bundle, rendered),
                    error,
                )
            )
            continue
        result = (
            remove(
                identifier,
                item.installation,
                replace_modified=replace_modified,
                lock_timeout=lock_timeout,
            )
            if bundle is None or rendered is None
            else sync(
                bundle,
                rendered,
                item.installation,
                replace_modified=replace_modified,
                lock_timeout=lock_timeout,
            )
        )
        results.append(
            TargetResult(
                item.target,
                item.root,
                item.state_root,
                None,
                result.status,
                skill_observation(result.observation, bundle, rendered),
                result.error,
                tuple(path for resource in result.resources for path in resource.recovery_paths),
            )
        )
    return tuple(results)


def install(
    bundle: Bundle, targets: Iterable[Target], *, lock_timeout: float = 0
) -> tuple[TargetResult, ...]:
    if not isinstance(bundle, Bundle):
        raise TypeError("bundle must be a Bundle snapshot")
    return _skills(bundle, bundle.id, targets, "install", False, lock_timeout)


def update(
    bundle: Bundle,
    targets: Iterable[Target],
    *,
    replace_modified: bool = False,
    lock_timeout: float = 0,
) -> tuple[TargetResult, ...]:
    if not isinstance(bundle, Bundle):
        raise TypeError("bundle must be a Bundle snapshot")
    return _skills(bundle, bundle.id, targets, "update", replace_modified, lock_timeout)


def uninstall(
    bundle_id: str,
    targets: Iterable[Target],
    *,
    replace_modified: bool = False,
    lock_timeout: float = 0,
) -> tuple[TargetResult, ...]:
    validate_identifier(bundle_id, "bundle id")
    return _skills(None, bundle_id, targets, "uninstall", replace_modified, lock_timeout)
