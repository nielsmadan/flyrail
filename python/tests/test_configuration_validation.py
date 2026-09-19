import errno
import json
import os
import stat
import sys
from dataclasses import replace
from pathlib import Path
from typing import Any

import pytest
from skill_helpers import snapshot
from test_configuration_lifecycle import bundle, receipt, rendered

from flyrail import (
    BundleEntry,
    Dependency,
    ErrorCode,
    Family,
    FileContent,
    InstallationTarget,
    Notice,
    NoticeKind,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    ResourceAuthority,
    SectionContent,
    TreeContent,
    apply_preview,
    inspect_installation,
    preview,
    preview_removal,
    remove,
    sync,
)
from flyrail import _lifecycle as lifecycle
from flyrail._codec import decode
from flyrail._resource_io import observe as observe_revision
from flyrail._resource_models import (
    Generation,
    Revision,
)
from flyrail._security import security

pytestmark = pytest.mark.integration


@pytest.mark.parametrize("removal", [False, True])
@pytest.mark.parametrize("damage", ["unknown-entry", "corrupt-index", "observation-io"])
def test_failed_public_previews_retain_typed_error_without_observation_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, removal: bool, damage: str
) -> None:
    path = tmp_path / "AGENTS.md"
    target = InstallationTarget(tmp_path / "index")
    desired, output = bundle(), rendered(path)
    assert sync(desired, output, target).status is OperationStatus.APPLIED
    index_file = target.index_root / "index.json"
    original_index = index_file.read_bytes()
    stray = target.index_root / "unexpected"
    if damage == "unknown-entry":
        stray.write_bytes(b"unrecognized metadata")
    elif damage == "corrupt-index":
        index_file.write_bytes(b"{}")

    def observe(selected: Path) -> Revision:
        if damage == "observation-io" and selected == target.index_root:
            raise OSError(errno.EIO, "index observation unavailable", str(selected))
        return observe_revision(selected)

    before = snapshot(tmp_path)
    with monkeypatch.context() as patch:
        patch.setattr(lifecycle, "observe", observe)
        proposal = (
            preview_removal(desired.id, target) if removal else preview(desired, output, target)
        )
        assert not proposal.applicable
        assert proposal.index_revision is None and proposal.resources == ()
        assert proposal.error is not None
        assert proposal.error.code is (
            ErrorCode.IO_ERROR if damage == "observation-io" else ErrorCode.INVALID_STATE
        )
        result = apply_preview(proposal)
        assert result.status is OperationStatus.FAILED and result.error == proposal.error
        assert snapshot(tmp_path) == before

    if damage == "unknown-entry":
        stray.unlink()
    elif damage == "corrupt-index":
        index_file.write_bytes(original_index)
    repaired = snapshot(tmp_path)
    assert apply_preview(proposal).error == proposal.error
    assert snapshot(tmp_path) == repaired
    fresh = preview_removal(desired.id, target) if removal else preview(desired, output, target)
    assert fresh.applicable and fresh.index_revision is not None
    assert apply_preview(fresh).status is (
        OperationStatus.APPLIED if removal else OperationStatus.UNCHANGED
    )


