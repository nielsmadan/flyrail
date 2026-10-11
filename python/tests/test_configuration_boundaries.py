import errno
from collections.abc import Callable
from dataclasses import replace
from pathlib import Path
from typing import Any

import pytest
from state_helpers import index_record, put
from test_configuration_lifecycle import bundle, rendered

from flyrail import (
    Bundle,
    BundleEntry,
    BundleIdentity,
    DirectoryStore,
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
from flyrail._resource_models import Index
from flyrail._state import resource_key
from flyrail.authority import transaction_path
from flyrail.configuration import ResourcePlan
from flyrail.stores import StateStore


@pytest.mark.parametrize(
    "field,value",
    [
        ("resource", []),
        ("receipt", []),
        ("previous", []),
        ("before", []),
        ("require_existing", []),
        ("error", []),
    ],
)
def test_preview_resource_rejects_borrowed_mutable_state(
    tmp_path: Path, field: str, value: Any
) -> None:
    plan = preview(
        bundle(),
        rendered(tmp_path / "file"),
        InstallationTarget(DirectoryStore(tmp_path / "index")),
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
        ("error", []),
    ],
)
def test_preview_rejects_mutable_or_inconsistent_public_inputs(
    tmp_path: Path, field: str, value: Any
) -> None:
    plan = preview(
        bundle(),
        rendered(tmp_path / "file"),
        InstallationTarget(DirectoryStore(tmp_path / "index")),
    )
    with pytest.raises((TypeError, ValueError)):
        replace(plan, **{field: value})


@pytest.mark.parametrize("kind", ["index", "resource", "claim", "result", "resource-result"])
@pytest.mark.parametrize("field", ["error", "shape"])
def test_public_summary_values_reject_mutable_payloads(
    tmp_path: Path, kind: str, field: str
) -> None:
    result = sync(
        bundle(),
        rendered(tmp_path / "file"),
        InstallationTarget(DirectoryStore(tmp_path / "index")),
    )
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
    store = DirectoryStore(tmp_path / "state")
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))
    desired = rendered(tmp_path / "file")
    invalid: Any = []
    calls: dict[str, Callable[[], object]] = {
        "preview": lambda: preview(invalid, desired, target),
        "apply": lambda: apply_preview(invalid),
        "remove": lambda: remove("team", invalid),
        "preview-removal": lambda: preview_removal("team", invalid),
        "inspect": lambda: inspect_installation("team", invalid),
        "sync": lambda: sync(invalid, desired, target),
        "skill-install": lambda: install(invalid, [], store=store),
        "skill-update": lambda: update(invalid, [], store=store),
        "skills": lambda: render_skills(invalid, Target.directory(tmp_path / "skills")),
        "skill-inspect": lambda: inspect(invalid, [], store=store),
    }
    with pytest.raises((TypeError, ValueError)):
        calls[operation]()
    assert list(tmp_path.iterdir()) == []


