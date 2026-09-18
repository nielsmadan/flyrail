import multiprocessing as mp
import os
from dataclasses import replace
from pathlib import Path
from typing import Any

import pytest
from test_configuration_lifecycle import bundle, rendered

from flyrail import (
    BundleEntry,
    ErrorCode,
    Family,
    FileContent,
    InstallationTarget,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    ResourceAuthority,
    ResourceKind,
    ResourceRef,
    TreeContent,
    apply_preview,
    preview,
    sync,
)
from flyrail._codec import decode, encode
from flyrail._filesystem import target_lock
from flyrail._observation import ObservationFailure
from flyrail._resource_io import admit
from flyrail._resource_models import Index


@pytest.mark.parametrize(
    "change",
    ["foreign", "owned", "receipt", "index", "mode", "ancestor", "recovery", "hardlink", "symlink"],
)
def test_preview_rejects_every_observed_revision_change(tmp_path: Path, change: str) -> None:
    parent = tmp_path / "project"
    parent.mkdir()
    path = parent / "AGENTS.md"
    path.write_bytes(b"# foreign\n")
    target = InstallationTarget(tmp_path / "index")
    assert sync(bundle(), rendered(path), target).status is OperationStatus.APPLIED
    plan = preview(bundle("2"), rendered(path, "second"), target)
    state = ResourceAuthority(path).state_root
    if change in {"foreign", "owned"}:
        path.write_bytes(
            path.read_bytes().replace(
                b"foreign" if change == "foreign" else b"generated", b"changed"
            )
        )
    elif change == "receipt":
        value = decode((state / "receipt.json").read_bytes())
        (state / "receipt.json").write_bytes(encode(replace(value, transaction_id="f" * 32)))  # type: ignore[type-var]
    elif change == "index":
        value = decode((target.index_root / "index.json").read_bytes())
        assert isinstance(value, Index) and value.current
        (target.index_root / "index.json").write_bytes(
            encode(replace(value, current=replace(value.current, version="different")))
        )
    elif change == "mode":
        path.chmod(0o400)
    elif change == "ancestor":
        parent.rename(tmp_path / "old-project")
        parent.mkdir()
        path.write_bytes(b"replacement")
    elif change == "recovery":
        (state / "transaction.json").write_bytes(b"{}")
    elif change == "hardlink":
        os.link(path, tmp_path / "linked")
    else:
        path.rename(tmp_path / "actual")
        path.symlink_to(tmp_path / "actual")
    current = path.read_bytes()
    result = apply_preview(plan)
    assert result.status is OperationStatus.FAILED
    assert result.error is not None and result.error.code in {
        ErrorCode.CONCURRENT_CHANGE,
        ErrorCode.UNSAFE_PATH,
    }
    assert path.read_bytes() == current
    path.chmod(0o600)


def reservation_worker(destination: str, kind: str, channel: Any) -> None:
    authority = ResourceAuthority(destination)
    ref = ResourceRef(authority.destination, ResourceKind(kind), authority.state_root)
    try:
        with target_lock(ref.state_root, 0):
            channel.send("reserved")
            if not channel.poll(10):
                raise RuntimeError("barrier timed out")
            channel.recv()
            admit(ref)
        channel.send("admitted")
    except ObservationFailure as error:
        channel.send(str(error.error.code))
    finally:
        channel.close()


@pytest.mark.parametrize("order", ["tree-first", "child-first", "simultaneous"])
def test_real_process_reservations_register_before_hierarchy_validation(
    tmp_path: Path, order: str
) -> None:
    tree = tmp_path / "tree"
    tree.mkdir()
    ctx = mp.get_context("spawn")
    channels = [ctx.Pipe(), ctx.Pipe()]
    processes = [
        ctx.Process(target=reservation_worker, args=(str(path), kind, channels[index][1]))
        for index, (path, kind) in enumerate([(tree, "tree"), (tree / "child", "document")])
    ]
    first = 1 if order == "child-first" else 0

    def receive(index: int) -> object:
        assert channels[index][0].poll(10)
        return channels[index][0].recv()

    try:
        processes[first].start()
        assert receive(first) == "reserved"
        if order != "simultaneous":
            channels[first][0].send("validate")
            assert receive(first) == "admitted"
        processes[1 - first].start()
        assert receive(1 - first) == "reserved"
        channels[1 - first][0].send("validate")
        assert receive(1 - first) == "conflict"
        if order == "simultaneous":
            channels[first][0].send("validate")
            assert receive(first) == "conflict"
        for process in processes:
            process.join(10)
            assert process.exitcode == 0
        assert ResourceAuthority(tree).lock_path.is_file()
        assert ResourceAuthority(tree / "child").lock_path.is_file()
    finally:
        for process in processes:
            if process.is_alive():
                process.kill()
                process.join()
        for pair in channels:
            for channel in pair:
                channel.close()


def test_tombstoned_empty_fences_still_exclude_other_boundaries(tmp_path: Path) -> None:
    tree = tmp_path / "tree"
    tree.mkdir()
    state = ResourceAuthority(tree).state_root
    state.mkdir()
    child = RenderedBundle(
        [RenderedArtifact("child", Family.INSTRUCTIONS, tree / "child", FileContent(b"x"))]
    )
    result = sync(bundle(), child, InstallationTarget(tmp_path / "index"))
    assert result.status is OperationStatus.FAILED and result.error.code is ErrorCode.CONFLICT  # type: ignore[union-attr]
    assert ResourceAuthority(tree / "child").state_root.is_dir()
    desired = RenderedBundle(
        [RenderedArtifact("tree", Family.SKILLS, tree, TreeContent([BundleEntry("x", b"x")]))]
    )
    assert (
        sync(bundle(), desired, InstallationTarget(tmp_path / "other-index")).status
        is OperationStatus.FAILED
    )
