import os
import shutil
from dataclasses import asdict
from pathlib import Path

import pytest
from skill_helpers import make_bundle, snapshot

from flyrail import (
    Acquisition,
    Bundle,
    BundleEntry,
    BundleIdentity,
    DocumentFormat,
    ErrorCode,
    Family,
    FileContent,
    FileMode,
    InstallationTarget,
    InstructionArtifact,
    Key,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    ResourceAuthority,
    SectionContent,
    Selector,
    StructuredContent,
    Target,
    TreeContent,
    apply_preview,
    freeze_value,
    inspect,
    inspect_installation,
    install,
    preview,
    remove,
    sync,
    uninstall,
    update,
)
from flyrail._codec import decode
from flyrail._resource_models import Receipt

pytestmark = pytest.mark.integration


def bundle(version: str = "1", identifier: str = "team") -> Bundle:
    return Bundle.from_artifacts(
        BundleIdentity(identifier, version), [InstructionArtifact("guide", "text")]
    )


def rendered(
    path: Path, text: str = "generated", artifact_id: str = "guide", *, existing: bool = False
) -> RenderedBundle:
    return RenderedBundle(
        [
            RenderedArtifact(
                artifact_id,
                Family.INSTRUCTIONS,
                path,
                SectionContent("guide", text),
                require_existing=existing,
            )
        ]
    )


def receipt(path: Path) -> Receipt:
    value = decode((ResourceAuthority(path).state_root / "receipt.json").read_bytes())
    assert isinstance(value, Receipt)
    return value


def test_generated_preview_is_readonly_and_owns_only_section(tmp_path: Path) -> None:
    destination = tmp_path / "AGENTS.md"
    destination.write_bytes(b"# Foreign\ncredential=synthetic-private-value\n")
    destination.chmod(0o600)
    target = InstallationTarget(tmp_path / "index", [("surface", "application")])
    original = snapshot(tmp_path)
    plan = preview(bundle(), rendered(destination), target)
    assert plan.applicable and snapshot(tmp_path) == original
    assert plan.resources[0].before.data == destination.read_bytes()
    assert plan.resources[0].after.data is not None
    result = apply_preview(plan)
    assert result.status is OperationStatus.APPLIED
    assert result.observation.is_current
    assert "synthetic-private-value" not in repr(asdict(result))
    assert sync(bundle(), rendered(destination), target).status is OperationStatus.UNCHANGED
    assert remove("team", target).status is OperationStatus.APPLIED
    assert destination.read_bytes() == b"# Foreign\ncredential=synthetic-private-value\n"
    if os.name != "nt":
        assert destination.stat().st_mode & 0o777 == 0o600
    assert remove("team", target).status is OperationStatus.UNCHANGED


@pytest.mark.parametrize("format", list(DocumentFormat))
def test_multi_owner_structured_lifecycle_and_creation_provenance(
    tmp_path: Path, format: DocumentFormat
) -> None:
    path = tmp_path / f"config.{format}"
    targets = [InstallationTarget(tmp_path / name) for name in ("index-a", "index-b")]
    bundles = [bundle(identifier=name) for name in ("a", "b")]
    renders = [
        RenderedBundle(
            [
                RenderedArtifact(
                    name,
                    Family.MCP,
                    path,
                    StructuredContent(
                        format, Selector([Key("mcp"), Key(name)]), freeze_value({"command": name})
                    ),
                )
            ]
        )
        for name in ("a", "b")
    ]
    assert sync(bundles[0], renders[0], targets[0]).status is OperationStatus.APPLIED
    assert sync(bundles[1], renders[1], targets[1]).status is OperationStatus.APPLIED
    assert len(receipt(path).claims) == 2
    assert remove("a", targets[0]).status is OperationStatus.APPLIED
    assert inspect_installation("b", targets[1]).is_current
    assert remove("b", targets[1]).status is OperationStatus.APPLIED
    assert path.exists() is False
    assert ResourceAuthority(path).lock_path.is_file()


