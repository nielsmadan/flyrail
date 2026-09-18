from dataclasses import replace
from pathlib import Path
from typing import Any

import pytest
from skill_helpers import snapshot
from test_configuration_lifecycle import bundle, receipt, rendered

from flyrail import (
    Acquisition,
    Bundle,
    DocumentFormat,
    EditStatus,
    ErrorCode,
    Family,
    InstallationTarget,
    InstructionArtifact,
    Key,
    Member,
    Notice,
    NoticeKind,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    Scalar,
    Selector,
    StructuredContent,
    TargetError,
    freeze_value,
    inspect_installation,
    preview,
    preview_removal,
    remove,
    sync,
)


def test_intact_array_member_is_current_without_restoration_anchors(tmp_path: Path) -> None:
    path = tmp_path / "shared.json"
    path.write_bytes(b'{"items":[{"id":"a"},{"id":"x","v":1},{"id":"b"}]}')
    target = InstallationTarget(tmp_path / "index")
    desired = bundle()
    output = RenderedBundle(
        [
            RenderedArtifact(
                "guide",
                Family.INSTRUCTIONS,
                path,
                StructuredContent(
                    DocumentFormat.JSON,
                    Selector([Key("items"), Member(Scalar("x"), ("id",))]),
                    freeze_value({"id": "x", "v": 2}),
                ),
            ),
        ]
    )
    assert (
        sync(desired, output, target, acquisition=Acquisition.TAKEOVER).status
        is OperationStatus.APPLIED
    )
    original = receipt(path).claims[0].selection
    assert original is not None and original.baseline is not None
    path.write_bytes(path.read_bytes().replace(b'"a"', b'"c"').replace(b'"b"', b'"d"'))
    before = snapshot(tmp_path)
    observed = inspect_installation(desired.id, target)
    assert observed.is_current and observed.matches(desired, output)
    assert observed.resources[0].claims[0].status is EditStatus.CURRENT
    assert preview(desired, output, target).applicable
    assert snapshot(tmp_path) == before

    refreshed = sync(desired, output, target)
    assert refreshed.status is OperationStatus.APPLIED and refreshed.observation.is_current
    stable = snapshot(tmp_path)
    unchanged = sync(desired, output, target)
    assert unchanged.status is OperationStatus.UNCHANGED
    assert unchanged.observation.matches(desired, output)
    retirement = preview_removal(desired.id, target)
    assert not retirement.applicable
    assert retirement.resources[0].error is not None
    assert "no surviving anchors" in retirement.resources[0].error.message
    assert remove(desired.id, target).status is OperationStatus.FAILED
    assert snapshot(tmp_path) == stable
    retained = receipt(path).claims[0].selection
    assert retained is not None and retained.baseline == original.baseline


@pytest.mark.parametrize(
    "change", ["none", "id", "version", "content", "render", "modified", "unsupported"]
)
def test_observation_compares_the_desired_generation(tmp_path: Path, change: str) -> None:
    target = InstallationTarget(tmp_path / "index")
    path = tmp_path / "AGENTS.md"
    desired, output = bundle(), rendered(path)
    assert sync(desired, output, target).status is OperationStatus.APPLIED
    if change == "id":
        desired = bundle(identifier="other")
    elif change == "version":
        desired = bundle(version="next")
    elif change == "content":
        desired = Bundle.from_artifacts(desired.identity, [InstructionArtifact("guide", "new")])
    elif change == "render":
        output = rendered(path, "changed generated guidance")
    elif change == "modified":
        path.write_bytes(path.read_bytes().replace(b"generated", b"user edit"))
    elif change == "unsupported":
        output = RenderedBundle(
            output.artifacts, notices=[Notice("guide", NoticeKind.UNSUPPORTED, "Unavailable")]
        )
    observed = inspect_installation("team", target)
    before = snapshot(tmp_path)
    assert observed.matches(desired, output) is (change == "none")
    assert observed.is_current is (change != "modified")
    assert snapshot(tmp_path) == before


def test_absent_and_unresolved_observations_do_not_match(tmp_path: Path) -> None:
    target = InstallationTarget(tmp_path / "index")
    desired, output = bundle(), rendered(tmp_path / "AGENTS.md")
    assert inspect_installation(desired.id, target).matches(desired, output) is False
    observed = sync(desired, output, target).observation
    assert replace(observed, pending=True).matches(desired, output) is False
    assert (
        replace(observed, error=TargetError(ErrorCode.IO_ERROR, "read failed")).matches(
            desired, output
        )
        is False
    )


@pytest.mark.parametrize("invalid_bundle", [False, True])
def test_comparison_rejects_invalid_public_values(tmp_path: Path, invalid_bundle: bool) -> None:
    observed = inspect_installation("team", InstallationTarget(tmp_path / "index"))
    invalid: Any = object()
    with pytest.raises(TypeError, match="comparison requires"):
        observed.matches(
            invalid if invalid_bundle else bundle(),
            rendered(tmp_path / "AGENTS.md") if invalid_bundle else invalid,
        )