@pytest.mark.parametrize("removal", [False, True])
@pytest.mark.parametrize(
    "damage", ["symlink", "hardlink", "fifo", "ancestor-symlink", "ancestor-file"]
)
def test_failed_public_previews_preserve_observed_unsafe_destinations(
    tmp_path: Path, removal: bool, damage: str
) -> None:
    parent = tmp_path / "project"
    parent.mkdir()
    path = parent / "AGENTS.md"
    path.write_bytes(b"# foreign\n")
    target = InstallationTarget(tmp_path / "index")
    desired, output = bundle(), rendered(path)
    assert sync(desired, output, target).status is OperationStatus.APPLIED
    assert preview(desired, output, target).applicable
    changed = parent if damage.startswith("ancestor-") else path
    saved = tmp_path / "saved"
    if damage == "hardlink":
        os.link(path, saved)
    else:
        changed.rename(saved)
        if damage in {"symlink", "ancestor-symlink"}:
            changed.symlink_to(saved, target_is_directory=damage == "ancestor-symlink")
        elif damage == "fifo":
            if sys.platform == "win32":
                pytest.skip("Windows has no filesystem FIFO constructor")
            else:
                os.mkfifo(changed)
        else:
            changed.write_bytes(b"ancestor replaced by a file")

    def protected() -> tuple[object, ...]:
        metadata = {}
        for entry in tmp_path.rglob("*"):
            info = entry.lstat()
            metadata[entry] = (
                info.st_mode,
                info.st_ino,
                info.st_nlink,
                info.st_uid,
                info.st_gid,
                entry.readlink() if entry.is_symlink() else None,
                security(entry)
                if stat.S_ISREG(info.st_mode) or stat.S_ISDIR(info.st_mode)
                else None,
            )
        return snapshot(tmp_path), metadata

    before = protected()
    proposal = preview_removal(desired.id, target) if removal else preview(desired, output, target)
    assert not proposal.applicable
    assert proposal.index_revision is None and proposal.resources == ()
    assert proposal.error is not None and proposal.error.code is ErrorCode.UNSAFE_PATH
    assert proposal.error.path == changed
    result = apply_preview(proposal)
    assert result.status is OperationStatus.FAILED and result.error == proposal.error
    assert protected() == before

    if damage == "hardlink":
        saved.unlink()
    else:
        changed.unlink()
        saved.rename(changed)
    repaired = protected()
    assert apply_preview(proposal).error == proposal.error
    assert protected() == repaired
    fresh = preview_removal(desired.id, target) if removal else preview(desired, output, target)
    assert fresh.applicable and fresh.index_revision is not None
    assert apply_preview(fresh).status is (
        OperationStatus.APPLIED if removal else OperationStatus.UNCHANGED
    )


def test_public_previews_raise_for_invalid_caller_inputs(tmp_path: Path) -> None:
    target = InstallationTarget(tmp_path / "index")
    desired, output = bundle(), rendered(tmp_path / "AGENTS.md")
    before = snapshot(tmp_path)
    with pytest.raises(TypeError, match="acquisition"):
        preview(desired, output, target, acquisition="adopt")  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="replacement"):
        preview_removal(desired.id, target, replace_modified=1)  # type: ignore[arg-type]
    with pytest.raises(ValueError, match="bundle id"):
        preview_removal("../invalid", target)
    overlap = InstallationTarget(tmp_path / "AGENTS.md/index")
    with pytest.raises(ValueError, match="overlaps"):
        preview(desired, output, overlap)
    with pytest.raises(ValueError, match="concrete resource"):
        rendered(tmp_path / ".." / "AGENTS.md")
    assert snapshot(tmp_path) == before


@pytest.mark.parametrize(
    "damage",
    [
        "unknown",
        "duplicate",
        "version-bool",
        "bad-id",
        "owner-type",
        "claim-id",
        "unknown-kind",
        "baseline",
        "layout",
        "selection",
        "requirements",
        "overlap",
        "resource",
        "header",
        "hardlink",
    ],
)
def test_corrupt_ownership_cannot_authorize_content_changes(tmp_path: Path, damage: str) -> None:
    path = tmp_path / "AGENTS.md"
    target = InstallationTarget(tmp_path / "index")
    assert sync(bundle(), rendered(path), target).status is OperationStatus.APPLIED
    original = path.read_bytes()
    state = ResourceAuthority(path).state_root
    file = state / "receipt.json"
    raw = json.loads(file.read_bytes())
    fields = raw["fields"]
    owned = fields["claims"][0]["fields"]
    if damage == "unknown":
        fields["future"] = True
    elif damage == "duplicate":
        file.write_bytes(
            file.read_bytes().replace(
                b'"schema_version":2', b'"schema_version":2,"schema_version":2'
            )
        )
    elif damage == "version-bool":
        fields["schema_version"] = True
    elif damage == "bad-id":
        fields["transaction_id"] = "../escape"
    elif damage == "owner-type":
        owned["bundle_id"] = ["team"]
    elif damage == "claim-id":
        owned["claim_id"] = "f" * 64
    elif damage == "unknown-kind":
        owned["claim"]["fields"]["kind"]["value"] = "unknown"
    elif damage == "baseline":
        owned["baseline"] = {"type": "Revision", "fields": {"nodes": []}}
    elif damage == "layout":
        owned["selection"]["fields"]["installed"]["fields"]["layout"] = {"bytes": "ff"}
    elif damage == "selection":
        owned["selection"] = None
    elif damage == "requirements":
        owned["requirements"] = [[{"path": "relative"}, "a" * 64]]
    elif damage == "overlap":
        fields["claims"].append(fields["claims"][0])
    elif damage == "resource":
        fields["resource"]["fields"]["state_root"] = {"path": str(tmp_path / "redirect")}
    elif damage == "header":
        (state / "authority.json").write_bytes(b"{}")
    else:
        import os

        os.link(file, tmp_path / "receipt-link")
    if damage not in {"duplicate", "header", "hardlink"}:
        file.write_text(json.dumps(raw))
    result = sync(bundle("2"), rendered(path, "changed"), target)
    assert result.status is OperationStatus.FAILED
    assert result.error is not None
    assert path.read_bytes() == original
    assert remove("team", target).status is OperationStatus.FAILED


