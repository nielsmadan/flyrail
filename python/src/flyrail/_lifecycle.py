import math
import os
from collections.abc import Iterator
from contextlib import ExitStack, contextmanager
from dataclasses import replace
from pathlib import Path

from flyrail._filesystem import target_lock
from flyrail._observation import ObservationFailure, Observer
from flyrail._resource_io import (
    admit,
    ancestors,
    atomic,
    failure,
    observe,
    read,
    require,
    unsupported,
    validate_kind,
    validate_state,
)
from flyrail._resource_models import Ancestor, Generation, Index, Receipt, ResourceRef, Revision
from flyrail._resource_plan import artifact_claim, plan_resource, resource, summary
from flyrail._resource_transaction import cleanup, prepare, publish, recover
from flyrail._validation import portable_path_key, validate_identifier
from flyrail.authority import Claim, _UnsafeDestination
from flyrail.bundle import Bundle
from flyrail.configuration import (
    InstallationObservation,
    InstallationResult,
    InstallationTarget,
    LifecyclePreview,
    ResourceObservation,
    ResourcePlan,
    ResourceResult,
)
from flyrail.editors import Acquisition, EditStatus
from flyrail.models import OperationStatus
from flyrail.observations import ErrorCode, TargetError
from flyrail.rendered import DependencyMode, NoticeKind, RenderedArtifact, RenderedBundle


def error_detail(error: OSError | ObservationFailure | ValueError) -> TargetError:
    if isinstance(error, ObservationFailure):
        return error.error
    if isinstance(error, _UnsafeDestination):
        return TargetError(ErrorCode.UNSAFE_PATH, str(error), error.path)
    if isinstance(error, OSError):
        return TargetError(
            ErrorCode.UNSUPPORTED if unsupported(error) else ErrorCode.IO_ERROR,
            str(error),
            Path(error.filename) if error.filename else None,
            error.errno,
        )
    return TargetError(ErrorCode.INVALID_STATE, str(error))


def _index_records(
    identifier: str, target: InstallationTarget
) -> tuple[Index | None, Index | None]:
    observer = Observer()
    observer.managed_directory(target.index_root)
    for child in observer.directory(target.index_root):
        if child.name not in {"lock", "index.json", "index.json.next"}:
            raise failure(ErrorCode.INVALID_STATE, "unknown installation index entry", child)
        observer.file(child)
    observer.finish()
    result = read(target.index_root / "index.json", Index)
    pending = read(target.index_root / "index.json.next", Index)
    for value in (result, pending):
        if value is not None and (
            value.bundle_id != identifier or value.context_digest != target.context_digest
        ):
            raise failure(
                ErrorCode.INVALID_STATE, "installation index context disagrees", target.index_root
            )
    return result, pending


def _index(identifier: str, target: InstallationTarget) -> tuple[Index | None, Index | None]:
    result, pending = _index_records(identifier, target)
    if pending is None:
        return result, None
    base = result or Index(identifier, target.context_digest)
    return replace(
        base,
        residual=tuple(
            sorted(set(base.residual) | set(pending.resources), key=lambda ref: str(ref.state_root))
        ),
    ), pending


def _completes_pending_index(current: Index | None, prepared: Index | None) -> bool:
    return (
        current is not None
        and bool(current.pending or current.previous or current.residual)
        and prepared == Index(current.bundle_id, current.context_digest, current.pending)
    )


def _expects_receipt(index: Index | None, prepared: Index | None, ref: ResourceRef) -> bool:
    generations: tuple[Generation | None, ...] = (
        () if index is None else (index.current, index.previous)
    )
    if prepared is not None and _completes_pending_index(index, prepared):
        generations += (prepared.current,)
    return any(
        ref == member
        for generation in generations
        if generation is not None
        for member, _ in generation.membership
    )


def _missing_receipt(ref: ResourceRef) -> TargetError:
    return TargetError(
        ErrorCode.INVALID_STATE,
        "recorded ownership receipt is missing; restore metadata before retrying",
        ref.state_root / "receipt.json",
    )


def _check_receipt(index: Index | None, prepared: Index | None, ref: ResourceRef) -> None:
    if (
        _expects_receipt(index, prepared, ref)
        and read(ref.state_root / "receipt.json", Receipt) is None
    ):
        error = _missing_receipt(ref)
        raise failure(error.code, error.message, ref.state_root / "receipt.json")


