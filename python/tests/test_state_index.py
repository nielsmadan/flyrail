from pathlib import Path

import pytest
from skill_helpers import make_bundle
from state_helpers import bundle, rendered

from flyrail import (
    Bundle,
    BundleIdentity,
    DirectoryStore,
    HookRuntime,
    HookShell,
    InstallationTarget,
    InstructionArtifact,
    MemoryStore,
    OperationStatus,
    Platform,
    RenderContext,
    SQLiteStore,
    Target,
    apply_preview,
    inspect,
    inspect_installation,
    install,
    preview,
    remove,
    render,
    sync,
)


def test_installation_target_requires_a_store(tmp_path: Path) -> None:
    with pytest.raises(TypeError, match="store must be a StateStore"):
        InstallationTarget(tmp_path / "index")  # type: ignore[arg-type]


@pytest.mark.integration
def test_one_store_keeps_independent_indexes_per_context(tmp_path: Path) -> None:
    store = MemoryStore()
    skills = make_bundle(tmp_path / "bundle")
    first = install(skills, [Target.directory(tmp_path / "one")], store=store)
    second = install(skills, [Target.directory(tmp_path / "two")], store=store)
    assert [r.status for r in (*first, *second)] == [
        OperationStatus.APPLIED,
        OperationStatus.APPLIED,
    ]
    assert len([key for key in store.keys("index/") if key.startswith(f"index/{skills.id}/")]) == 2


@pytest.mark.integration
def test_a_store_inside_a_managed_destination_is_refused(tmp_path: Path) -> None:
    container = tmp_path / "skills"
    store = DirectoryStore(container / "state")
    results = install(make_bundle(tmp_path / "bundle"), [Target.directory(container)], store=store)
    assert results[0].status is OperationStatus.FAILED
    assert results[0].error is not None and "overlap" in results[0].error.message
    assert list(store.keys("")) == []


def test_skill_functions_reject_a_missing_store(tmp_path: Path) -> None:
    with pytest.raises(TypeError, match="store must be a StateStore"):
        inspect(make_bundle(tmp_path / "bundle"), [Target.directory(tmp_path)], store=None)  # type: ignore[arg-type]


@pytest.mark.parametrize("spelling", ["traversal", "symlink"])
def test_an_aliased_store_location_still_overlaps_a_destination(
    tmp_path: Path, spelling: str
) -> None:
    (tmp_path / "other").mkdir()
    (tmp_path / "link").symlink_to(tmp_path, target_is_directory=True)
    parent = tmp_path / "other" / ".." if spelling == "traversal" else tmp_path / "link"
    store = DirectoryStore(parent / "AGENTS.md" / "state")
    with pytest.raises(ValueError, match="state store overlaps a resource"):
        preview(bundle(), rendered(tmp_path / "AGENTS.md"), InstallationTarget(store))
    assert list(store.keys("")) == []


def test_a_staging_directory_that_overlaps_a_destination_is_refused(tmp_path: Path) -> None:
    store = SQLiteStore(tmp_path / "state.sqlite3")
    destination = tmp_path / "state.sqlite3.staging" / "AGENTS.md"
    with pytest.raises(ValueError, match="state store overlaps a resource"):
        preview(bundle(), rendered(destination), InstallationTarget(store))
    assert sorted(path.name for path in tmp_path.iterdir()) == []


@pytest.mark.integration
def test_preview_approves_parents_created_for_the_store(tmp_path: Path) -> None:
    target = InstallationTarget(DirectoryStore(tmp_path / "project" / "state"))
    plan = preview(bundle(), rendered(tmp_path / "project" / "AGENTS.md"), target)
    assert apply_preview(plan).status is OperationStatus.APPLIED
    assert b"generated" in (tmp_path / "project" / "AGENTS.md").read_bytes()


@pytest.mark.integration
def test_render_contexts_sharing_a_store_keep_separate_installations(tmp_path: Path) -> None:
    store = DirectoryStore(tmp_path / "state")
    project = tmp_path / "project"
    shared = Bundle.from_artifacts(
        BundleIdentity("team", "1"), [InstructionArtifact("guide", "Shared guidance.\n")]
    )
    contexts = [
        RenderContext(Target.project(agent, project), Platform.LINUX)
        for agent in ("claude", "codex")
    ]
    installations = [selected.installation(store) for selected in contexts]
    assert installations[0].context == (
        ("agent", "claude"),
        ("root", str(project)),
        ("scope", "project"),
        ("surface", "cli"),
    )
    assert contexts[0].installation(store, (("app", "notes"),)).context == (("app", "notes"),)
    for selected, installation in zip(contexts, installations, strict=True):
        result = sync(shared, render(shared, selected), installation)
        assert result.status is OperationStatus.APPLIED
    assert b"Shared guidance." in (project / "CLAUDE.md").read_bytes()
    assert b"Shared guidance." in (project / "AGENTS.md").read_bytes()
    assert all(inspect_installation("team", item).is_current for item in installations)


@pytest.mark.integration
def test_default_installation_identity_ignores_volatile_routing_fields(tmp_path: Path) -> None:
    store = DirectoryStore(tmp_path / "state")
    shared = Bundle.from_artifacts(
        BundleIdentity("team", "1"), [InstructionArtifact("guide", "Shared.\n")]
    )
    installed = RenderContext(
        Target.project("codex", tmp_path),
        Platform.MACOS,
        hook_runtime=HookRuntime(None, HookShell.POSIX),
        asset_root=tmp_path / "assets",
    )
    removing = RenderContext(Target.project("codex", tmp_path), Platform.LINUX)
    result = sync(shared, render(shared, installed), installed.installation(store))
    assert result.status is OperationStatus.APPLIED
    assert remove("team", removing.installation(store)).status is OperationStatus.APPLIED
    assert sorted(path.name for path in tmp_path.iterdir()) == ["state"]