@pytest.mark.parametrize(
    "damage",
    [
        "unknown",
        "context",
        "owner",
        "schema",
        "generation",
        "duplicate-resource",
        "kind-collision",
        "next-corrupt",
    ],
)
def test_corrupt_index_is_bookkeeping_not_ownership_authority(tmp_path: Path, damage: str) -> None:
    path = tmp_path / "AGENTS.md"
    target = InstallationTarget(tmp_path / "index")
    assert sync(bundle(), rendered(path), target).status is OperationStatus.APPLIED
    before = path.read_bytes()
    file = target.index_root / "index.json"
    raw = json.loads(file.read_bytes())
    fields = raw["fields"]
    if damage == "unknown":
        fields["extra"] = 1
    elif damage == "context":
        fields["context_digest"] = "f" * 64
    elif damage == "owner":
        fields["bundle_id"] = "other"
    elif damage == "schema":
        fields["schema_version"] = 1
    elif damage == "generation":
        fields["current"]["fields"]["bundle_digest"] = "invalid"
    elif damage == "duplicate-resource":
        fields["current"]["fields"]["membership"] *= 2
    elif damage == "kind-collision":
        ref = json.loads(json.dumps(fields["current"]["fields"]["membership"][0][0]))
        ref["fields"]["kind"]["value"] = "tree"
        fields["residual"] = [ref]
    else:
        (target.index_root / "index.json.next").write_bytes(b"{}")
    file.write_text(json.dumps(raw))
    result = sync(bundle("2"), rendered(path, "changed"), target)
    assert result.status is OperationStatus.FAILED and path.read_bytes() == before
    assert inspect_installation("team", target).error is not None


def test_physical_claim_aliases_deduplicate_and_incompatible_aliases_fail_preflight(
    tmp_path: Path,
) -> None:
    path = tmp_path / "AGENTS.md"
    target = InstallationTarget(tmp_path / "index")
    first = rendered(path).artifacts[0]
    selected = RenderedBundle([first, replace(first, id="second")])
    assert sync(bundle(), selected, target).status is OperationStatus.APPLIED
    assert len(receipt(path).claims) == 1
    conflict = RenderedBundle(
        [first, replace(first, id="second", content=SectionContent("guide", "different"))]
    )
    original = path.read_bytes()
    assert sync(bundle("2"), conflict, target).status is OperationStatus.FAILED
    assert path.read_bytes() == original


def test_resource_dependency_cycle_is_rejected_after_grouping(tmp_path: Path) -> None:
    artifacts = [
        RenderedArtifact(name, Family.INSTRUCTIONS, tmp_path / path, SectionContent(name, "text"))
        for name, path in [("a", "one"), ("b", "two"), ("c", "one")]
    ]
    rendered_bundle = RenderedBundle(artifacts, [Dependency("a", "b"), Dependency("b", "c")])
    result = sync(bundle(), rendered_bundle, InstallationTarget(tmp_path / "index"))
    assert result.status is OperationStatus.FAILED
    assert (tmp_path / "one").exists() is False and (tmp_path / "two").exists() is False