def _overlap(left: Path, right: Path) -> bool:
    first = tuple(portable_path_key(part) for part in left.parts)
    second = tuple(portable_path_key(part) for part in right.parts)
    return first[: min(len(first), len(second))] == second[: min(len(first), len(second))]


def _references(
    rendered: RenderedBundle,
    index: Index | None,
    target: InstallationTarget,
    bundle: Bundle | None,
    resolved: dict[str, ResourceRef] | None = None,
) -> tuple[ResourceRef, ...]:
    for source in () if bundle is None else bundle.source_roots:
        if _overlap(source, target.index_root):
            raise ValueError("installation index overlaps bundle source")
    if resolved is None:
        resolved = {artifact.id: resource(artifact) for artifact in rendered.artifacts}
    outputs: dict[ResourceRef, list[Path]] = {}
    for artifact in rendered.artifacts:
        ref = resolved[artifact.id]
        outputs.setdefault(ref, []).append(
            ref.destination / artifact.subtree if artifact.subtree else ref.destination
        )
    refs = set(resolved.values())
    if index is not None:
        refs.update(index.resources)
    result = tuple(sorted(refs, key=lambda ref: str(ref.state_root)))
    if len({ref.destination for ref in result}) != len(result):
        raise ValueError("resource kinds disagree at the same destination")
    for ref in result:
        if _overlap(ref.destination, target.index_root) or _overlap(
            ref.state_root, target.index_root
        ):
            raise ValueError("installation index overlaps a resource or authority")
        for source in () if bundle is None else bundle.source_roots:
            if _overlap(source, ref.state_root):
                raise ValueError("resource authority overlaps bundle source")
            if any(_overlap(source, output) for output in outputs.get(ref, ())):
                raise ValueError("resource output overlaps bundle source")
    return result


def _order(
    plans: tuple[ResourcePlan, ...], rendered: RenderedBundle, by_id: dict[str, ResourceRef]
) -> tuple[ResourcePlan, ...]:
    refs = {plan.resource.destination: plan.resource for plan in plans}
    edges: set[tuple[ResourceRef, ResourceRef]] = set()
    for edge in rendered.dependencies:
        dependent, dependency = by_id[edge.dependent], by_id[edge.required]
        if dependent != dependency:
            edges.add((dependent, dependency))
    for plan in plans:
        for old in () if plan.previous is None else plan.previous.claims:
            new = next(
                (item for item in plan.receipt.claims if item.claim_id == old.claim_id), None
            )
            for path, claim_id in old.requirements:
                required = refs.get(path)
                if required is None or required == plan.resource:
                    continue
                required_plan = next(item for item in plans if item.resource == required)
                old_asset = next(
                    (
                        item
                        for item in (
                            () if required_plan.previous is None else required_plan.previous.claims
                        )
                        if item.claim_id == claim_id
                    ),
                    None,
                )
                new_asset = next(
                    (item for item in required_plan.receipt.claims if item.claim_id == claim_id),
                    None,
                )
                if (
                    old_asset is not None
                    and new_asset is not None
                    and old_asset.installed != new_asset.installed
                    and (path, claim_id) not in old.stable_requirements
                ):
                    raise ValueError(
                        "referenced asset changes require a distinct revision destination"
                    )
                if old_asset is not None and new_asset is None:
                    if new is not None and (path, claim_id) in new.requirements:
                        raise ValueError("cannot retire an asset with an active reference")
                    edges.add((required, plan.resource))
    pending = {plan.resource: plan for plan in plans}
    ordered = []
    while pending:
        ready = sorted(
            (ref for ref in pending if not any(a == ref and b in pending for a, b in edges)),
            key=lambda ref: str(ref.state_root),
        )
        if not ready:
            raise ValueError("cyclic resource dependencies")
        for ref in ready:
            ordered.append(pending.pop(ref))
    return tuple(ordered)


