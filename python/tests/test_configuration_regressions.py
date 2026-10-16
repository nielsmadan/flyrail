import errno
import os
from contextlib import AbstractContextManager
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace

import pytest
from state_helpers import journal_directory, put
from test_configuration_lifecycle import bundle, rendered

from flyrail import (
    Acquisition,
    Bundle,
    BundleEntry,
    BundleIdentity,
    DirectoryStore,
    ErrorCode,
    Family,
    FileContent,
    InstallationTarget,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    ResourceAuthority,
    SkillArtifact,
    Target,
    TreeContent,
    apply_preview,
    inspect,
    install,
    preview,
    remove,
    sync,
    uninstall,
    update,
)
from flyrail import _lifecycle as lifecycle
from flyrail import _resource_io as io
from flyrail import _resource_transaction as tx
from flyrail._resource_models import Node, Receipt, Revision
from flyrail._state import read_record, resource_key
from flyrail.authority import transaction_path
from flyrail.configuration import ResourcePlan
from flyrail.inspection import skill_targets
from flyrail.stores import StateStore

pytestmark = pytest.mark.integration


def tree_rendering(path: Path, value: bytes) -> RenderedBundle:
    tree = TreeContent(BundleEntry(name, value) for name in ("a", "branch/b", "branch/c", "z"))
    return RenderedBundle([RenderedArtifact("tree", Family.HOOKS, path, tree)])


def test_file_takeover_preserves_nested_owned_document_and_fence(tmp_path: Path) -> None:
    directory = tmp_path / "directory"
    directory.mkdir()
    child = directory / "AGENTS.md"
    child_target = InstallationTarget(DirectoryStore(tmp_path / "child-index"))
    assert sync(bundle(), rendered(child), child_target).status is OperationStatus.APPLIED
    original = io.observe(directory)
    desired = RenderedBundle(
        [RenderedArtifact("file", Family.HOOKS, directory, FileContent(b"outer"))]
    )
    result = sync(
        bundle(identifier="outer"),
        desired,
        InstallationTarget(DirectoryStore(tmp_path / "outer-index")),
        acquisition=Acquisition.TAKEOVER,
    )
    assert result.status is OperationStatus.FAILED
    assert io.observe(directory) == original
    assert remove("team", child_target).status is OperationStatus.APPLIED


@pytest.mark.skipif(os.name == "nt", reason="POSIX private directory modes")
@pytest.mark.parametrize("unsafe", ["state", "staging", "backup"])
def test_unrecorded_public_management_directory_is_left_untouched(
    tmp_path: Path, unsafe: str
) -> None:
    path = tmp_path / "file"
    path.write_bytes(b"private original")
    path.chmod(0o644)
    destination = ResourceAuthority(path).destination
    state = transaction_path(destination, "e" * 32)
    state.mkdir(mode=0o700)
    for name in ("staging", "backup"):
        (state / name).mkdir(mode=0o700)
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))
    selected = state if unsafe == "state" else state / unsafe
    selected.chmod(0o755)
    desired = RenderedBundle([RenderedArtifact("file", Family.HOOKS, path, FileContent(b"new"))])
    result = sync(bundle(), desired, target, acquisition=Acquisition.TAKEOVER)
    assert result.status is OperationStatus.APPLIED
    assert selected.stat().st_mode & 0o777 == 0o755
    assert path.read_bytes() == b"new"
    assert sorted(child.name for child in tmp_path.iterdir()) == sorted(
        ["file", "index", state.name]
    )
    assert tuple((state / "staging").iterdir()) == ()
    assert tuple((state / "backup").iterdir()) == ()
    assert read_record(target.store, resource_key(destination, "receipt"), Receipt) is not None


def test_windows_payload_privacy_is_checked_before_writing_any_bytes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    destination = tmp_path / "file"
    observed: list[bytes] = []

    def refuse(path: Path) -> None:
        observed.append(path.read_bytes())
        raise OSError(errno.ENOTSUP, "payload inherited foreign DACL", path)

    monkeypatch.setattr(io, "os", SimpleNamespace(name="nt"))
    monkeypatch.setattr(io, "ensure_private", refuse)
    desired = Revision((Node("", b"private payload", 0o600, 0, 0, 0, 0),))
    with pytest.raises(OSError, match="foreign DACL"):
        io.write_revision(destination, desired)
    assert observed == [b""]
    assert destination.read_bytes() == b""


