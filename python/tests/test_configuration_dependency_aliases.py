from pathlib import Path

import pytest
from test_configuration_lifecycle import bundle

from flyrail import (
    Dependency,
    DependencyMode,
    Family,
    FileContent,
    InstallationTarget,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    SectionContent,
    apply_preview,
    inspect_installation,
    preview,
    remove,
    sync,
)


@pytest.mark.parametrize("mode", list(DependencyMode))
@pytest.mark.parametrize("consumer_count", [1, 2])
@pytest.mark.parametrize("partial", [False, True])
def test_dependencies_on_physical_aliases_survive_install_update_and_removal(
    tmp_path: Path, mode: DependencyMode, consumer_count: int, partial: bool
) -> None:
    consumer = tmp_path / "consumer"
    target = InstallationTarget(tmp_path / "index")

    def output(version: str) -> RenderedBundle:
        launcher = tmp_path / (
            "launcher" if mode is DependencyMode.STABLE_REFERENCE else f"launcher-{version}"
        )
        consumers = [f"consumer-{index}" for index in range(consumer_count)]
        return RenderedBundle(
            [
                RenderedArtifact(name, Family.HOOKS, launcher, FileContent(version.encode()))
                for name in ["launcher-a", "launcher-b"]
            ]
            + [
                RenderedArtifact(
                    name,
                    Family.INSTRUCTIONS,
                    consumer,
                    SectionContent("entry", str(launcher))
                    if partial
                    else FileContent(str(launcher).encode()),
                )
                for name in consumers
            ],
            [
                Dependency(name, required, mode)
                for name in consumers
                for required in ["launcher-a", "launcher-b"]
            ],
        )

    first = output("1")
    proposal = preview(bundle(), first, target)
    assert proposal.applicable
    assert [len(plan.receipt.claims) for plan in proposal.resources] == [1, 1]
    launcher_plan, consumer_plan = proposal.resources
    owned = consumer_plan.receipt.claims[0]
    assert consumer_plan.resource.destination == consumer
    assert owned.requirements == (
        (launcher_plan.resource.destination, launcher_plan.receipt.claims[0].claim_id),
    )
    assert owned.stable_requirements == (
        owned.requirements if mode is DependencyMode.STABLE_REFERENCE else ()
    )
    assert apply_preview(proposal).status is OperationStatus.APPLIED
    assert inspect_installation("team", target).matches(bundle(), first)
    assert sync(bundle(), first, target).status is OperationStatus.UNCHANGED

    second = output("2")
    updated = sync(bundle("2"), second, target)
    assert updated.status is OperationStatus.APPLIED
    assert updated.observation.matches(bundle("2"), second)
    launcher = next(
        artifact.destination for artifact in second.artifacts if artifact.id == "launcher-a"
    )
    assert launcher.read_bytes() == b"2"
    assert str(launcher).encode() in consumer.read_bytes()
    removed = remove("team", target)
    assert removed.status is OperationStatus.APPLIED
    assert [result.resource.destination for result in removed.resources] == [consumer, launcher]
    assert not consumer.exists() and not launcher.exists()