def _preview(
    identifier: str,
    bundle: Bundle | None,
    rendered: RenderedBundle,
    target: InstallationTarget,
    acquisition: Acquisition,
    replace_modified: bool,
) -> LifecyclePreview:
    index, prepared = _index(identifier, target)
    resolved = {artifact.id: resource(artifact) for artifact in rendered.artifacts}
    refs = _references(rendered, index, target, bundle, resolved)
    by_id = {artifact.id: artifact for artifact in rendered.artifacts}
    requirements: dict[str, set[tuple[Path, str]]] = {
        artifact.id: set() for artifact in rendered.artifacts
    }
    stable_contracts: dict[str, set[tuple[Path, str]]] = {
        artifact.id: set() for artifact in rendered.artifacts
    }
    for edge in rendered.dependencies:
        required = resolved[edge.required]
        pair = (
            required.destination,
            artifact_claim(by_id[edge.required]).identity(required.authority),
        )
        requirements[edge.dependent].add(pair)
        if edge.mode is DependencyMode.STABLE_REFERENCE:
            stable_contracts[edge.dependent].add(pair)
    grouped: dict[tuple[ResourceRef, Claim], RenderedArtifact] = {}
    for artifact in rendered.artifacts:
        ref = resolved[artifact.id]
        key = (ref, artifact_claim(artifact))
        previous_artifact = grouped.get(key)
        if previous_artifact is not None:
            if (
                previous_artifact.content != artifact.content
                or previous_artifact.family != artifact.family
                or previous_artifact.schema_requirements != artifact.schema_requirements
            ):
                raise ValueError("physical aliases have incompatible desired content")
            if stable_contracts[previous_artifact.id] != stable_contracts[artifact.id]:
                raise ValueError("physical aliases have incompatible stable-reference requirements")
            grouped[key] = replace(
                previous_artifact,
                require_existing=previous_artifact.require_existing or artifact.require_existing,
            )
            requirements[previous_artifact.id].update(requirements[artifact.id])
        else:
            grouped[key] = artifact
    selected_artifacts: dict[ResourceRef, list[RenderedArtifact]] = {}
    for (ref, _), artifact in grouped.items():
        selected_artifacts.setdefault(ref, []).append(artifact)
    normalized_requirements = {key: tuple(sorted(value)) for key, value in requirements.items()}
    stable_requirements = {key: tuple(sorted(value)) for key, value in stable_contracts.items()}
    plans = tuple(
        plan_resource(
            ref,
            identifier,
            bundle,
            rendered,
            tuple(selected_artifacts.get(ref, ())),
            acquisition,
            replace_modified,
            normalized_requirements,
            stable_requirements,
        )
        for ref in refs
    )
    plans = tuple(
        replace(plan, after=plan.before, error=_missing_receipt(plan.resource))
        if plan.previous is None and _expects_receipt(index, prepared, plan.resource)
        else plan
        for plan in plans
    )
    generation = (
        None
        if bundle is None
        else Generation(
            bundle.version,
            bundle.content_digest,
            rendered.content_digest,
            tuple(
                (
                    ref,
                    tuple(
                        sorted(
                            artifact_claim(artifact).identity(ref.authority)
                            for artifact in selected_artifacts[ref]
                        )
                    ),
                )
                for ref in refs
                if ref in selected_artifacts
            ),
        )
    )
    error = (
        TargetError(ErrorCode.UNSUPPORTED, "rendered target has unsupported requirements")
        if any(notice.kind is NoticeKind.UNSUPPORTED for notice in rendered.notices)
        else None
    )
    if (target.index_root / "index.json.next").exists():
        error = TargetError(
            ErrorCode.RECOVERY_NEEDED,
            "installation index has prepared metadata",
            target.index_root / "index.json.next",
        )
    return LifecyclePreview(
        identifier,
        bundle,
        rendered,
        target,
        acquisition,
        replace_modified,
        index,
        observe(target.index_root),
        ancestors(target.index_root / "index.json"),
        generation,
        plans
        if any(plan.error is not None for plan in plans)
        else _order(plans, rendered, resolved),
        error,
    )


def _public_preview(
    identifier: str,
    bundle: Bundle | None,
    rendered: RenderedBundle,
    target: InstallationTarget,
    acquisition: Acquisition,
    replace_modified: bool,
) -> LifecyclePreview:
    try:
        return _preview(identifier, bundle, rendered, target, acquisition, replace_modified)
    except (OSError, ObservationFailure, _UnsafeDestination) as error:
        return LifecyclePreview(
            identifier,
            bundle,
            rendered,
            target,
            acquisition,
            replace_modified,
            None,
            None,
            (),
            None,
            (),
            error_detail(error),
        )