@pytest.mark.skipif(os.name == "nt", reason="POSIX ancestor mode mutation")
def test_approved_ancestor_evidence_survives_resource_preparation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    project.mkdir(mode=0o755)
    path = project / "AGENTS.md"
    path.write_bytes(b"foreign\n")
    target = InstallationTarget(DirectoryStore(tmp_path / "metadata" / "index"))
    plan = preview(bundle(), rendered(path), target)
    original_prepare = tx.prepare

    def prepare(store: StateStore, resource_plan: ResourcePlan) -> tx.Journal:
        project.chmod(0o700)
        return original_prepare(store, resource_plan)

    monkeypatch.setattr(lifecycle, "prepare", prepare)
    result = apply_preview(plan)
    assert result.status is OperationStatus.FAILED
    assert path.read_bytes() == b"foreign\n"


def test_preview_accepts_only_its_explicitly_created_missing_parents(tmp_path: Path) -> None:
    path = tmp_path / "project" / "nested" / "AGENTS.md"
    target = InstallationTarget(DirectoryStore(tmp_path / "metadata" / "index"))
    plan = preview(bundle(), rendered(path), target)
    assert apply_preview(plan).status is OperationStatus.APPLIED
    assert b"generated" in path.read_bytes()


@pytest.mark.parametrize("damage", ["journal", "unknown", "stage", "backup"])
@pytest.mark.parametrize("existing", [False, True])
def test_final_receipt_commit_rejects_changed_journal_and_layout(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, damage: str, existing: bool
) -> None:
    path = tmp_path / "AGENTS.md"
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))
    if existing:
        assert sync(bundle(), rendered(path), target).status is OperationStatus.APPLIED
    destination = ResourceAuthority(path).destination
    store = target.store
    previous = read_record(store, resource_key(destination, "receipt"), Receipt)
    original = tx.publish_receipt

    def corrupt(store: StateStore, journal: tx.Journal) -> None:
        evidence = b"unrecognized private evidence"
        if damage == "journal":
            put(store, resource_key(destination, "journal"), evidence)
        else:
            selected = {
                "unknown": tx.transaction_directory(store, journal) / "unexpected",
                "stage": tx.paths(tx.transaction_directory(store, journal))[0],
                "backup": tx.paths(tx.transaction_directory(store, journal))[1],
            }[damage]
            selected.write_bytes(evidence)
        original(store, journal)

    monkeypatch.setattr(tx, "publish_receipt", corrupt)
    result = sync(bundle("2"), rendered(path, "second"), target)
    assert result.status is OperationStatus.INCOMPLETE
    assert read_record(store, resource_key(destination, "receipt"), Receipt) == previous
    assert store.read(resource_key(destination, "journal")) is not None


@pytest.mark.parametrize("first_alias", [False, True])
def test_equivalent_skill_routes_inspect_update_and_remove_across_calls(
    tmp_path: Path, first_alias: bool
) -> None:
    store = DirectoryStore(tmp_path / "state")
    directory = Target.directory(tmp_path / "skills")
    alias = Target.user("claude", home=tmp_path, env={"CLAUDE_CONFIG_DIR": str(tmp_path)})
    first, second = (alias, directory) if first_alias else (directory, alias)

    def skill(version: str) -> Bundle:
        return Bundle.from_artifacts(
            BundleIdentity("routes", version),
            [
                SkillArtifact(
                    "skill", "example", TreeContent([BundleEntry("SKILL.md", version.encode())])
                )
            ],
        )

    assert install(skill("1"), [first], store=store)[0].status is OperationStatus.APPLIED
    for targets in ([second], [second, first], [first, second]):
        assert all(
            item.observation.is_current for item in inspect(skill("1"), targets, store=store)
        )
    assert update(skill("2"), [second], store=store)[0].status is OperationStatus.APPLIED
    assert inspect(skill("2"), [first], store=store)[0].observation.is_current
    assert uninstall("routes", [first], store=store)[0].status is OperationStatus.APPLIED
    assert uninstall("routes", [second], store=store)[0].status is OperationStatus.UNCHANGED
    routes = skill_targets("routes", [first, second], store)
    assert isinstance(routes[0].installation, InstallationTarget)
    assert isinstance(routes[1].installation, InstallationTarget)
    assert routes[0].installation.context_digest == routes[1].installation.context_digest
    assert routes[0].installation.routing_context != routes[1].installation.routing_context


