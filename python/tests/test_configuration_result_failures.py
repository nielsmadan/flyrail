import errno
from pathlib import Path
from typing import Any

import pytest
from skill_helpers import make_bundle, snapshot
from test_configuration_lifecycle import bundle, rendered

from flyrail import (
    Bundle,
    ErrorCode,
    InstallationTarget,
    ObservationState,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    Target,
    TargetInspection,
    TargetResult,
    inspect,
    inspection,
    install,
    preview,
    sync,
    uninstall,
    update,
)
from flyrail import _lifecycle as lifecycle
from flyrail._observation import ObservationFailure
from flyrail._resource_io import observe as observe_revision
from flyrail._resource_models import ResourceRef, Revision
from flyrail._resource_plan import resource as resolve_resource
from flyrail._resource_transaction import Journal
from flyrail._resource_transaction import publish as publish_resource


def test_skill_inspection_preserves_returned_resource_errors_and_conflicts(tmp_path: Path) -> None:
    desired = make_bundle(tmp_path / "source")
    invalid = Target.directory(tmp_path / "invalid")
    invalid.root.write_bytes(b"container is a file")
    conflicting = Target.directory(tmp_path / "conflicting")
    (conflicting.root / "review").mkdir(parents=True)
    (conflicting.root / "review/SKILL.md").write_bytes(b"unowned skill")
    before = snapshot(tmp_path)

    failed, conflict = inspect(desired, [invalid, conflicting])

    assert failed.observation.state is ObservationState.UNKNOWN
    assert failed.observation.error is not None
    assert failed.observation.error.code is ErrorCode.UNSAFE_PATH
    assert failed.observation.error.path == invalid.root
    assert conflict.observation.state is ObservationState.ABSENT
    assert tuple(item.path for item in conflict.observation.conflicts) == (str(conflicting.root),)
    assert snapshot(tmp_path) == before


def test_skill_inspection_preserves_returned_aggregate_preview_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    desired = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    index = tmp_path / ".skills.flyrail-index-team"

    def observe(path: Path) -> Revision:
        if path == index:
            raise OSError(errno.EIO, "index snapshot unavailable", str(path))
        return observe_revision(path)

    before = snapshot(tmp_path)
    monkeypatch.setattr(lifecycle, "observe", observe)
    observed, alias = inspect(desired, [target, target])

    assert observed.observation.state is ObservationState.UNKNOWN
    assert observed.observation.error is not None
    assert observed.observation.error.code is ErrorCode.IO_ERROR
    assert observed.observation.error.path == index
    assert observed.observation.error.errno == errno.EIO
    assert alias.alias_of == 0 and alias.observation is observed.observation
    assert snapshot(tmp_path) == before


@pytest.mark.parametrize("preview_error", [False, True])
def test_skill_inspection_retains_recovery_state_after_interrupted_install(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, preview_error: bool
) -> None:
    desired = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")

    def interrupt(journal: Journal) -> None:
        publish_resource(journal)
        raise KeyboardInterrupt

    with monkeypatch.context() as patch:
        patch.setattr(lifecycle, "publish", interrupt)
        with pytest.raises(KeyboardInterrupt):
            install(desired, [target])
    assert (target.root / "review/SKILL.md").read_bytes() == b"original"
    index = tmp_path / ".skills.flyrail-index-team"

    def observe(path: Path) -> Revision:
        if preview_error and path == index:
            raise OSError(errno.EIO, "index snapshot unavailable", str(path))
        return observe_revision(path)

    monkeypatch.setattr(lifecycle, "observe", observe)
    before = snapshot(tmp_path)
    result = inspect(desired, [target])[0]

    assert result.observation.state is ObservationState.RECOVERY_NEEDED
    assert result.state_root / "transaction.json" in result.observation.recovery_paths
    if preview_error:
        assert result.observation.error is not None
        assert result.observation.error.code is ErrorCode.IO_ERROR
        assert result.observation.error.path == index
    assert snapshot(tmp_path) == before