def preview(
    bundle: Bundle,
    rendered: RenderedBundle,
    target: InstallationTarget,
    *,
    acquisition: Acquisition = Acquisition.CONFLICT,
    replace_modified: bool = False,
) -> LifecyclePreview:
    if (
        not isinstance(bundle, Bundle)
        or type(rendered) is not RenderedBundle
        or type(target) is not InstallationTarget
    ):
        raise TypeError(
            "preview requires a bundle snapshot, rendered bundle and installation target"
        )
    if not isinstance(acquisition, Acquisition) or not isinstance(replace_modified, bool):
        raise TypeError("invalid acquisition or replacement policy")
    return _public_preview(bundle.id, bundle, rendered, target, acquisition, replace_modified)


def preview_removal(
    bundle_id: str, target: InstallationTarget, *, replace_modified: bool = False
) -> LifecyclePreview:
    validate_identifier(bundle_id, "bundle id")
    if type(target) is not InstallationTarget or not isinstance(replace_modified, bool):
        raise TypeError("invalid installation target or replacement policy")
    return _public_preview(
        bundle_id, None, RenderedBundle(()), target, Acquisition.CONFLICT, replace_modified
    )


def inspect_installation(bundle_id: str, target: InstallationTarget) -> InstallationObservation:
    validate_identifier(bundle_id, "bundle id")
    if type(target) is not InstallationTarget:
        raise TypeError("target must be an InstallationTarget")
    resources: list[ResourceObservation] = []
    try:
        index, prepared = _index(bundle_id, target)
        for ref in () if index is None else index.resources:
            try:
                _check_receipt(index, prepared, ref)
                resources.append(summary(ref))
            except (OSError, ObservationFailure, ValueError) as error:
                resources.append(
                    ResourceObservation(ref.destination, ref.state_root, error=error_detail(error))
                )
        current = index.current if index else None
        pending = (target.index_root / "index.json.next").exists() or (
            index is not None
            and (index.pending is not None or bool(index.residual) or index.previous is not None)
        )
        actual = {
            (item.destination, claim.claim_id)
            for item in resources
            for claim in item.claims
            if claim.bundle_id == bundle_id
        }
        expected = (
            set()
            if current is None
            else {
                (ref.destination, identifier)
                for ref, ids in current.membership
                for identifier in ids
            }
        )
        intact = all(
            not item.error
            and not item.recovery_paths
            and all(
                current is not None
                and claim.version == current.version
                and claim.bundle_digest == current.bundle_digest
                and claim.render_digest == current.render_digest
                and claim.status is EditStatus.CURRENT
                for claim in item.claims
                if claim.bundle_id == bundle_id
            )
            for item in resources
        )
        return InstallationObservation(
            bundle_id,
            target,
            tuple(resources),
            current is not None and not pending and intact and actual == expected,
            pending,
            current.version if current else None,
            current.bundle_digest if current else None,
            current.render_digest if current else None,
        )
    except (OSError, ObservationFailure, ValueError) as error:
        return InstallationObservation(
            bundle_id, target, tuple(resources), False, False, error=error_detail(error)
        )


def _timeout(timeout: float) -> None:
    if isinstance(timeout, bool) or not isinstance(timeout, int | float):
        raise TypeError("lock timeout must be a number")
    if timeout < 0 or not math.isfinite(timeout):
        raise ValueError("lock timeout must be finite and nonnegative")


@contextmanager
def locks(
    identifier: str, target: InstallationTarget, refs: tuple[ResourceRef, ...], timeout: float
) -> Iterator[tuple[Ancestor, ...]]:
    created: list[Ancestor] = []

    def record(path: Path) -> None:
        created.append(ancestors(path / "lock")[-1])

    with ExitStack() as stack:
        stack.enter_context(target_lock(target.index_root, timeout, on_created=record))
        current, prepared = _index_records(identifier, target)
        if prepared is not None:
            _promote_prepared_index(target, current, prepared)
        for ref in sorted(refs, key=lambda item: str(item.state_root)):
            validate_kind(ref)
            stack.enter_context(target_lock(ref.state_root, timeout, on_created=record))
            admit(ref)
        yield tuple(created)


def _state_matches(path: Path, expected: Revision, initial: bool) -> Revision:
    current = observe(path)
    if initial:
        if current != expected:
            raise failure(ErrorCode.CONCURRENT_CHANGE, "preview metadata changed", path)
        return current
    allowed = {"", "lock", "authority.json"}
    before = {node.path: node for node in expected.nodes}
    after = {node.path: node for node in current.nodes}
    if (
        any(after.get(key) != value for key, value in before.items())
        or not (after.keys() - before.keys()) <= allowed
    ):
        raise failure(ErrorCode.CONCURRENT_CHANGE, "preview metadata changed", path)
    return current


