from pathlib import Path

import pytest

from flyrail import (
    Agent,
    Bundle,
    BundleIdentity,
    ChangeAction,
    Family,
    InstallationTarget,
    InstructionArtifact,
    Platform,
    RenderContext,
    RenderedArtifact,
    RenderedBundle,
    Surface,
    Target,
    TargetScope,
    preview,
    preview_removal,
    render,
    summarize,
    sync,
)


def bundle_of(text: str = "Run the checks.\n") -> Bundle:
    return Bundle.from_artifacts(
        BundleIdentity("summary-demo", "one"), [InstructionArtifact("guide", text)]
    )


def context(root: Path) -> RenderContext:
    return RenderContext(Target.project("codex", root), Platform.LINUX)


def test_summarizes_a_creation_plan(tmp_path: Path) -> None:
    bundle = bundle_of()
    selected = context(tmp_path)
    rendered = render(bundle, selected)
    target = selected.installation(tmp_path / "index")

    summary = summarize(preview(bundle, rendered, target))

    assert summary.agent is Agent.CODEX
    assert summary.scope is TargetScope.PROJECT
    assert summary.surface is Surface.CLI
    assert summary.applicable
    assert summary.error is None
    (change,) = summary.changes
    assert change.action is ChangeAction.CREATE
    assert change.family is Family.INSTRUCTIONS
    assert change.artifact_id == "guide"
    assert change.destination == tmp_path / "AGENTS.md"


def test_summarizes_an_unchanged_plan(tmp_path: Path) -> None:
    bundle = bundle_of()
    selected = context(tmp_path)
    rendered = render(bundle, selected)
    target = selected.installation(tmp_path / "index")
    sync(bundle, rendered, target)

    summary = summarize(preview(bundle, rendered, target))

    assert [change.action for change in summary.changes] == [ChangeAction.UNCHANGED]


def test_summarizes_an_update_plan(tmp_path: Path) -> None:
    selected = context(tmp_path)
    first = bundle_of()
    target = selected.installation(tmp_path / "index")
    sync(first, render(first, selected), target)
    second = bundle_of("Run the checks twice.\n")

    summary = summarize(preview(second, render(second, selected), target))

    assert [change.action for change in summary.changes] == [ChangeAction.UPDATE]


def test_summarizes_a_removal_plan(tmp_path: Path) -> None:
    bundle = bundle_of()
    selected = context(tmp_path)
    target = selected.installation(tmp_path / "index")
    sync(bundle, render(bundle, selected), target)

    summary = summarize(preview_removal(bundle.id, target))

    (change,) = summary.changes
    assert change.action is ChangeAction.DELETE
    assert change.family is None
    assert change.artifact_id is None
    assert change.destination == tmp_path / "AGENTS.md"
    assert summary.notices == ()


def test_summarizes_a_conflicting_plan(tmp_path: Path) -> None:
    bundle = bundle_of()
    selected = context(tmp_path)
    base = render(bundle, selected)
    artifact = base.artifacts[0]
    forced = RenderedBundle(
        [
            RenderedArtifact(
                artifact.id,
                artifact.family,
                artifact.destination,
                artifact.content,
                require_existing=True,
            )
        ],
        notices=base.notices,
        routing_context=base.routing_context,
    )

    summary = summarize(preview(bundle, forced, selected.installation(tmp_path / "index")))

    assert not summary.applicable
    (change,) = summary.changes
    assert change.action is ChangeAction.CONFLICT


def test_directory_targets_have_no_agent(tmp_path: Path) -> None:
    bundle = bundle_of()
    selected = context(tmp_path)
    rendered = render(bundle, selected)

    summary = summarize(preview(bundle, rendered, InstallationTarget(tmp_path / "index")))

    assert summary.agent is None
    assert summary.scope is None
    assert summary.surface is None


def test_summarize_rejects_other_values() -> None:
    with pytest.raises(TypeError, match="LifecyclePreview"):
        summarize(object())  # type: ignore[arg-type]