def test_routing_context_is_frozen_in_the_preview_without_changing_index_identity(
    tmp_path: Path,
) -> None:
    pairs = [["agent", "claude"]]
    target = InstallationTarget(
        DirectoryStore(tmp_path / "index"), routing_context=((key, value) for key, value in pairs)
    )
    plan = preview(bundle(), rendered(tmp_path / "AGENTS.md"), target)
    pairs[0][1] = "changed"
    assert plan.target.routing_context == (("agent", "claude"),)
    assert (
        target.context_digest
        == replace(target, routing_context=(("agent", "other"),)).context_digest
    )
    assert apply_preview(plan).status is OperationStatus.APPLIED


@pytest.mark.parametrize("rollback", [False, True])
@pytest.mark.parametrize("damage", ["none", "bytes", "identity", "metadata", "gap"])
def test_source_free_retry_resumes_interior_tree_cleanup_and_rejects_changed_remainder(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, rollback: bool, damage: str
) -> None:
    path = tmp_path / "tree"
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))
    assert sync(bundle(), tree_rendering(path, b"old"), target).status is OperationStatus.APPLIED
    original = Path.unlink
    selected_parent = "staging" if rollback else "backup"

    def unlink(selected: Path, missing_ok: bool = False) -> None:
        original(selected, missing_ok=missing_ok)
        if selected.name == "c" and selected.parent.parent.parent.name == selected_parent:
            raise SystemExit("interrupted interior cleanup")

    def refuse(store: StateStore, journal: tx.Journal) -> None:
        raise OSError(errno.EIO, "receipt unavailable")

    with monkeypatch.context() as patch:
        patch.setattr(Path, "unlink", unlink)
        if rollback:
            patch.setattr(tx, "publish_receipt", refuse)
        with pytest.raises(SystemExit):
            sync(bundle("2"), tree_rendering(path, b"new"), target)
    destination = ResourceAuthority(path).destination
    retained = journal_directory(target.store, destination) / selected_parent / "resource"
    public = io.observe(path)
    receipt = read_record(target.store, resource_key(destination, "receipt"), Receipt)
    assert receipt is not None and receipt.claims[0].version == ("1" if rollback else "2")
    assert (path / "a").read_bytes() == (b"old" if rollback else b"new")
    if damage == "bytes":
        (retained / "a").write_bytes(b"changed private content")
    elif damage == "identity":
        selected = retained / "a"
        selected.rename(tmp_path / "retained-original")
        selected.write_bytes((tmp_path / "retained-original").read_bytes())
    elif damage == "metadata":
        (retained / "a").chmod(0o400)
    elif damage == "gap":
        (retained / "a").unlink()
    result = remove("team", target)
    if damage == "none":
        assert result.status is OperationStatus.APPLIED
        assert not path.exists()
        assert remove("team", target).status is OperationStatus.UNCHANGED
    else:
        assert result.status is OperationStatus.INCOMPLETE
        assert io.observe(path) == public
        assert read_record(target.store, resource_key(destination, "receipt"), Receipt) == receipt
        assert target.store.read(resource_key(destination, "journal")) is not None


@pytest.mark.skipif(os.name == "nt", reason="POSIX storage modes")
@pytest.mark.parametrize("phase", ["before-publish", "before-receipt", "after-commit"])
def test_private_storage_metadata_is_bound_through_publication_and_cleanup(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, phase: str
) -> None:
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"foreign")
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))
    original_publish = tx.publish
    original_receipt = tx.publish_receipt
    original_cleanup = tx.cleanup
    destination = ResourceAuthority(path).destination

    def changed(store: StateStore, journal: tx.Journal) -> None:
        (tx.transaction_directory(store, journal) / "backup").chmod(0o755)
        if phase == "before-publish":
            original_publish(store, journal)
        elif phase == "before-receipt":
            original_receipt(store, journal)
        else:
            original_cleanup(store, journal)

    if phase == "before-publish":
        monkeypatch.setattr(lifecycle, "publish", changed)
    elif phase == "before-receipt":
        monkeypatch.setattr(tx, "publish_receipt", changed)
    else:
        monkeypatch.setattr(lifecycle, "cleanup", changed)
    result = sync(bundle(), rendered(path), target)
    state = journal_directory(target.store, destination)
    assert (state / "backup").stat().st_mode & 0o777 == 0o755
    assert target.store.read(resource_key(destination, "journal")) is not None
    stored = read_record(target.store, resource_key(destination, "receipt"), Receipt)
    if phase == "after-commit":
        assert result.status is OperationStatus.PARTIAL
        assert stored is not None and stored.claims[0].version == "1"
        assert b"generated" in path.read_bytes()
    else:
        assert result.status is OperationStatus.INCOMPLETE
        assert stored is None
        if phase == "before-publish":
            assert path.read_bytes() == b"foreign"


