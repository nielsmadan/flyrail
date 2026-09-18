import errno
import os
from collections.abc import Callable
from dataclasses import replace
from pathlib import Path
from typing import Any

import pytest
from test_configuration_lifecycle import bundle, rendered

from flyrail import (
    Bundle,
    BundleEntry,
    BundleIdentity,
    Family,
    InstallationTarget,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    ResourceAuthority,
    SkillArtifact,
    Target,
    TreeContent,
    apply_preview,
    inspect,
    inspect_installation,
    install,
    preview,
    preview_removal,
    remove,
    render_skills,
    sync,
    uninstall,
    update,
)
from flyrail import _lifecycle as lifecycle
from flyrail import _resource_transaction as tx
from flyrail._codec import encode
from flyrail.configuration import ResourcePlan


@pytest.mark.parametrize(
    "field,value",
    [
        ("resource", []),
        ("receipt", []),
        ("previous", []),
        ("before", []),
        ("require_existing", []),
        ("error", []),
        ("state_revision", []),
    ],
)
def test_preview_resource_rejects_borrowed_mutable_state(
    tmp_path: Path, field: str, value: Any
) -> None:
    plan = preview(
        bundle(), rendered(tmp_path / "file"), InstallationTarget(tmp_path / "index")
    ).resources[0]
    with pytest.raises((TypeError, ValueError)):
        replace(plan, **{field: value})


@pytest.mark.parametrize(
    "field,value",
    [
        ("bundle", []),
        ("bundle_id", "other"),
        ("rendered", []),
        ("target", []),
        ("acquisition", []),
        ("replace_modified", []),
        ("index", []),
        ("generation", []),
        ("index_revision", []),
        ("error", []),
    ],
)
def test_preview_rejects_mutable_or_inconsistent_public_inputs(
    tmp_path: Path, field: str, value: Any
) -> None:
    plan = preview(bundle(), rendered(tmp_path / "file"), InstallationTarget(tmp_path / "index"))
    with pytest.raises((TypeError, ValueError)):
        replace(plan, **{field: value})


@pytest.mark.parametrize("kind", ["index", "resource", "claim", "result", "resource-result"])
@pytest.mark.parametrize("field", ["error", "shape"])
def test_public_summary_values_reject_mutable_payloads(
    tmp_path: Path, kind: str, field: str
) -> None:
    result = sync(bundle(), rendered(tmp_path / "file"), InstallationTarget(tmp_path / "index"))
    values: dict[str, Any] = {
        "index": result.observation,
        "resource": result.observation.resources[0],
        "claim": result.observation.resources[0].claims[0],
        "result": result,
        "resource-result": result.resources[0],
    }
    shapes = {
        "index": "target",
        "resource": "claims",
        "claim": "status",
        "result": "status",
        "resource-result": "status",
    }
    key = shapes[kind] if field == "shape" or kind == "claim" else "error"
    with pytest.raises((TypeError, ValueError)):
        replace(values[kind], **{key: [[]]})


@pytest.mark.parametrize(
    "operation",
    [
        "preview",
        "apply",
        "remove",
        "preview-removal",
        "inspect",
        "sync",
        "skill-install",
        "skill-update",
        "skills",
        "skill-inspect",
    ],
)
def test_public_entrypoints_reject_wrong_values(tmp_path: Path, operation: str) -> None:
    target = InstallationTarget(tmp_path / "index")
    desired = rendered(tmp_path / "file")
    invalid: Any = []
    calls: dict[str, Callable[[], object]] = {
        "preview": lambda: preview(invalid, desired, target),
        "apply": lambda: apply_preview(invalid),
        "remove": lambda: remove("team", invalid),
        "preview-removal": lambda: preview_removal("team", invalid),
        "inspect": lambda: inspect_installation("team", invalid),
        "sync": lambda: sync(invalid, desired, target),
        "skill-install": lambda: install(invalid, []),
        "skill-update": lambda: update(invalid, []),
        "skills": lambda: render_skills(invalid, Target.directory(tmp_path / "skills")),
        "skill-inspect": lambda: inspect(invalid, []),
    }
    with pytest.raises((TypeError, ValueError)):
        calls[operation]()
    assert list(tmp_path.iterdir()) == []


@pytest.mark.parametrize(
    "seam", ["pending-index", "final-index", "journal-next", "receipt-next", "before-journal"]
)
def test_index_and_metadata_replacement_failures_preserve_membership(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, seam: str
) -> None:
    path = tmp_path / "file"
    target = InstallationTarget(tmp_path / "index")
    if seam != "pending-index":
        assert sync(bundle(), rendered(path), target).status is OperationStatus.APPLIED
    original_replace = os.replace
    count = 0

    def failing(source: Path, destination: Path) -> None:
        nonlocal count
        if destination.name == "index.json":
            count += 1
        should_fail = (
            (seam == "pending-index" and destination.name == "index.json")
            or (seam == "final-index" and destination.name == "index.json" and count == 2)
            or (seam == "journal-next" and destination.name == "transaction.json")
            or (seam == "receipt-next" and destination.name == "receipt.json")
        )
        if should_fail:
            raise OSError(errno.EIO, "injected atomic replacement failure")
        original_replace(source, destination)

    monkeypatch.setattr(os, "replace", failing)
    original_prepare = tx.prepare

    def interrupted(plan: ResourcePlan) -> tx.Journal:
        if seam == "before-journal":
            state = plan.resource.state_root / "staging"
            state.mkdir(exist_ok=True)
            (state / "unknown").write_bytes(b"retained")
            raise OSError(errno.EIO, "interrupted staging")
        return original_prepare(plan)

    monkeypatch.setattr(lifecycle, "prepare", interrupted)
    result = sync(bundle("2"), rendered(path, "second"), target)
    assert result.status in {
        OperationStatus.FAILED,
        OperationStatus.PARTIAL,
        OperationStatus.INCOMPLETE,
    }
    monkeypatch.setattr(os, "replace", original_replace)
    monkeypatch.setattr(lifecycle, "prepare", tx.prepare)
    if seam == "before-journal":
        retry = remove("team", target)
        assert retry.status is OperationStatus.INCOMPLETE
        assert (
            ResourceAuthority(path).state_root / "staging" / "unknown"
        ).read_bytes() == b"retained"
    else:
        retry = remove("team", target)
        assert retry.status in {OperationStatus.APPLIED, OperationStatus.UNCHANGED}
        assert path.exists() is False


