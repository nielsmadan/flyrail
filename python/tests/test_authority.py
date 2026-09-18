import os
from pathlib import Path
from typing import Any

import pytest

from flyrail import (
    Claim,
    Key,
    Member,
    Ownership,
    ResourceAuthority,
    Scalar,
    SectionBoundaries,
    Selector,
)
from flyrail.authority import canonical_destination, hierarchy_conflicts, state_path


def test_routes_to_same_document_share_one_authority_without_writes(tmp_path: Path) -> None:
    project = tmp_path / "home/work/project"
    project.mkdir(parents=True)
    document = project / "AGENTS.md"
    document.write_bytes(b"owned and foreign text")
    from_home = ResourceAuthority(tmp_path / "home" / "work/project/AGENTS.md")
    from_project = ResourceAuthority(project / "AGENTS.md")
    assert from_home == from_project
    assert from_home.state_root.parent == project
    assert from_home.lock_path == from_home.state_root / "lock"
    assert hierarchy_conflicts(from_home, whole_tree=False) == ()
    assert list(project.iterdir()) == [document]


def test_absent_destinations_and_nested_roots_keep_disjoint_authorities(tmp_path: Path) -> None:
    home = ResourceAuthority(tmp_path / "home/AGENTS.md")
    project = ResourceAuthority(tmp_path / "home/project/AGENTS.md")
    assert home.destination == tmp_path / "home/AGENTS.md"
    assert project.state_root != home.state_root
    assert home.state_root not in project.ancestor_fences
    assert list(tmp_path.iterdir()) == []
    assert state_path(tmp_path / "ABSENT.md") == state_path(tmp_path / "absent.md")


def test_long_resource_names_have_bounded_sibling_state_names(tmp_path: Path) -> None:
    authority = ResourceAuthority(tmp_path / ("a" * 240))
    assert len(authority.state_root.name) < 100


def test_ancestor_case_aliases_preserve_claim_identity(tmp_path: Path) -> None:
    parent = tmp_path / "Config"
    parent.mkdir()
    alias = tmp_path / "config"
    if not alias.is_dir():
        pytest.skip("host is case sensitive")
    first = ResourceAuthority(parent / "absent/config.json")
    second = ResourceAuthority(alias / "absent/config.json")
    assert first == second
    claim = Claim(Ownership.STRUCTURED, Selector([Key("mcp")]))
    assert claim.identity(first) == claim.identity(second)


def test_persistent_fences_detect_tree_child_conflict_even_without_receipts(tmp_path: Path) -> None:
    tree = ResourceAuthority(tmp_path / "config")
    child = ResourceAuthority(tmp_path / "config/nested/hooks.json")
    child.state_root.mkdir(parents=True)
    tree.state_root.mkdir()
    assert hierarchy_conflicts(tree, whole_tree=True) == (child.state_root,)
    assert hierarchy_conflicts(child, whole_tree=False) == (tree.state_root,)
    assert hierarchy_conflicts(ResourceAuthority(tmp_path / "sibling.md"), whole_tree=False) == ()


def test_fence_order_excludes_both_parent_and_child_admission(tmp_path: Path) -> None:
    tree = ResourceAuthority(tmp_path / "config")
    tree.state_root.mkdir()
    assert hierarchy_conflicts(tree, whole_tree=True) == ()
    child = ResourceAuthority(tmp_path / "config/child.md")
    child.state_root.mkdir(parents=True)
    assert hierarchy_conflicts(child, whole_tree=False) == (tree.state_root,)
    assert hierarchy_conflicts(tree, whole_tree=True) == (child.state_root,)


@pytest.mark.parametrize("value", ["", "bad\0path", "/", "../other", "a/../other"])
def test_rejects_ambiguous_resource_destinations(value: str) -> None:
    with pytest.raises(ValueError):
        canonical_destination(value)


def test_relative_resource_destinations_snapshot_current_directory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)
    assert canonical_destination("AGENTS.md") == tmp_path / "AGENTS.md"


def test_resource_cannot_enter_management_state_or_regular_ancestor(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="reserved"):
        ResourceAuthority(tmp_path / ".flyrail-deadbeef.state/receipt.json")
    file = tmp_path / "file"
    file.write_bytes(b"file")
    with pytest.raises(ValueError, match="ancestor"):
        ResourceAuthority(file / "child")
    with pytest.raises(ValueError, match="whole-tree"):
        hierarchy_conflicts(ResourceAuthority(file), whole_tree=True)


@pytest.mark.parametrize("name", [".flyrail-deadbeef.state", ".FLYRAIL-deadbeef.STATE"])
@pytest.mark.parametrize("suffix", ["", "receipt.json"])
def test_resource_rejects_portable_management_names(tmp_path: Path, name: str, suffix: str) -> None:
    with pytest.raises(ValueError, match="reserved"):
        ResourceAuthority(tmp_path / name / suffix)


@pytest.mark.parametrize("suffix", ["receipt.json", "absent/receipt.json"])
def test_canonical_physical_path_cannot_enter_management_state(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, suffix: str
) -> None:
    physical = tmp_path / ".FLYRAIL-deadbeef.STATE"
    physical.mkdir()
    monkeypatch.setattr("flyrail.authority._physical_directory", lambda path: physical)
    with pytest.raises(ValueError, match="reserved"):
        ResourceAuthority(tmp_path / suffix)


