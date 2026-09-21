import shutil
from pathlib import Path

import pytest

from flyrail import (
    Agent,
    Bundle,
    BundleEntry,
    BundleIdentity,
    ChangeAction,
    Command,
    ErrorCode,
    Family,
    InstallationTarget,
    InstructionArtifact,
    McpArtifact,
    NoticeKind,
    PlannedChange,
    PlanSummary,
    Platform,
    RenderContext,
    RenderedArtifact,
    RenderedBundle,
    ResourceKind,
    SkillArtifact,
    Surface,
    Target,
    TargetScope,
    TreeContent,
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


def skill(name: str) -> SkillArtifact:
    return SkillArtifact(name, name, TreeContent([BundleEntry("SKILL.md", f"{name}\n".encode())]))


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
    assert change.families == (Family.INSTRUCTIONS,)
    assert change.artifact_ids == ("guide",)
    assert change.destination == tmp_path / "AGENTS.md"
    assert change.error is None
    assert change.recovery_paths == ()


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
    assert change.families == ()
    assert change.artifact_ids == ()
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
    assert summary.error is None
    (change,) = summary.changes
    assert change.action is ChangeAction.CONFLICT
    assert change.error is not None
    assert change.error.code is ErrorCode.CONFLICT
    assert change.error.message == "resource must already exist"
    assert change.error.path == tmp_path / "AGENTS.md"
    assert change.recovery_paths == ()


def test_reports_recovery_paths_as_a_conflict(tmp_path: Path) -> None:
    bundle = bundle_of()
    selected = context(tmp_path)
    rendered = render(bundle, selected)
    target = selected.installation(tmp_path / "index")
    sync(bundle, rendered, target)
    state = preview(bundle, rendered, target).resources[0].resource.state_root
    (state / "staging" / "leftover").write_text("interrupted\n")

    summary = summarize(preview(bundle, rendered, target))

    assert not summary.applicable
    assert summary.error is None
    (change,) = summary.changes
    assert change.action is ChangeAction.CONFLICT
    assert change.error is None
    assert change.recovery_paths == (state / "staging",)


def test_reports_a_target_error(tmp_path: Path) -> None:
    bundle = bundle_of()
    selected = context(tmp_path)
    rendered = render(bundle, selected)
    target = selected.installation(tmp_path / "index")
    sync(bundle, rendered, target)
    index = tmp_path / "index" / "index.json"
    shutil.copyfile(index, index.with_suffix(".json.next"))

    summary = summarize(preview(bundle, rendered, target))

    assert not summary.applicable
    assert summary.error is not None
    assert summary.error.code is ErrorCode.RECOVERY_NEEDED
    assert summary.error.message == "installation index has prepared metadata"
    assert summary.error.path == index.with_suffix(".json.next")


def test_forwards_render_notices(tmp_path: Path) -> None:
    bundle = Bundle.from_artifacts(
        BundleIdentity("notice-demo", "one"),
        [
            InstructionArtifact("guide", "Run the checks.\n"),
            McpArtifact("tools", "tools", Command(["project-server"])),
        ],
    )
    selected = RenderContext(Target.project("pi", tmp_path), Platform.LINUX)
    rendered = render(bundle, selected)

    summary = summarize(preview(bundle, rendered, selected.installation(tmp_path / "index")))

    assert [(notice.artifact_id, notice.kind, notice.code) for notice in summary.notices] == [
        ("guide", NoticeKind.ACTIVATION, "instruction-loading"),
        ("tools", NoticeKind.ACTIVATION, "mcp-activation"),
        ("tools", NoticeKind.PREREQUISITE, "pi-mcp-adapter-v2-32-1"),
    ]
    assert summary.notices[2].message == (
        "Requires externally installed pi-mcp-adapter; emitted schema targets v2.32.1."
    )
    assert summary.notices == rendered.notices


def test_reports_every_mcp_artifact_sharing_a_destination(tmp_path: Path) -> None:
    bundle = Bundle.from_artifacts(
        BundleIdentity("multi-mcp", "one"),
        [
            McpArtifact("m1", "m1", Command(["s1"])),
            McpArtifact("m2", "m2", Command(["s2"])),
        ],
    )
    selected = RenderContext(Target.project("claude", tmp_path), Platform.LINUX)
    rendered = render(bundle, selected)

    summary = summarize(preview(bundle, rendered, selected.installation(tmp_path / "index")))

    (change,) = summary.changes
    assert change.artifact_ids == ("m1", "m2")
    assert change.families == (Family.MCP, Family.MCP)
    assert change.kind is ResourceKind.DOCUMENT
    assert change.destination == tmp_path / ".mcp.json"
    assert change.action is ChangeAction.CREATE


def test_reports_every_skill_sharing_a_container(tmp_path: Path) -> None:
    bundle = Bundle.from_artifacts(
        BundleIdentity("multi-skill", "one"), [skill("review"), skill("deploy")]
    )
    selected = RenderContext(Target.project("claude", tmp_path), Platform.LINUX)
    rendered = render(bundle, selected)

    summary = summarize(preview(bundle, rendered, selected.installation(tmp_path / "index")))

    (change,) = summary.changes
    assert change.artifact_ids == ("deploy", "review")
    assert change.families == (Family.SKILLS, Family.SKILLS)
    assert change.kind is ResourceKind.SKILL_CONTAINER
    assert change.destination == tmp_path / ".claude" / "skills"


def test_reports_every_instruction_sharing_a_document(tmp_path: Path) -> None:
    bundle = Bundle.from_artifacts(
        BundleIdentity("multi-instruction", "one"),
        [InstructionArtifact("second", "Second.\n"), InstructionArtifact("first", "First.\n")],
    )
    selected = context(tmp_path)
    rendered = render(bundle, selected)

    summary = summarize(preview(bundle, rendered, selected.installation(tmp_path / "index")))

    (change,) = summary.changes
    assert change.artifact_ids == ("first", "second")
    assert change.families == (Family.INSTRUCTIONS, Family.INSTRUCTIONS)
    assert change.destination == tmp_path / "AGENTS.md"


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


def test_planned_change_rejects_mismatched_artifact_metadata(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="artifact ids and families must agree"):
        PlannedChange(
            ("guide", "other"),
            (Family.INSTRUCTIONS,),
            ResourceKind.DOCUMENT,
            tmp_path / "AGENTS.md",
            ChangeAction.CREATE,
        )


def test_planned_change_rejects_other_values(tmp_path: Path) -> None:
    with pytest.raises(TypeError, match="ResourceKind and ChangeAction"):
        PlannedChange((), (), "document", tmp_path / "AGENTS.md", ChangeAction.CREATE)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="expected str values"):
        PlannedChange(
            (Family.MCP,),
            (Family.MCP,),
            ResourceKind.DOCUMENT,
            tmp_path / "AGENTS.md",
            ChangeAction.CREATE,
        )
    with pytest.raises(TypeError, match="families must be Family values"):
        PlannedChange(
            ("tools",),
            ("mcp",),  # type: ignore[arg-type]
            ResourceKind.DOCUMENT,
            tmp_path / "AGENTS.md",
            ChangeAction.CREATE,
        )
    with pytest.raises(TypeError, match="error must be a TargetError"):
        PlannedChange(
            (),
            (),
            ResourceKind.DOCUMENT,
            tmp_path / "AGENTS.md",
            ChangeAction.CONFLICT,
            "broken",  # type: ignore[arg-type]
        )
    with pytest.raises(ValueError, match="absolute Path"):
        PlannedChange((), (), ResourceKind.DOCUMENT, Path("AGENTS.md"), ChangeAction.CREATE)
    with pytest.raises(ValueError, match="absolute"):
        PlannedChange(
            (),
            (),
            ResourceKind.DOCUMENT,
            tmp_path / "AGENTS.md",
            ChangeAction.CONFLICT,
            None,
            (Path("staging"),),
        )


def test_plan_summary_rejects_other_values(tmp_path: Path) -> None:
    change = PlannedChange(
        ("guide",),
        (Family.INSTRUCTIONS,),
        ResourceKind.DOCUMENT,
        tmp_path / "AGENTS.md",
        ChangeAction.CREATE,
    )
    with pytest.raises(TypeError, match="routing"):
        PlanSummary("codex", None, None, True, (change,), (), None)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="routing"):
        PlanSummary(Agent.CODEX, "project", None, True, (change,), (), None)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="routing"):
        PlanSummary(Agent.CODEX, TargetScope.PROJECT, "cli", True, (change,), (), None)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="applicability"):
        PlanSummary(None, None, None, 1, (change,), (), None)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="PlannedChange"):
        PlanSummary(None, None, None, True, ("change",), (), None)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="expected Notice values"):
        PlanSummary(None, None, None, True, (change,), ("notice",), None)  # type: ignore[arg-type]
    with pytest.raises(TypeError, match="error must be a TargetError"):
        PlanSummary(None, None, None, True, (change,), (), "broken")  # type: ignore[arg-type]