def _fresh(
    preview: LifecyclePreview, *, initial: bool, created: tuple[Ancestor, ...] = ()
) -> LifecyclePreview:
    if preview.index_revision is None:
        raise failure(
            ErrorCode.INVALID_STATE, "preview metadata was not observed", preview.target.index_root
        )

    def checked(path: Path, expected: tuple[Ancestor, ...]) -> tuple[Ancestor, ...]:
        current = ancestors(path)
        added = tuple(
            item for item in created if item.path in path.parents and item not in expected
        )
        approved = tuple(sorted((*expected, *added), key=lambda item: len(item.path.parts)))
        if current != approved:
            raise failure(ErrorCode.CONCURRENT_CHANGE, "preview ancestor changed", path)
        return approved

    index_revision = _state_matches(preview.target.index_root, preview.index_revision, initial)
    index_ancestors = checked(preview.target.index_root / "index.json", preview.index_ancestors)
    resources = []
    for plan in preview.resources:
        require(plan.resource.destination, plan.before)
        state_revision = _state_matches(plan.resource.state_root, plan.state_revision, initial)
        approved = checked(plan.resource.state_root / "transaction.json", plan.ancestors)
        resources.append(replace(plan, state_revision=state_revision, ancestors=approved))
    return replace(
        preview,
        index_revision=index_revision,
        index_ancestors=index_ancestors,
        resources=tuple(resources),
    )


def _require_index(preview: LifecyclePreview, revision: Revision) -> None:
    require(preview.target.index_root, revision)
    if ancestors(preview.target.index_root / "index.json") != preview.index_ancestors:
        raise failure(
            ErrorCode.CONCURRENT_CHANGE,
            "installation index ancestor changed",
            preview.target.index_root,
        )


def recovery_paths(ref: ResourceRef) -> tuple[Path, ...]:
    try:
        return validate_state(ref)
    except (OSError, ObservationFailure, ValueError):
        return (ref.state_root,)


def _run(preview: LifecyclePreview) -> InstallationResult:
    target, identifier = preview.target, preview.bundle_id
    results: list[ResourceResult] = []
    initial_error = preview.error or next(
        (plan.error for plan in preview.resources if plan.error), None
    )
    if initial_error or any(plan.recovery_paths for plan in preview.resources):
        detail = initial_error or TargetError(
            ErrorCode.RECOVERY_NEEDED, "preview requires recovery and re-observation"
        )
        return InstallationResult(
            OperationStatus.FAILED, inspect_installation(identifier, target), error=detail
        )
    preview = _fresh(preview, initial=False)
    previous = preview.index
    pending = Index(
        identifier,
        target.context_digest,
        previous.current if previous else None,
        preview.generation,
        previous.current if previous else None,
        tuple(plan.resource for plan in preview.resources),
    )
    unchanged = all(
        plan.before == plan.after and plan.previous == plan.receipt for plan in preview.resources
    )
    if (
        unchanged
        and previous is not None
        and previous.current == preview.generation
        and not previous.pending
        and not previous.residual
    ):
        return InstallationResult(
            OperationStatus.UNCHANGED,
            inspect_installation(identifier, target),
            notices=preview.rendered.notices,
        )
    if unchanged and previous is None and preview.bundle is None:
        return InstallationResult(
            OperationStatus.UNCHANGED, inspect_installation(identifier, target)
        )
    atomic(target.index_root / "index.json", pending, expected_ancestors=preview.index_ancestors)
    pending_revision = observe(target.index_root)
    failed: TargetError | None = None
    for plan in preview.resources:
        if plan.previous == plan.receipt and plan.before == plan.after:
            results.append(ResourceResult(plan.resource, OperationStatus.UNCHANGED))
            continue
        committed = False
        journal = None
        try:
            _require_index(preview, pending_revision)
            journal = prepare(plan)
            _require_index(preview, pending_revision)
            publish(journal)
            committed = True
            cleanup(journal)
            results.append(ResourceResult(plan.resource, OperationStatus.APPLIED))
        except (OSError, ObservationFailure, ValueError) as error:
            failed = error_detail(error)
            try:
                committed = committed or (
                    journal is not None
                    and read(plan.resource.state_root / "receipt.json", Receipt) == journal.receipt
                )
                recover(plan.resource)
            except (OSError, ObservationFailure, ValueError) as recovery_error:
                failed = replace(
                    failed,
                    message=failed.message + "; recovery: " + error_detail(recovery_error).message,
                )
            paths = recovery_paths(plan.resource)
            status = (
                OperationStatus.APPLIED
                if committed
                else (OperationStatus.INCOMPLETE if paths else OperationStatus.FAILED)
            )
            results.append(ResourceResult(plan.resource, status, failed, paths))
            break
    if failed is None:
        try:
            _require_index(preview, pending_revision)
            atomic(
                target.index_root / "index.json",
                Index(identifier, target.context_digest, preview.generation),
                expected_ancestors=preview.index_ancestors,
            )
        except (OSError, ObservationFailure, ValueError) as error:
            failed = error_detail(error)
    applied = any(item.status is OperationStatus.APPLIED for item in results)
    status = (
        OperationStatus.APPLIED
        if failed is None
        else (
            OperationStatus.PARTIAL
            if applied
            else (
                OperationStatus.INCOMPLETE
                if any(item.status is OperationStatus.INCOMPLETE for item in results)
                else OperationStatus.FAILED
            )
        )
    )
    return InstallationResult(
        status,
        inspect_installation(identifier, target),
        tuple(results),
        failed,
        preview.rendered.notices,
    )


