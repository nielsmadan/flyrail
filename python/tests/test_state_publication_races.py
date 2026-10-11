import errno
from collections.abc import Callable
from pathlib import Path
from typing import IO, Any

import pytest
from state_helpers import bundle, rendered, target

import flyrail._resource_io as resource_io
import flyrail._resource_transaction as transaction
from flyrail import (
    BundleEntry,
    Family,
    OperationStatus,
    RenderedArtifact,
    RenderedBundle,
    TreeContent,
    sync,
)
from flyrail._filesystem import rename_exclusive
from flyrail._resource_io import move

pytestmark = pytest.mark.integration

FOREIGN = b"foreign edit\n"

Call = Callable[..., Any]

PUBLICATION = ("before-publish-move", "after-publish-move", "before-receipt", "after-receipt")
REPLACEMENT = (
    "before-backup-move",
    "during-backup-move",
    *PUBLICATION,
    "rollback-unpublish",
    "rollback-restore",
)
CREATION = (*PUBLICATION, "rollback-unpublish")
CASES = [
    *((shape, point) for shape in ("replace-document", "replace-tree") for point in REPLACEMENT),
    *((shape, point) for shape in ("create-document", "create-tree") for point in CREATION),
]


def _hook(
    real: Call,
    action: Callable[[], None],
    *,
    after: bool = False,
    when: Callable[..., bool] = lambda *args: True,
) -> Call:
    fired: list[bool] = []

    def wrapped(*args: Any, **kwargs: Any) -> Any:
        if fired or not when(*args):
            return real(*args, **kwargs)
        fired.append(True)
        if not after:
            action()
        result = real(*args, **kwargs)
        if after:
            action()
        return result

    return wrapped


def _into(name: str) -> Callable[..., bool]:
    return lambda source, destination, *rest: Path(destination).parent.name == name


def _out_of(name: str) -> Callable[..., bool]:
    return lambda source, destination, *rest: Path(source).parent.name == name


def _fail_receipt(*args: object) -> None:
    raise OSError(errno.EIO, "injected receipt failure")


def _install(monkeypatch: pytest.MonkeyPatch, point: str, edit: Callable[[], None]) -> None:
    after = point.startswith("after")
    if point.startswith("rollback"):
        monkeypatch.setattr(transaction, "publish_receipt", _fail_receipt)
    if point == "before-backup-move":
        monkeypatch.setattr(transaction, "move", _hook(move, edit, when=_into("backup")))
    elif point == "during-backup-move":
        hooked = _hook(rename_exclusive, edit, when=_into("backup"))
        monkeypatch.setattr(resource_io, "rename_exclusive", hooked)
    elif point.endswith("publish-move"):
        hooked = _hook(rename_exclusive, edit, after=after)
        monkeypatch.setattr(transaction, "rename_exclusive", hooked)
    elif point.endswith("receipt"):
        hooked = _hook(transaction.publish_receipt, edit, after=after)
        monkeypatch.setattr(transaction, "publish_receipt", hooked)
    else:
        when = _into("staging") if point == "rollback-unpublish" else _out_of("backup")
        hooked = _hook(rename_exclusive, edit, when=when)
        monkeypatch.setattr(resource_io, "rename_exclusive", hooked)


def _tree(path: Path, text: str) -> RenderedBundle:
    content = TreeContent([BundleEntry("SKILL.md", text.encode())])
    return RenderedBundle([RenderedArtifact("asset", Family.SKILLS, path, content)])


def _append(path: Path, tree: bool) -> None:
    if tree:
        path.mkdir(exist_ok=True)
        path = path / "foreign"
    with path.open("ab") as stream:
        stream.write(FOREIGN)


def _retained(root: Path, destination: Path) -> bool:
    candidates = [destination, *root.glob(".flyrail-*.txn/*/resource")]
    files = [item for path in candidates for item in (path.rglob("*") if path.is_dir() else [path])]
    return any(path.is_file() and FOREIGN in path.read_bytes() for path in files)


@pytest.mark.parametrize(("shape", "point"), CASES)
def test_a_foreign_edit_during_publication_is_never_lost(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, shape: str, point: str
) -> None:
    tree = shape.endswith("tree")
    selected, destination = target(tmp_path), tmp_path / ("skill" if tree else "AGENTS.md")
    render = _tree if tree else rendered
    if shape.startswith("replace"):
        installed = sync(bundle(), render(destination, "one"), selected)
        assert installed.status is OperationStatus.APPLIED
    edits: list[str] = []

    def edit() -> None:
        _append(destination, tree)
        edits.append(point)

    _install(monkeypatch, point, edit)
    result = sync(bundle("2"), render(destination, "two"), selected)
    monkeypatch.undo()
    assert edits == [point], result
    assert _retained(tmp_path, destination), result


@pytest.mark.parametrize(
    "point",
    [
        "before-receipt",
        pytest.param(
            "backup-unlink",
            marks=pytest.mark.xfail(
                strict=True,
                raises=AssertionError,
                reason="a descriptor opened before the backup move writes into the backup "
                "after cleanup's last check of it, and cleanup unlinks it",
            ),
        ),
    ],
)
def test_a_descriptor_held_across_the_backup_move(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, point: str
) -> None:
    selected, agents = target(tmp_path), tmp_path / "AGENTS.md"
    assert sync(bundle(), rendered(agents, "one"), selected).status is OperationStatus.APPLIED
    held: list[IO[bytes]] = []
    writes: list[str] = []

    def hold() -> None:
        held.append(agents.open("ab"))

    def write() -> None:
        held[0].write(FOREIGN)
        held[0].flush()
        writes.append(point)

    monkeypatch.setattr(transaction, "move", _hook(move, hold, when=_into("backup")))
    if point == "before-receipt":
        hooked = _hook(transaction.publish_receipt, write)
        monkeypatch.setattr(transaction, "publish_receipt", hooked)
    else:
        hooked = _hook(Path.unlink, write, when=lambda path, *rest: path.parent.name == "backup")
        monkeypatch.setattr(Path, "unlink", hooked)
    try:
        result = sync(bundle("2"), rendered(agents, "two"), selected)
    finally:
        monkeypatch.undo()
        for stream in held:
            stream.close()
    if writes != [point]:
        pytest.fail(f"the held descriptor was not written: {result}")
    assert _retained(tmp_path, agents), result