@pytest.mark.parametrize("acquisition", [Acquisition.TAKEOVER, Acquisition.ADOPT])
def test_acquisition_rename_updates_and_first_baseline(
    tmp_path: Path, acquisition: Acquisition
) -> None:
    path = tmp_path / "AGENTS.md"
    original = b"# Project\n\n<!-- flyrail:guide:start -->\noriginal\n<!-- flyrail:guide:end -->\n"
    path.write_bytes(original)
    target = InstallationTarget(tmp_path / "index")
    assert sync(bundle(), rendered(path, "original"), target).status is OperationStatus.FAILED
    assert (
        sync(bundle(), rendered(path, "original"), target, acquisition=acquisition).status
        is OperationStatus.APPLIED
    )
    first = receipt(path).claims[0]
    assert first.selection is not None
    for version in ["2", "3"]:
        assert (
            sync(bundle(version), rendered(path, version, "renamed"), target).status
            is OperationStatus.APPLIED
        )
        owned = receipt(path).claims[0]
        assert owned.artifact_id == "renamed"
        assert owned.selection is not None and owned.selection.baseline == first.selection.baseline
    assert remove("team", target).status is OperationStatus.APPLIED
    assert path.read_bytes() == (
        original if acquisition is Acquisition.TAKEOVER else b"# Project\n\n"
    )


def test_replacement_preserves_original_takeover_and_current_chmod(tmp_path: Path) -> None:
    path = tmp_path / "AGENTS.md"
    original = b"<!-- flyrail:guide:start -->\noriginal\n<!-- flyrail:guide:end -->\n"
    path.write_bytes(original)
    target = InstallationTarget(tmp_path / "index")
    assert (
        sync(bundle(), rendered(path), target, acquisition=Acquisition.TAKEOVER).status
        is OperationStatus.APPLIED
    )
    path.write_bytes(path.read_bytes().replace(b"generated", b"user edit"))
    path.chmod(0o600)
    assert sync(bundle("2"), rendered(path, "second"), target).error.code is ErrorCode.MODIFIED  # type: ignore[union-attr]
    assert (
        sync(bundle("2"), rendered(path, "second"), target, replace_modified=True).status
        is OperationStatus.APPLIED
    )
    assert remove("team", target).status is OperationStatus.APPLIED
    assert path.read_bytes() == original
    if os.name != "nt":
        assert path.stat().st_mode & 0o777 == 0o600


@pytest.mark.parametrize("mutation", ["modified", "missing"])
def test_modified_retirement_keeps_residual_across_desired_generations(
    tmp_path: Path, mutation: str
) -> None:
    old, new = tmp_path / "old.md", tmp_path / "new.md"
    target = InstallationTarget(tmp_path / "index")
    assert sync(bundle(), rendered(old), target).status is OperationStatus.APPLIED
    if mutation == "missing":
        old.unlink()
    else:
        old.write_bytes(old.read_bytes().replace(b"generated", b"edited"))
    for version in ["2", "3"]:
        result = sync(bundle(version), rendered(new), target)
        assert result.status is OperationStatus.FAILED
        assert new.exists() is False
    status = inspect_installation("team", target)
    assert len(status.resources) == 1 and status.resources[0].claims[0].status == "modified"
    assert remove("team", target).status is OperationStatus.FAILED
    assert len(receipt(old).claims) == 1


def test_retarget_takes_fresh_baseline_and_restores_old(tmp_path: Path) -> None:
    a, b = tmp_path / "a.md", tmp_path / "b.md"
    first = b"<!-- flyrail:guide:start -->\nfirst\n<!-- flyrail:guide:end -->\n"
    second = first.replace(b"first", b"second")
    a.write_bytes(first)
    b.write_bytes(second)
    target = InstallationTarget(tmp_path / "index")
    assert (
        sync(bundle(), rendered(a), target, acquisition=Acquisition.TAKEOVER).status
        is OperationStatus.APPLIED
    )
    assert (
        sync(bundle("2"), rendered(b), target, acquisition=Acquisition.TAKEOVER).status
        is OperationStatus.APPLIED
    )
    assert a.read_bytes() == first
    assert remove("team", target).status is OperationStatus.APPLIED
    assert b.read_bytes() == second


def test_required_existing_is_not_a_caller_exists_race(tmp_path: Path) -> None:
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"foreign")
    target = InstallationTarget(tmp_path / "index")
    desired = rendered(path, existing=True)
    path.unlink()
    plan = preview(bundle(), desired, target)
    assert plan.applicable is False
    assert apply_preview(plan).status is OperationStatus.FAILED
    assert path.exists() is False
    path.write_bytes(b"foreign")
    plan = preview(bundle(), desired, target)
    path.unlink()
    assert apply_preview(plan).error.code is ErrorCode.CONCURRENT_CHANGE  # type: ignore[union-attr]
    assert path.exists() is False