@pytest.mark.parametrize(
    ("operation", "failure"),
    [
        (operation, failure)
        for operation in ["inspect", "install", "update", "uninstall"]
        for failure in ["container-link", "index-link", "render-io", "reference-io"]
        if operation != "uninstall" or failure.endswith("link")
    ],
)
def test_skill_target_failures_preserve_order_and_continue_independent_targets(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, operation: str, failure: str
) -> None:
    desired = make_bundle(tmp_path / "source")
    failed = Target.directory(tmp_path / "failed")
    healthy = Target.directory(tmp_path / "healthy")
    if operation in {"update", "uninstall"}:
        assert install(desired, [healthy])[0].status is OperationStatus.APPLIED
    failure_path = failed.root
    if failure == "container-link":
        failed.root.symlink_to(tmp_path / "missing", target_is_directory=True)
    elif failure == "index-link":
        failure_path = tmp_path / ".failed.flyrail-index-team"
        failure_path.symlink_to(tmp_path / "missing", target_is_directory=True)
    elif failure == "render-io":
        original_render = inspection.render_skills

        def render(selected: Bundle, target: Target) -> RenderedBundle:
            if target == failed:
                raise OSError(errno.EIO, "render destination unavailable", str(target.root))
            return original_render(selected, target)

        monkeypatch.setattr(inspection, "render_skills", render)
    else:

        def resource(artifact: RenderedArtifact) -> ResourceRef:
            if artifact.destination == failed.root:
                raise OSError(errno.EIO, "reference unavailable", str(artifact.destination))
            return resolve_resource(artifact)

        monkeypatch.setattr(lifecycle, "resource", resource)

    targets = [failed, healthy, healthy]
    results: tuple[TargetInspection, ...] | tuple[TargetResult, ...]
    if operation == "inspect":
        results = inspect(desired, targets)
    elif operation == "uninstall":
        results = uninstall(desired.id, targets)
    else:
        results = (install if operation == "install" else update)(desired, targets)

    assert tuple(result.target for result in results) == tuple(targets)
    error = results[0].observation.error
    assert error is not None
    assert error.code is (ErrorCode.UNSAFE_PATH if failure.endswith("link") else ErrorCode.IO_ERROR)
    assert error.path == failure_path
    assert results[0].root == failed.root
    assert results[0].observation.state is ObservationState.UNKNOWN
    assert results[1].alias_of is None
    assert results[2].alias_of == 1 and results[2].observation is results[1].observation
    if operation == "inspect":
        assert results[1].observation.state is ObservationState.ABSENT
        expected_names = sorted(
            [failure_path.name, "source"] if failure.endswith("link") else ["source"]
        )
        assert sorted(path.name for path in tmp_path.iterdir()) == expected_names
    else:
        assert isinstance(results[0], TargetResult)
        assert isinstance(results[1], TargetResult)
        assert results[0].status is OperationStatus.FAILED and results[0].error == error
        assert results[1].status is (
            OperationStatus.UNCHANGED if operation == "update" else OperationStatus.APPLIED
        )
        if operation == "uninstall":
            assert not healthy.root.exists()
        else:
            assert (healthy.root / "review/SKILL.md").read_bytes() == b"original"
    if failure.endswith("link"):
        assert failure_path.readlink() == tmp_path / "missing"


@pytest.mark.parametrize(
    ("operation", "invalid"),
    [
        (operation, invalid)
        for operation in ["inspect", "install", "update", "uninstall"]
        for invalid in ["type", "reserved-path", "source-overlap"]
        if operation != "uninstall" or invalid != "source-overlap"
    ],
)
def test_skill_request_errors_raise_before_mutating_healthy_targets(
    tmp_path: Path, operation: str, invalid: str
) -> None:
    desired = make_bundle(tmp_path / "source")
    healthy = Target.directory(tmp_path / "healthy")
    malformed: Any = (
        object()
        if invalid == "type"
        else Target.directory(tmp_path / ".flyrail-reserved.state")
        if invalid == "reserved-path"
        else Target.directory(tmp_path / "source")
    )
    before = snapshot(tmp_path)
    with pytest.raises(TypeError if invalid == "type" else ValueError):
        if operation == "uninstall":
            uninstall(desired.id, [healthy, malformed])
        elif operation == "inspect":
            inspect(desired, [healthy, malformed])
        else:
            (install if operation == "install" else update)(desired, [healthy, malformed])
    assert snapshot(tmp_path) == before


def test_sync_returns_typed_error_for_an_existing_destination_link(tmp_path: Path) -> None:
    path = tmp_path / "AGENTS.md"
    target = InstallationTarget(tmp_path / "index")
    path.symlink_to(tmp_path / "missing")
    desired, output = bundle(), rendered(path)

    proposal = preview(desired, output, target)
    result = sync(desired, output, target)

    assert result.status is OperationStatus.FAILED
    assert result.error == proposal.error
    assert result.error is not None and result.error.code is ErrorCode.UNSAFE_PATH
    assert result.error.path == path
    assert path.readlink() == tmp_path / "missing"
    assert tuple(tmp_path.iterdir()) == (path,)
    with pytest.raises(ValueError, match="lock timeout"):
        sync(desired, output, target, lock_timeout=-1)


@pytest.mark.parametrize("kind", ["os", "observation", "value", "type"])
def test_sync_reference_resolution_converts_expected_errors_only(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, kind: str
) -> None:
    path = tmp_path / "AGENTS.md"
    target = InstallationTarget(tmp_path / "index")
    failure: Exception = (
        OSError(errno.EIO, "reference observation failed", str(path))
        if kind == "os"
        else ObservationFailure(ErrorCode.IO_ERROR, "reference observation failed", path)
        if kind == "observation"
        else ValueError("invalid resource request")
        if kind == "value"
        else TypeError("invalid resource type")
    )

    def resource(artifact: RenderedArtifact) -> ResourceRef:
        raise failure

    monkeypatch.setattr(lifecycle, "resource", resource)
    if kind in {"type", "value"}:
        with pytest.raises(type(failure), match=str(failure)):
            sync(bundle(), rendered(path), target)
    else:
        result = sync(bundle(), rendered(path), target)
        assert result.status is OperationStatus.FAILED
        assert result.error is not None and result.error.code is ErrorCode.IO_ERROR
        assert result.error.path == path
        if isinstance(failure, ObservationFailure):
            assert result.error == failure.error
        else:
            assert result.error.errno == errno.EIO
    assert list(tmp_path.iterdir()) == []
