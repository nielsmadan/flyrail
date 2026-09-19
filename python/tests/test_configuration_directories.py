import os
from pathlib import Path

import pytest
from test_configuration_lifecycle import bundle, receipt

from flyrail import (
    BundleEntry,
    Family,
    InstallationTarget,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    TreeContent,
    remove,
    sync,
)
from flyrail._resource_io import observe

pytestmark = pytest.mark.integration


def skill(path: Path, name: str, data: bytes = b"skill") -> RenderedBundle:
    return RenderedBundle(
        [
            RenderedArtifact(
                name,
                Family.SKILLS,
                path,
                TreeContent([BundleEntry("SKILL.md", data)]),
                subtree="namespace/" + name,
            )
        ]
    )


@pytest.mark.parametrize(
    "foreign", ["none", "file", "mode", "namespace-identity", "root-identity", "preexisting"]
)
def test_created_container_parents_belong_to_shared_provenance_until_last_owner(
    tmp_path: Path, foreign: str
) -> None:
    root = tmp_path / "skills"
    if foreign == "preexisting":
        (root / "namespace").mkdir(parents=True)
    a, b = InstallationTarget(tmp_path / "a"), InstallationTarget(tmp_path / "b")
    assert sync(bundle(identifier="a"), skill(root, "a"), a).status is OperationStatus.APPLIED
    assert sync(bundle(identifier="b"), skill(root, "b"), b).status is OperationStatus.APPLIED
    markers = receipt(root).created_directories
    if foreign == "preexisting":
        assert markers == ()
    else:
        actual = {node.path: node for node in observe(root).nodes}
        assert tuple(node.path for node in markers) == ("", "namespace")
        assert all(marker == actual[marker.path] and marker.inode for marker in markers)
    if foreign == "file":
        (root / "namespace" / "foreign").write_bytes(b"keep")
    elif foreign == "mode":
        if os.name == "nt":
            pytest.skip("POSIX restrictive directory mode")
        (root / "namespace").chmod(0o700)
    elif foreign in {"namespace-identity", "root-identity"}:
        directory = root if foreign == "root-identity" else root / "namespace"
        held = tmp_path / "held"
        directory.rename(held)
        directory.mkdir(mode=0o755)
        for child in held.iterdir():
            child.rename(directory / child.name)
        held.rmdir()
    assert remove("a", a).status is OperationStatus.APPLIED
    assert (root / "namespace" / "b" / "SKILL.md").read_bytes() == b"skill"
    assert remove("b", b).status is OperationStatus.APPLIED
    assert receipt(root).created_directories == ()
    if foreign == "none":
        assert not root.exists()
    elif foreign == "root-identity":
        assert root.is_dir() and list(root.iterdir()) == []
    elif foreign == "file":
        assert (root / "namespace" / "foreign").read_bytes() == b"keep"
    else:
        assert (root / "namespace").is_dir() and list((root / "namespace").iterdir()) == []


def test_directory_evidence_updates_to_published_identity_after_content_update(
    tmp_path: Path,
) -> None:
    root = tmp_path / "skills"
    target = InstallationTarget(tmp_path / "index")
    assert sync(bundle(), skill(root, "review"), target).status is OperationStatus.APPLIED
    first = receipt(root).created_directories
    assert (
        sync(bundle("2"), skill(root, "review", b"updated"), target).status
        is OperationStatus.APPLIED
    )
    second = receipt(root).created_directories
    actual = {node.path: node for node in observe(root).nodes}
    assert all(node == actual[node.path] for node in second)
    assert tuple(node.inode for node in first) != tuple(node.inode for node in second)
    assert remove("team", target).status is OperationStatus.APPLIED
    assert not root.exists()


def test_modified_subtree_retains_created_parent_evidence_for_explicit_retry(
    tmp_path: Path,
) -> None:
    root = tmp_path / "skills"
    target = InstallationTarget(tmp_path / "index")
    assert sync(bundle(), skill(root, "review"), target).status is OperationStatus.APPLIED
    original = receipt(root)
    (root / "namespace" / "review" / "SKILL.md").write_bytes(b"local changes")
    assert remove("team", target).status is OperationStatus.FAILED
    assert receipt(root) == original
    assert remove("team", target, replace_modified=True).status is OperationStatus.APPLIED
    assert not root.exists()