def apply_preview(preview: LifecyclePreview, *, lock_timeout: float = 0) -> InstallationResult:
    if type(preview) is not LifecyclePreview:
        raise TypeError("apply requires a LifecyclePreview")
    _timeout(lock_timeout)
    if preview.error is not None:
        return InstallationResult(
            OperationStatus.FAILED,
            inspect_installation(preview.bundle_id, preview.target),
            error=preview.error,
            notices=preview.rendered.notices,
        )
    if not preview.rendered.supported:
        return _unsupported_result(preview.bundle_id, preview.rendered, preview.target)
    try:
        _fresh(preview, initial=True)
        with locks(
            preview.bundle_id,
            preview.target,
            tuple(plan.resource for plan in preview.resources),
            lock_timeout,
        ) as created:
            return _run(_fresh(preview, initial=False, created=created))
    except (OSError, ObservationFailure, ValueError) as error:
        return InstallationResult(
            OperationStatus.FAILED,
            inspect_installation(preview.bundle_id, preview.target),
            error=error_detail(error),
        )


def _unsupported_result(
    identifier: str, rendered: RenderedBundle, target: InstallationTarget
) -> InstallationResult:
    return InstallationResult(
        OperationStatus.FAILED,
        inspect_installation(identifier, target),
        error=TargetError(ErrorCode.UNSUPPORTED, "rendered target has unsupported requirements"),
        notices=rendered.notices,
    )


def _promote_prepared_index(
    target: InstallationTarget, current: Index | None, prepared: Index
) -> bool:
    if prepared == current:
        return False
    if _completes_pending_index(current, prepared):
        return False
    previous = current.current if current else None
    previous_refs = set(current.resources) if current else set()
    desired_refs = {ref for ref, _ in prepared.pending.membership} if prepared.pending else set()
    if (
        prepared.current == previous
        and prepared.previous == previous
        and set(prepared.residual) == previous_refs | desired_refs
    ):
        return True
    raise failure(
        ErrorCode.RECOVERY_NEEDED,
        "unrecognized installation index preparation retained",
        target.index_root / "index.json.next",
    )


