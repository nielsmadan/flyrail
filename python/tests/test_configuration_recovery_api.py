import multiprocessing as mp
import os
from collections.abc import Callable
from dataclasses import replace
from pathlib import Path

import pytest
from skill_helpers import make_bundle, snapshot
from test_configuration_lifecycle import bundle, rendered
from test_configuration_recovery import crash_worker

from flyrail import (
    Acquisition,
    ErrorCode,
    Family,
    FileContent,
    InstallationResult,
    InstallationTarget,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    ResourceAuthority,
    Target,
    apply_preview,
    inspect,
    inspect_installation,
    install,
    preview,
    preview_removal,
    recover_installation,
    remove,
    sync,
    uninstall,
    update,
)
from flyrail._codec import encode
from flyrail._resource_io import read
from flyrail._resource_models import Index


def run_crash(worker: Callable[[str, str], None], root: Path, boundary: str) -> None:
    process = mp.get_context("spawn").Process(target=worker, args=(str(root), boundary))
    process.start()
    process.join(20)
    try:
        assert process.exitcode == 77
    finally:
        if process.is_alive():
            process.kill()
            process.join()


def test_recovery_preserves_absent_and_healthy_installations(tmp_path: Path) -> None:
    target = InstallationTarget(tmp_path / "index")
    before = snapshot(tmp_path)
    assert recover_installation("team", target).status is OperationStatus.UNCHANGED
    assert snapshot(tmp_path) == before
    path = tmp_path / "AGENTS.md"
    assert sync(bundle(), rendered(path), target).observation.is_current
    before = snapshot(tmp_path)
    recovered = recover_installation("team", target)
    assert recovered.status is OperationStatus.UNCHANGED and recovered.observation.is_current
    assert snapshot(tmp_path) == before


@pytest.mark.parametrize("boundary", ["prepared", "published", "committed"])
@pytest.mark.parametrize("operation", ["update", "remove"])
def test_public_recovery_then_strict_preview_after_process_interruption(
    tmp_path: Path, boundary: str, operation: str
) -> None:
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"# foreign\n")
    target = InstallationTarget(tmp_path / "index")
    assert sync(bundle(), rendered(path), target).observation.is_current
    original = path.read_bytes()
    run_crash(crash_worker, tmp_path, boundary)
    old_preview = preview_removal("team", target)
    recovered = recover_installation("team", target)
    assert recovered.status is OperationStatus.APPLIED and recovered.error is None
    assert recovered.observation.pending
    assert all(
        not item.recovery_paths and not item.error for item in recovered.observation.resources
    )
    assert path.read_bytes() == (
        original.replace(b"generated", b"second") if boundary == "committed" else original
    )
    assert apply_preview(old_preview).status is OperationStatus.FAILED
    assert recover_installation("team", target).status is OperationStatus.UNCHANGED
    proposal = (
        preview(bundle("3"), rendered(path, "third"), target)
        if operation == "update"
        else preview_removal("team", target)
    )
    assert proposal.applicable
    applied = apply_preview(proposal)
    assert applied.status is OperationStatus.APPLIED and not applied.observation.pending
    assert path.read_bytes() == (
        original.replace(b"generated", b"third") if operation == "update" else b"# foreign\n"
    )


def test_unrecognized_recovery_retains_evidence_until_reconciled(tmp_path: Path) -> None:
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"# foreign\n")
    target = InstallationTarget(tmp_path / "index")
    assert sync(bundle(), rendered(path), target).observation.is_current
    original = path.read_bytes()
    run_crash(crash_worker, tmp_path, "prepared")
    path.write_bytes(original + b"Unexpected foreign edit.\n")
    before = snapshot(tmp_path)
    result = recover_installation("team", target)
    assert result.status is OperationStatus.INCOMPLETE
    assert result.error is not None and result.error.code is ErrorCode.RECOVERY_NEEDED
    assert result.resources[0].recovery_paths
    assert snapshot(tmp_path) == before
    path.write_bytes(original)
    assert recover_installation("team", target).status is OperationStatus.APPLIED
    assert remove("team", target).status is OperationStatus.APPLIED
    assert path.read_bytes() == b"# foreign\n"


