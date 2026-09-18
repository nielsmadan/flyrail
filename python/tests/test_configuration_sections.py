from pathlib import Path

import pytest

from flyrail import (
    Bundle,
    BundleIdentity,
    Family,
    InstallationTarget,
    InstructionArtifact,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    SectionContent,
    inspect_installation,
    remove,
    sync,
)


@pytest.mark.parametrize("newline", [b"\n", b"\r\n", b"\r"])
@pytest.mark.parametrize("independent", [False, True])
def test_adjacent_section_removal_preserves_other_owners_and_foreign_text(
    tmp_path: Path, newline: bytes, independent: bool
) -> None:
    path = tmp_path / "AGENTS.md"
    prefix = b"# Foreign" + newline
    suffix = "Üser suffix".encode() + newline
    path.write_bytes(prefix)
    installations = []
    for names in [("a",), ("b",)] if independent else [("a", "b")]:
        bundle = Bundle.from_artifacts(
            BundleIdentity(names[0], "1"), [InstructionArtifact(name, name) for name in names]
        )
        rendered = RenderedBundle(
            [
                RenderedArtifact(name, Family.INSTRUCTIONS, path, SectionContent(name, name))
                for name in names
            ]
        )
        target = InstallationTarget(tmp_path / ("index-" + names[0]))
        assert sync(bundle, rendered, target).status is OperationStatus.APPLIED
        installations.append((bundle, rendered, target))
    path.write_bytes(
        path.read_bytes().replace(
            b"<!-- flyrail:a:end -->" + newline * 2, b"<!-- flyrail:a:end -->" + newline
        )
        + suffix
    )
    for bundle, _, target in installations:
        assert inspect_installation(bundle.id, target).is_current
    last_bundle, _, last_target = installations.pop()
    assert remove(last_bundle.id, last_target).status is OperationStatus.APPLIED
    if installations:
        first_bundle, first_rendered, first_target = installations[0]
        assert inspect_installation(first_bundle.id, first_target).is_current
        assert sync(first_bundle, first_rendered, first_target).status is OperationStatus.UNCHANGED
        assert remove(first_bundle.id, first_target).status is OperationStatus.APPLIED
    assert path.read_bytes() == prefix + suffix