def _recover_locked(
    identifier: str, target: InstallationTarget, index: Index | None, refs: tuple[ResourceRef, ...]
) -> InstallationResult:
    if _index(identifier, target)[0] != index:
        raise failure(
            ErrorCode.CONCURRENT_CHANGE, "index changed while acquiring locks", target.index_root
        )
    current, prepared = _index_records(identifier, target)
    promote = _promote_prepared_index(target, current, prepared) if prepared is not None else False
    revision = observe(target.index_root)
    parents = ancestors(target.index_root / "index.json")
    for ref in refs:
        _check_receipt(index, prepared, ref)
    results: list[ResourceResult] = []
    for ref in refs:
        try:
            pending = bool(validate_state(ref))
            recover(ref)
            results.append(
                ResourceResult(
                    ref, OperationStatus.APPLIED if pending else OperationStatus.UNCHANGED
                )
            )
        except (OSError, ObservationFailure, ValueError) as error:
            result = ResourceResult(
                ref, OperationStatus.INCOMPLETE, error_detail(error), recovery_paths(ref)
            )
            return InstallationResult(
                OperationStatus.INCOMPLETE,
                inspect_installation(identifier, target),
                (*results, result),
                result.error,
            )
    if prepared is not None:
        require(target.index_root, revision)
        if ancestors(target.index_root / "index.json") != parents:
            raise failure(ErrorCode.CONCURRENT_CHANGE, "index ancestor changed", target.index_root)
        next_path = target.index_root / "index.json.next"
        if promote:
            os.replace(next_path, target.index_root / "index.json")
        else:
            next_path.unlink()
    changed = prepared is not None or any(
        result.status is OperationStatus.APPLIED for result in results
    )
    return InstallationResult(
        OperationStatus.APPLIED if changed else OperationStatus.UNCHANGED,
        inspect_installation(identifier, target),
        tuple(results),
    )


def recover_installation(
    bundle_id: str, target: InstallationTarget, *, lock_timeout: float = 0
) -> InstallationResult:
    validate_identifier(bundle_id, "bundle id")
    if type(target) is not InstallationTarget:
        raise TypeError("target must be an InstallationTarget")
    _timeout(lock_timeout)
    try:
        index, _ = _index(bundle_id, target)
        if index is None:
            return InstallationResult(
                OperationStatus.UNCHANGED, inspect_installation(bundle_id, target)
            )
        refs = _references(RenderedBundle(()), index, target, None)
        with locks(bundle_id, target, refs, lock_timeout):
            return _recover_locked(bundle_id, target, index, refs)
    except (OSError, ObservationFailure, ValueError) as error:
        return InstallationResult(
            OperationStatus.FAILED,
            inspect_installation(bundle_id, target),
            error=error_detail(error),
        )


def _sync(
    identifier: str,
    bundle: Bundle | None,
    rendered: RenderedBundle,
    target: InstallationTarget,
    acquisition: Acquisition,
    replace_modified: bool,
    lock_timeout: float,
) -> InstallationResult:
    _timeout(lock_timeout)
    if not rendered.supported:
        return _unsupported_result(identifier, rendered, target)
    try:
        index, _ = _index(identifier, target)
        refs = _references(rendered, index, target, bundle)
        with locks(identifier, target, refs, lock_timeout):
            recovered = _recover_locked(identifier, target, index, refs)
            if recovered.error is not None:
                return recovered
            return _run(
                _preview(identifier, bundle, rendered, target, acquisition, replace_modified)
            )
    except (OSError, ObservationFailure, ValueError) as error:
        return InstallationResult(
            OperationStatus.FAILED,
            inspect_installation(identifier, target),
            error=error_detail(error),
        )


def sync(
    bundle: Bundle,
    rendered: RenderedBundle,
    target: InstallationTarget,
    *,
    acquisition: Acquisition = Acquisition.CONFLICT,
    replace_modified: bool = False,
    lock_timeout: float = 0,
) -> InstallationResult:
    if (
        not isinstance(bundle, Bundle)
        or type(rendered) is not RenderedBundle
        or type(target) is not InstallationTarget
    ):
        raise TypeError("sync requires a bundle snapshot, rendered bundle and installation target")
    if not isinstance(acquisition, Acquisition) or not isinstance(replace_modified, bool):
        raise TypeError("invalid acquisition or replacement policy")
    _timeout(lock_timeout)
    try:
        _references(rendered, None, target, bundle)
    except (OSError, ObservationFailure, _UnsafeDestination) as error:
        return InstallationResult(
            OperationStatus.FAILED,
            inspect_installation(bundle.id, target),
            error=error_detail(error),
            notices=rendered.notices,
        )
    return _sync(bundle.id, bundle, rendered, target, acquisition, replace_modified, lock_timeout)


def remove(
    bundle_id: str,
    target: InstallationTarget,
    *,
    replace_modified: bool = False,
    lock_timeout: float = 0,
) -> InstallationResult:
    validate_identifier(bundle_id, "bundle id")
    if type(target) is not InstallationTarget or not isinstance(replace_modified, bool):
        raise TypeError("invalid installation target or replacement policy")
    return _sync(
        bundle_id,
        None,
        RenderedBundle(()),
        target,
        Acquisition.CONFLICT,
        replace_modified,
        lock_timeout,
    )