def test_old_referenced_asset_requires_a_new_revision_destination(tmp_path: Path) -> None:
    target = InstallationTarget(tmp_path / "index")
    path = tmp_path / "AGENTS.md"
    asset = tmp_path / "hook"
    first = RenderedBundle(
        [
            *rendered(path).artifacts,
            RenderedArtifact("asset", Family.HOOKS, asset, FileContent(b"v1")),
        ],
        [Dependency("guide", "asset")],
    )
    assert sync(bundle(), first, target).status is OperationStatus.APPLIED
    second = RenderedBundle(
        [
            *rendered(path, "next").artifacts,
            RenderedArtifact("asset", Family.HOOKS, asset, FileContent(b"v2")),
        ],
        first.dependencies,
    )
    assert sync(bundle("2"), second, target).status is OperationStatus.FAILED
    assert asset.read_bytes() == b"v1"


@pytest.mark.parametrize(
    "field,value",
    [
        ("replace_modified", 1),
        ("acquisition", "adopt"),
        ("lock_timeout", True),
        ("lock_timeout", -1),
        ("lock_timeout", float("nan")),
    ],
)
def test_invalid_public_policies_fail_before_any_write(
    tmp_path: Path, field: str, value: Any
) -> None:
    target = InstallationTarget(tmp_path / "index")
    path = tmp_path / "AGENTS.md"
    with pytest.raises((TypeError, ValueError)):
        sync(bundle(), rendered(path), target, **{field: value})
    assert list(tmp_path.iterdir()) == []


def test_caller_iterables_and_nested_pairs_are_detached(tmp_path: Path) -> None:
    pairs = [["surface", "application"]]
    target = InstallationTarget(tmp_path / "index", pairs)  # type: ignore[arg-type]
    pairs[0][1] = "changed"
    assert target.context == (("surface", "application"),)
    content = [BundleEntry("file", b"data")]
    tree = TreeContent(content)
    content.clear()
    assert tree.entries[0].data == b"data"
    plan = preview(bundle(), rendered(tmp_path / "AGENTS.md"), target)
    resources = list(plan.resources)
    ancestors = list(plan.index_ancestors)
    frozen = replace(plan, resources=resources, index_ancestors=ancestors)  # type: ignore[arg-type]
    resources.clear()
    ancestors.clear()
    assert frozen.resources and frozen.index_ancestors
    membership = [[plan.resources[0].resource, [plan.resources[0].receipt.claims[0].claim_id]]]
    generation = Generation("1", "a" * 64, "b" * 64, membership)  # type: ignore[arg-type]
    membership[0][1].clear()  # type: ignore[attr-defined]
    assert generation.membership[0][1]
    requirements = [[tmp_path / "required", "a" * 64]]
    owned = replace(plan.resources[0].receipt.claims[0], requirements=requirements)  # type: ignore[arg-type]
    requirements[0][1] = "changed"
    assert owned.requirements[0][1] == "a" * 64


@pytest.mark.parametrize(
    "raw",
    [
        b'{"extra":1}',
        b'{"bytes":"AA"}',
        b'{"path":"relative"}',
        b'{"type":"Unknown","fields":{}}',
        b'{"enum":"Claim","value":false}',
        b"[1.2]",
        b'{"type":"Key","fields":{}}',
        b'{"value":["integer","01"]}',
        b'{"type":"Node","fields":[]}',
    ],
)
def test_strict_state_codec_rejects_ambiguous_or_unknown_records(raw: bytes) -> None:
    with pytest.raises((ValueError, TypeError)):
        decode(raw)


def test_unsupported_render_notice_blocks_complete_target(tmp_path: Path) -> None:
    path = tmp_path / "AGENTS.md"
    desired = RenderedBundle(
        rendered(path).artifacts,
        notices=[Notice("guide", NoticeKind.UNSUPPORTED, "unsupported event")],
    )
    result = sync(bundle(), desired, InstallationTarget(tmp_path / "index"))
    assert result.status is OperationStatus.FAILED and path.exists() is False