@pytest.mark.parametrize(
    "seam", ["pending-index", "final-index", "journal-write", "receipt-write", "before-journal"]
)
def test_index_and_metadata_replacement_failures_preserve_membership(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, seam: str
) -> None:
    path = tmp_path / "file"
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))
    if seam != "pending-index":
        assert sync(bundle(), rendered(path), target).status is OperationStatus.APPLIED
    original_write = target.store.write
    count = 0

    def write(key: str, value: bytes, *, expected: bytes | None) -> bool:
        nonlocal count
        if (seam == "journal-write" and key.endswith("/journal")) or (
            seam == "receipt-write" and key.endswith("/receipt")
        ):
            raise OSError(errno.EIO, "injected record store failure")
        if key.startswith("index/"):
            count += 1
            if (seam == "pending-index" and count == 1) or (seam == "final-index" and count == 2):
                raise OSError(errno.EIO, "injected index store failure")
        return original_write(key, value, expected=expected)

    monkeypatch.setattr(target.store, "write", write)
    original_prepare = tx.prepare
    unrecorded: list[Path] = []

    def interrupted(store: StateStore, plan: ResourcePlan) -> tx.Journal:
        if seam == "before-journal":
            directory = transaction_path(plan.resource.destination, plan.receipt.transaction_id)
            state = directory / "staging"
            state.mkdir(parents=True, exist_ok=True)
            unrecorded.append(directory)
            (state / "unknown").write_bytes(b"retained")
            raise OSError(errno.EIO, "interrupted staging")
        return original_prepare(store, plan)

    monkeypatch.setattr(lifecycle, "prepare", interrupted)
    result = sync(bundle("2"), rendered(path, "second"), target)
    assert result.status in {
        OperationStatus.FAILED,
        OperationStatus.PARTIAL,
        OperationStatus.INCOMPLETE,
    }
    if seam == "final-index":
        retained = index_record(target, "team")
        assert result.status is OperationStatus.PARTIAL
        assert isinstance(retained, Index) and retained.pending is not None
    monkeypatch.setattr(target.store, "write", original_write)
    monkeypatch.setattr(lifecycle, "prepare", tx.prepare)
    if seam == "before-journal":
        retry = remove("team", target)
        assert retry.status is OperationStatus.APPLIED
        assert (unrecorded[0] / "staging" / "unknown").read_bytes() == b"retained"
    else:
        retry = remove("team", target)
        assert retry.status in {OperationStatus.APPLIED, OperationStatus.UNCHANGED}
        assert path.exists() is False


def test_schema2_skill_uses_same_authority_and_other_owner_is_protected(tmp_path: Path) -> None:
    store = DirectoryStore(tmp_path / "state")
    tree = TreeContent([BundleEntry("SKILL.md", b"skill")])
    b = Bundle.from_artifacts(
        BundleIdentity("team", "1"), [SkillArtifact("artifact", "example", tree)]
    )
    target = Target.directory(tmp_path / "skills")
    assert install(b, [target], store=store)[0].status is OperationStatus.APPLIED
    assert inspect(b, [target], store=store)[0].observation.is_current
    other = Bundle.from_artifacts(
        BundleIdentity("other", "1"), [SkillArtifact("artifact", "example", tree)]
    )
    assert install(other, [target], store=store)[0].status is OperationStatus.FAILED
    assert inspect(other, [target], store=store)[0].observation.conflicts
    assert uninstall("team", [target], store=store)[0].status is OperationStatus.APPLIED


@pytest.mark.parametrize(
    "target_kind",
    [
        "index-overlap",
        "resource-kind",
        "header-mismatch",
        "missing-index-receipt",
    ],
)
def test_resource_identity_and_reserved_state_checks(tmp_path: Path, target_kind: str) -> None:
    path = tmp_path / "file"
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))
    if target_kind == "index-overlap":
        with pytest.raises(ValueError):
            sync(bundle(), rendered(path), InstallationTarget(DirectoryStore(path / "index")))
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
    destination = ResourceAuthority(path).destination
    if target_kind == "header-mismatch":
        other = (
            preview(
                bundle(),
                rendered(tmp_path / "other"),
                InstallationTarget(DirectoryStore(tmp_path / "other-index")),
            )
            .resources[0]
            .resource
        )
        put(target.store, resource_key(destination, "authority"), encode(other))
    else:
        receipt_key = resource_key(destination, "receipt")
        stored = target.store.read(receipt_key)
        assert stored is not None and target.store.delete(receipt_key, expected=stored)
        index = index_record(target, "team")
        before = path.read_bytes()
        result = remove("team", target)
        assert result.status is OperationStatus.FAILED
        assert result.error is not None and "receipt is missing" in result.error.message
        assert index_record(target, "team") == index
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
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))
    assert sync(bundle(), desired, target).status is OperationStatus.APPLIED
    assert remove("team", target).status is OperationStatus.APPLIED
    assert (path / "foreign").read_bytes() == b"keep"