@pytest.mark.parametrize("kind", ["section", "file"])
@pytest.mark.parametrize("membership", ["current", "previous"])
def test_missing_committed_receipt_preserves_index_and_payload_for_every_public_operation(
    tmp_path: Path, kind: str, membership: str
) -> None:
    path = tmp_path / "AGENTS.md"
    target = InstallationTarget(tmp_path / "index")
    path.write_bytes(
        b"# original\n\n<!-- flyrail:guide:start -->\nbaseline\n<!-- flyrail:guide:end -->\n"
    )
    desired = (
        rendered(path)
        if kind == "section"
        else RenderedBundle(
            [RenderedArtifact("guide", Family.INSTRUCTIONS, path, FileContent(b"generated\n"))]
        )
    )
    assert sync(bundle(), desired, target, acquisition=Acquisition.TAKEOVER).observation.is_current
    index_path = target.index_root / "index.json"
    index = preview_removal("team", target).index
    assert index is not None and index.current is not None
    if membership == "previous":
        index_path.write_bytes(encode(replace(index, current=None, previous=index.current)))
    receipt_path = ResourceAuthority(path).state_root / "receipt.json"
    saved_receipt = receipt_path.read_bytes()
    receipt_path.unlink()
    before = snapshot(tmp_path)
    observation = inspect_installation("team", target)
    assert not observation.is_current
    error = observation.resources[0].error
    assert error is not None and error.code is ErrorCode.INVALID_STATE
    assert error.path == receipt_path and "receipt is missing" in error.message
    for proposal in (
        preview(bundle(), desired, target, acquisition=Acquisition.TAKEOVER, replace_modified=True),
        preview_removal("team", target, replace_modified=True),
    ):
        assert not proposal.applicable
        result = apply_preview(proposal)
        assert result.status is OperationStatus.FAILED and result.error == error
    for result in (
        sync(bundle(), desired, target, acquisition=Acquisition.TAKEOVER, replace_modified=True),
        remove("team", target, replace_modified=True),
        recover_installation("team", target),
    ):
        assert result.status is OperationStatus.FAILED and result.error == error
    assert snapshot(tmp_path) == before
    receipt_path.write_bytes(saved_receipt)
    assert remove("team", target).status is OperationStatus.APPLIED
    assert path.read_bytes() == (
        b"# original\n\n<!-- flyrail:guide:start -->\nbaseline\n<!-- flyrail:guide:end -->\n"
    )


def test_skill_wrappers_report_missing_receipt(tmp_path: Path) -> None:
    source = make_bundle(tmp_path / "source")
    target = Target.directory(tmp_path / "skills")
    assert install(source, [target])[0].status is OperationStatus.APPLIED
    (ResourceAuthority(target.root).state_root / "receipt.json").unlink()
    before = snapshot(tmp_path)
    observation = inspect(source, [target])[0].observation
    assert not observation.is_current and observation.error is not None
    for result in (
        install(source, [target])[0],
        update(source, [target])[0],
        uninstall("team", [target])[0],
    ):
        assert result.status is OperationStatus.FAILED
        assert result.error is not None and "receipt is missing" in result.error.message
    assert snapshot(tmp_path) == before


def test_valid_retirement_receipt_reconciles_an_independent_index(tmp_path: Path) -> None:
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"# foreign\n")
    first = InstallationTarget(tmp_path / "first")
    second = InstallationTarget(tmp_path / "second")
    assert sync(bundle(), rendered(path), first).observation.is_current
    assert sync(bundle(), rendered(path), second).observation.is_current
    assert remove("team", first).status is OperationStatus.APPLIED
    observed = inspect_installation("team", second)
    assert not observed.is_current and observed.resources[0].error is None
    assert observed.resources[0].claims == ()
    assert recover_installation("team", second).status is OperationStatus.UNCHANGED
    assert preview_removal("team", second).applicable
    assert remove("team", second).status is OperationStatus.APPLIED
    assert path.read_bytes() == b"# foreign\n"


def index_crash_worker(root: str, boundary: str) -> None:
    target = InstallationTarget(Path(root) / "index")
    original_replace = os.replace
    count = 0

    def replace_metadata(source: Path, destination: Path) -> None:
        nonlocal count
        if source == target.index_root / "index.json.next":
            count += 1
            if count == (1 if boundary == "pending" else 2):
                os._exit(77)
        original_replace(source, destination)

    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(os, "replace", replace_metadata)
        sync(bundle(), rendered(Path(root) / "AGENTS.md"), target)