def test_tree_hierarchy_detects_case_aliased_management_fences(tmp_path: Path) -> None:
    tree = tmp_path / "tree"
    fence = tree / ".FLYRAIL-deadbeef.STATE"
    fence.mkdir(parents=True)
    assert hierarchy_conflicts(ResourceAuthority(tree), whole_tree=True) == (fence,)


def test_case_aliases_share_authority_only_on_case_insensitive_hosts(tmp_path: Path) -> None:
    original = tmp_path / "AGENTS.md"
    original.write_bytes(b"text")
    alias = tmp_path / "agents.md"
    if alias.exists():
        assert ResourceAuthority(alias) == ResourceAuthority(original)
    else:
        with pytest.raises(ValueError, match="spelling"):
            ResourceAuthority(alias)
        alias.write_bytes(b"different file")
        with pytest.raises(ValueError, match="collision"):
            ResourceAuthority(original)


def test_hardlinked_documents_cannot_create_split_authorities(tmp_path: Path) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_bytes(b"same inode")
    try:
        os.link(first, second)
    except OSError:
        pytest.skip("host does not permit hard links")
    with pytest.raises(ValueError, match="hard-linked"):
        ResourceAuthority(first)
    with pytest.raises(ValueError, match="hard-linked"):
        ResourceAuthority(second)


def test_unsafe_links_are_rejected_before_hierarchy_traversal(tmp_path: Path) -> None:
    tree = tmp_path / "tree"
    tree.mkdir()
    external = tmp_path / "external"
    external.mkdir()
    link = tree / "link"
    try:
        link.symlink_to(external, target_is_directory=True)
    except OSError:
        pytest.skip("host does not permit symlinks")
    with pytest.raises(ValueError, match="links"):
        ResourceAuthority(link / "file")
    with pytest.raises(ValueError, match="links"):
        hierarchy_conflicts(ResourceAuthority(tree), whole_tree=True)


@pytest.mark.parametrize(
    "first,second,overlap",
    [
        (Claim(Ownership.FILE), Claim(Ownership.SECTION, "guide"), True),
        (Claim(Ownership.TREE), Claim(Ownership.SUBTREE, "review"), True),
        (Claim(Ownership.SUBTREE, "review"), Claim(Ownership.SUBTREE, "review/scripts"), True),
        (Claim(Ownership.SUBTREE, "review"), Claim(Ownership.SUBTREE, "other"), False),
        (Claim(Ownership.SECTION, "guide"), Claim(Ownership.SECTION, "other"), False),
        (
            Claim(Ownership.SECTION, "guide"),
            Claim(Ownership.STRUCTURED, Selector([Key("x")])),
            True,
        ),
        (
            Claim(Ownership.STRUCTURED, Selector([Key("a")])),
            Claim(Ownership.STRUCTURED, Selector([Key("b")])),
            False,
        ),
        (
            Claim(Ownership.STRUCTURED, Selector([Key("a")])),
            Claim(Ownership.STRUCTURED, Selector([Key("a"), Key("b")])),
            True,
        ),
        (
            Claim(Ownership.STRUCTURED, Selector([Member(Scalar("a"))])),
            Claim(Ownership.STRUCTURED, Selector([Member(Scalar("b"))])),
            False,
        ),
        (
            Claim(Ownership.STRUCTURED, Selector([Member(Scalar("a"), ["name"])])),
            Claim(Ownership.STRUCTURED, Selector([Member(Scalar("b"), ["id"])])),
            True,
        ),
        (
            Claim(Ownership.STRUCTURED, Selector([Key("a")])),
            Claim(Ownership.STRUCTURED, Selector([Member(Scalar("a"))])),
            True,
        ),
    ],
)
def test_claim_collision_algebra_is_symmetric(first: Claim, second: Claim, overlap: bool) -> None:
    assert first.overlaps(second) is overlap
    assert second.overlaps(first) is overlap


def test_claim_identity_uses_physical_selector_not_attribution(tmp_path: Path) -> None:
    authority = ResourceAuthority(tmp_path / "AGENTS.md")
    boundaries = SectionBoundaries("start", "end")
    first = Claim(Ownership.SECTION, boundaries)
    assert first.overlaps(Claim(Ownership.SECTION, SectionBoundaries("start", "other end")))
    assert first.overlaps(Claim(Ownership.SECTION, SectionBoundaries("end", "other end")))
    assert first.identity(authority) == Claim(
        Ownership.SECTION, SectionBoundaries("start", "end")
    ).identity(authority)
    assert first.identity(authority) != Claim(
        Ownership.SECTION, SectionBoundaries("new start", "new end")
    ).identity(authority)
    assert Claim(Ownership.STRUCTURED, Selector([Key("hooks"), Member(Scalar(1))])).identity(
        authority
    ) != Claim(Ownership.STRUCTURED, Selector([Key("hooks"), Member(Scalar(True))])).identity(
        authority
    )


@pytest.mark.parametrize(
    "kind,selector",
    [
        ("file", None),
        (Ownership.FILE, "x"),
        (Ownership.STRUCTURED, "x"),
        (Ownership.SECTION, None),
        (Ownership.SUBTREE, None),
    ],
)
def test_invalid_claim_selectors(kind: Any, selector: Any) -> None:
    with pytest.raises((ValueError, TypeError)):
        Claim(kind, selector)


def test_invalid_authority_arguments(tmp_path: Path) -> None:
    with pytest.raises(TypeError):
        hierarchy_conflicts(ResourceAuthority(tmp_path / "file"), whole_tree=1)  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        Claim(Ownership.FILE).overlaps("file")  # type: ignore[arg-type]