@pytest.mark.parametrize("kind", ["file", "tree", "subtree"])
@pytest.mark.parametrize("acquisition", list(Acquisition))
def test_whole_content_takeover_adoption_and_removal(
    tmp_path: Path, kind: str, acquisition: Acquisition
) -> None:
    path = tmp_path / "resource"
    content = (
        FileContent(b"new", FileMode.READABLE)
        if kind == "file"
        else TreeContent([BundleEntry("run", b"new", True), BundleEntry("empty")])
    )
    if kind == "file":
        path.write_bytes(b"new" if acquisition is Acquisition.ADOPT else b"old")
        path.chmod(0o644)
    else:
        owned = path / "skill" if kind == "subtree" else path
        owned.mkdir(parents=True)
        (owned / "run").write_bytes(b"new" if acquisition is Acquisition.ADOPT else b"old")
        (owned / "run").chmod(0o755)
        (owned / "empty").mkdir()
    desired = RenderedBundle(
        [
            RenderedArtifact(
                "asset",
                Family.SKILLS,
                path,
                content,
                subtree="skill" if kind == "subtree" else None,
            )
        ]
    )
    target = InstallationTarget(tmp_path / "index")
    before = path.read_bytes() if kind == "file" else (owned / "run").read_bytes()
    result = sync(bundle(), desired, target, acquisition=acquisition)
    assert result.status is (
        OperationStatus.FAILED if acquisition is Acquisition.CONFLICT else OperationStatus.APPLIED
    )
    if acquisition is Acquisition.CONFLICT:
        return
    assert remove("team", target).status is OperationStatus.APPLIED
    if acquisition is Acquisition.TAKEOVER:
        assert (path.read_bytes() if kind == "file" else (owned / "run").read_bytes()) == before
    else:
        assert (path if kind != "subtree" else owned).exists() is False


def test_skill_adapter_snapshots_modes_aliases_hashes_and_source_free_removal(
    tmp_path: Path,
) -> None:
    first = make_bundle(tmp_path / "source", executable=True)
    target = Target.directory(tmp_path / "skills")
    assert inspect(first, [target])[0].observation.state == "absent"
    results = install(first, [target, target])
    assert [r.status for r in results] == [OperationStatus.APPLIED] * 2
    assert results[1].alias_of == 0
    shutil.rmtree(tmp_path / "source")
    assert inspect(first, [target])[0].observation.is_current
    second = make_bundle(tmp_path / "new-source", version="2", data=b"changed", executable=True)
    assert install(second, [target])[0].error.code is ErrorCode.UPDATE_REQUIRED  # type: ignore[union-attr]
    assert update(second, [target])[0].status is OperationStatus.APPLIED
    (target.root / "review" / "SKILL.md").write_bytes(b"user")
    assert uninstall("team", [target])[0].status is OperationStatus.FAILED
    assert update(second, [target], replace_modified=True)[0].status is OperationStatus.APPLIED
    assert uninstall("team", [target])[0].status is OperationStatus.APPLIED
    assert (target.root / "review").exists() is False


def test_bundle_inside_project_can_edit_other_file_but_not_source(tmp_path: Path) -> None:
    selected = make_bundle(tmp_path / "project" / "bundle")
    target = InstallationTarget(tmp_path / "index")
    assert (
        sync(selected, rendered(tmp_path / "project" / "AGENTS.md"), target).status
        is OperationStatus.APPLIED
    )
    with pytest.raises(ValueError, match="source"):
        sync(selected, rendered(tmp_path / "project" / "bundle" / "AGENTS.md"), target)


def test_complete_conflict_preflight_keeps_other_resource_unpublished(tmp_path: Path) -> None:
    a, b = tmp_path / "a", tmp_path / "b"
    b.write_bytes(b"foreign")
    desired = RenderedBundle(
        [
            RenderedArtifact("a", Family.INSTRUCTIONS, a, FileContent(b"new")),
            RenderedArtifact("b", Family.INSTRUCTIONS, b, FileContent(b"new")),
        ]
    )
    result = sync(bundle(), desired, InstallationTarget(tmp_path / "index"))
    assert result.status is OperationStatus.FAILED and a.exists() is False
    assert b.read_bytes() == b"foreign"