def test_case_equivalent_skill_routes_share_the_completed_render_digest(tmp_path: Path) -> None:
    store = DirectoryStore(tmp_path / "state")
    parent = tmp_path / "Config"
    parent.mkdir()
    alias = tmp_path / "config"
    if not alias.is_dir():
        pytest.skip("host is case sensitive")
    original = Target.directory(parent / "skills")
    equivalent = Target.directory(alias / "skills")
    skill = Bundle.from_artifacts(
        BundleIdentity("case", "1"),
        [SkillArtifact("skill", "example", TreeContent([BundleEntry("SKILL.md", b"skill")]))],
    )
    assert install(skill, [original], store=store)[0].status is OperationStatus.APPLIED
    assert inspect(skill, [equivalent], store=store)[0].observation.is_current
    assert (
        install(skill, [equivalent, original], store=store)[0].status is OperationStatus.UNCHANGED
    )
    assert uninstall("case", [equivalent], store=store)[0].status is OperationStatus.APPLIED


def test_preview_does_not_accept_an_ancestor_created_by_another_writer_during_locking(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "project" / "nested" / "AGENTS.md"
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))
    plan = preview(bundle(), rendered(path), target)
    original = Path.mkdir

    def mkdir(
        selected: Path, mode: int = 0o777, parents: bool = False, exist_ok: bool = False
    ) -> None:
        if selected == target.store.location:
            path.parent.mkdir(mode=0o700, parents=True)
        original(selected, mode=mode, parents=parents, exist_ok=exist_ok)

    monkeypatch.setattr(Path, "mkdir", mkdir)
    result = apply_preview(plan)
    assert result.status is OperationStatus.FAILED
    assert path.parent.is_dir()
    assert not path.exists()


@pytest.mark.skipif(os.name == "nt", reason="POSIX directory modes")
def test_preview_does_not_accept_a_store_ancestor_created_by_another_writer_during_locking(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project = tmp_path / "project"
    path = project / "AGENTS.md"
    store = DirectoryStore(project / "state")
    target = InstallationTarget(store)
    plan = preview(bundle(), rendered(path), target)
    original_lock = store.lock
    armed = True

    def lock(name: str, timeout: float) -> AbstractContextManager[None]:
        nonlocal armed
        if armed:
            armed = False
            if project.exists():
                project.rename(tmp_path / "replaced")
            project.mkdir()
            project.chmod(0o777)
        return original_lock(name, timeout)

    monkeypatch.setattr(store, "lock", lock)
    result = apply_preview(plan)
    assert result.status is OperationStatus.FAILED
    assert result.error is not None and result.error.code is ErrorCode.CONCURRENT_CHANGE
    assert not path.exists()


@pytest.mark.parametrize("folder", ["staging", "backup"])
def test_preparation_rejects_replaced_existing_storage_before_writing_payload(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, folder: str
) -> None:
    path = tmp_path / "AGENTS.md"
    target = InstallationTarget(DirectoryStore(tmp_path / "index"))
    assert sync(bundle(), rendered(path), target).status is OperationStatus.APPLIED
    destination = ResourceAuthority(path).destination
    before = io.observe(path)
    previous = read_record(target.store, resource_key(destination, "receipt"), Receipt)
    plan = preview(bundle("2"), rendered(path, "second"), target)
    state = plan.resources[0].transaction_directory
    original = io.require
    armed = True

    def require(selected: Path, revision: Revision) -> None:
        nonlocal armed
        if selected == state / folder and armed:
            armed = False
            (state / folder).rename(tmp_path / "original-storage")
            (state / folder).mkdir(mode=0o700)
        original(selected, revision)

    monkeypatch.setattr(tx, "require", require)
    result = apply_preview(plan)
    assert result.status is OperationStatus.FAILED
    assert io.observe(path) == before
    assert read_record(target.store, resource_key(destination, "receipt"), Receipt) == previous
    assert not state.exists()
    assert tuple((tmp_path / "original-storage").iterdir()) == ()