def test_missing_receipt_after_first_install_completion_preserves_prepared_evidence(
    tmp_path: Path,
) -> None:
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"# foreign\n")
    target = InstallationTarget(tmp_path / "index")
    run_crash(index_crash_worker, tmp_path, "completed")
    index_path = target.index_root / "index.json"
    prepared_path = target.index_root / "index.json.next"
    current = read(index_path, Index)
    prepared = read(prepared_path, Index)
    assert current is not None and current.current is None and current.previous is None
    assert current.pending is not None
    assert prepared is not None and prepared.current == current.pending
    state = ResourceAuthority(path).state_root
    assert {item.name for item in state.iterdir() if item.is_file()} == {
        "lock",
        "authority.json",
        "receipt.json",
    }
    receipt_path = state / "receipt.json"
    saved_receipt = receipt_path.read_bytes()
    receipt_path.unlink()
    before = snapshot(tmp_path)
    observation = inspect_installation("team", target)
    assert observation.pending and len(observation.resources) == 1
    error = observation.resources[0].error
    assert error is not None and error.code is ErrorCode.INVALID_STATE
    assert error.path == receipt_path and "receipt is missing" in error.message
    assert snapshot(tmp_path) == before
    for proposal in (
        preview(bundle(), rendered(path), target, replace_modified=True),
        preview_removal("team", target, replace_modified=True),
    ):
        assert not proposal.applicable and proposal.resources[0].error == error
        assert snapshot(tmp_path) == before
        result = apply_preview(proposal)
        assert result.status is OperationStatus.FAILED and result.error is not None
        assert result.observation.resources[0].error == error
        assert snapshot(tmp_path) == before
    operations: tuple[Callable[[], InstallationResult], ...] = (
        lambda: sync(bundle(), rendered(path), target, replace_modified=True),
        lambda: remove("team", target, replace_modified=True),
        lambda: recover_installation("team", target),
    )
    for operation in operations:
        result = operation()
        assert result.status is OperationStatus.FAILED and result.error == error
        assert result.observation.pending and result.observation.resources[0].error == error
        assert snapshot(tmp_path) == before
    receipt_path.write_bytes(saved_receipt)
    assert remove("team", target).status is OperationStatus.APPLIED
    assert path.read_bytes() == b"# foreign\n"


@pytest.mark.parametrize("boundary", ["pending", "completed", "prepared", "committed"])
def test_first_install_recovery_keeps_prepared_index_references_for_source_free_removal(
    tmp_path: Path, boundary: str
) -> None:
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"# foreign\n")
    target = InstallationTarget(tmp_path / "index")
    worker = index_crash_worker if boundary in {"pending", "completed"} else crash_worker
    run_crash(worker, tmp_path, boundary)
    observation = inspect_installation("team", target)
    assert observation.pending and len(observation.resources) == 1
    assert observation.resources[0].destination == path and observation.resources[0].error is None
    recovered = recover_installation("team", target)
    assert recovered.status is OperationStatus.APPLIED and recovered.error is None
    assert recovered.observation.pending and len(recovered.observation.resources) == 1
    if boundary in {"pending", "prepared"}:
        assert path.read_bytes() == b"# foreign\n"
    else:
        assert b"<!-- flyrail:guide:start -->" in path.read_bytes()
    assert remove("team", target).status is OperationStatus.APPLIED
    assert path.read_bytes() == b"# foreign\n"
    assert not inspect_installation("team", target).pending


def test_unrecognized_prepared_index_keeps_all_references(tmp_path: Path) -> None:
    path = tmp_path / "AGENTS.md"
    target = InstallationTarget(tmp_path / "index")
    assert sync(bundle(), rendered(path), target).observation.is_current
    proposal = preview(bundle("2"), rendered(tmp_path / "other"), target)
    assert proposal.index is not None and proposal.generation is not None
    prepared = replace(proposal.index, current=proposal.generation)
    (target.index_root / "index.json.next").write_bytes(encode(prepared))
    before = snapshot(tmp_path)
    result = recover_installation("team", target)
    assert result.status is OperationStatus.FAILED
    assert result.error is not None and result.error.code is ErrorCode.RECOVERY_NEEDED
    assert {item.destination for item in result.observation.resources} == {path, tmp_path / "other"}
    assert snapshot(tmp_path) == before