def test_schema2_skill_uses_same_authority_and_other_owner_is_protected(tmp_path: Path) -> None:
    tree = TreeContent([BundleEntry("SKILL.md", b"skill")])
    b = Bundle.from_artifacts(
        BundleIdentity("team", "1"), [SkillArtifact("artifact", "example", tree)]
    )
    target = Target.directory(tmp_path / "skills")
    assert install(b, [target])[0].status is OperationStatus.APPLIED
    assert inspect(b, [target])[0].observation.is_current
    other = Bundle.from_artifacts(
        BundleIdentity("other", "1"), [SkillArtifact("artifact", "example", tree)]
    )
    assert install(other, [target])[0].status is OperationStatus.FAILED
    assert inspect(other, [target])[0].observation.conflicts
    assert uninstall("team", [target])[0].status is OperationStatus.APPLIED


@pytest.mark.parametrize(
    "target_kind",
    [
        "index-overlap",
        "resource-kind",
        "unknown-state",
        "header-mismatch",
        "authority-next",
        "missing-index-receipt",
    ],
)
def test_resource_identity_and_reserved_state_checks(tmp_path: Path, target_kind: str) -> None:
    path = tmp_path / "file"
    target = InstallationTarget(tmp_path / "index")
    if target_kind == "index-overlap":
        with pytest.raises(ValueError):
            sync(bundle(), rendered(path), InstallationTarget(path / "index"))
        return
    if target_kind == "resource-kind":
        desired = RenderedBundle(
            [
                *rendered(path).artifacts,
                RenderedArtifact("tree", Family.SKILLS, path, TreeContent([])),
            ]
        )
        with pytest.raises(ValueError):
            sync(bundle(), desired, target)
        return
    assert sync(bundle(), rendered(path), target).status is OperationStatus.APPLIED
    state = ResourceAuthority(path).state_root
    if target_kind == "unknown-state":
        (state / "unknown").write_bytes(b"state")
    elif target_kind == "header-mismatch":
        other = (
            preview(
                bundle(), rendered(tmp_path / "other"), InstallationTarget(tmp_path / "other-index")
            )
            .resources[0]
            .resource
        )
        (state / "authority.json").write_bytes(encode(other))
    elif target_kind == "authority-next":
        (state / "authority.json").rename(state / "authority.json.next")
        assert sync(bundle(), rendered(path), target).status is OperationStatus.UNCHANGED
        return
    else:
        (state / "receipt.json").unlink()
        index = (target.index_root / "index.json").read_bytes()
        before = path.read_bytes()
        result = remove("team", target)
        assert result.status is OperationStatus.FAILED
        assert result.error is not None and "receipt is missing" in result.error.message
        assert (target.index_root / "index.json").read_bytes() == index
        assert path.read_bytes() == before
        return
    before = path.read_bytes()
    result = sync(bundle("2"), rendered(path, "next"), target)
    assert result.status is OperationStatus.FAILED and path.read_bytes() == before


def test_nested_subtree_parents_are_created_without_owning_foreign_container(
    tmp_path: Path,
) -> None:
    path = tmp_path / "skills"
    path.mkdir()
    (path / "foreign").write_bytes(b"keep")
    desired = RenderedBundle(
        [
            RenderedArtifact(
                "nested",
                Family.SKILLS,
                path,
                TreeContent([BundleEntry("SKILL.md", b"text")]),
                subtree="namespace/skill",
            )
        ]
    )
    target = InstallationTarget(tmp_path / "index")
    assert sync(bundle(), desired, target).status is OperationStatus.APPLIED
    assert remove("team", target).status is OperationStatus.APPLIED
    assert (path / "foreign").read_bytes() == b"keep"


def test_prepared_index_marks_status_pending_and_requires_fresh_sync(tmp_path: Path) -> None:
    path = tmp_path / "AGENTS.md"
    target = InstallationTarget(tmp_path / "index")
    assert sync(bundle(), rendered(path), target).status is OperationStatus.APPLIED
    payload = (target.index_root / "index.json").read_bytes()
    (target.index_root / "index.json.next").write_bytes(payload)
    observed = inspect_installation("team", target)
    assert observed.pending and not observed.is_current
    planned = preview(bundle(), rendered(path), target)
    assert not planned.applicable and planned.error is not None
    assert apply_preview(planned).status is OperationStatus.FAILED
    assert sync(bundle(), rendered(path), target).status is OperationStatus.UNCHANGED
    assert inspect_installation("team", target).is_current
