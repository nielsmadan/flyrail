import ctypes
import errno
import os
import sys
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock

import pytest
from skill_helpers import make_bundle, snapshot

from flyrail import (
    Bundle,
    MemoryStore,
    OperationStatus,
    Target,
    inspect,
    install,
    update,
)
from flyrail import _filesystem as filesystem
from flyrail.observations import ErrorCode


@pytest.mark.parametrize("version", [(3, 11, 9), (3, 12, 3)])
@pytest.mark.parametrize("operation", ["install", "update"])
def test_old_windows_python_rejects_mutations_before_writes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, version: tuple[int, int, int], operation: str
) -> None:
    source = tmp_path / "source"
    make_bundle(source)
    targets = [Target.directory(tmp_path / name / "skills") for name in ("one", "two")]
    store = MemoryStore()
    before = snapshot(tmp_path)
    monkeypatch.setattr(filesystem, "sys", SimpleNamespace(platform="win32", version_info=version))
    bundle = Bundle.from_directory(source)
    assert all(result.observation.error is None for result in inspect(bundle, targets, store=store))

    if operation == "install":
        results = install(bundle, targets, store=store)
    else:
        results = update(bundle, targets, store=store)

    assert len(results) == 2
    for result in results:
        assert result.status is OperationStatus.FAILED
        assert result.error is not None and result.error.code is ErrorCode.UNSUPPORTED
        assert result.error.path is not None
        assert result.observation.error is None
        assert result.recovery_paths == ()
    assert snapshot(tmp_path) == before
    assert list(store.keys("")) == []


@pytest.mark.parametrize(
    "platform,version",
    [
        ("win32", (3, 11, 10)),
        ("win32", (3, 12, 4)),
        ("win32", (3, 13, 0)),
        ("darwin", (3, 11, 6)),
        ("linux", (3, 11, 6)),
    ],
)
def test_private_directory_platform_guard_accepts_supported_versions(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    platform: str,
    version: tuple[int, int, int],
) -> None:
    monkeypatch.setattr(filesystem, "sys", SimpleNamespace(platform=platform, version_info=version))
    filesystem.require_private_directories(tmp_path / "state")


@pytest.mark.parametrize("kind", ["empty", "nonempty", "file"])
def test_native_exclusive_rename_preserves_collision(tmp_path: Path, kind: str) -> None:
    source, destination = tmp_path / "source", tmp_path / "destination"
    source.mkdir()
    (source / "original").write_bytes(b"source")
    if kind == "file":
        destination.write_bytes(b"destination")
    else:
        destination.mkdir()
        if kind == "nonempty":
            (destination / "original").write_bytes(b"destination")
    with pytest.raises(OSError):
        filesystem.rename_exclusive(source, destination)
    assert (source / "original").read_bytes() == b"source"
    if kind == "file":
        assert destination.read_bytes() == b"destination"
    elif kind == "nonempty":
        assert (destination / "original").read_bytes() == b"destination"
    else:
        assert list(destination.iterdir()) == []
    final = tmp_path / "final"
    filesystem.rename_exclusive(source, final)
    assert (final / "original").read_bytes() == b"source"


def test_linux_adapter_uses_native_noreplace_and_propagates_unsupported(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source, destination = tmp_path / "source", tmp_path / "destination"
    function = Mock(return_value=0)
    monkeypatch.setattr(
        filesystem, "os", SimpleNamespace(name="posix", fsencode=os.fsencode, strerror=os.strerror)
    )
    monkeypatch.setattr(ctypes, "CDLL", lambda *args, **kwargs: SimpleNamespace(renameat2=function))
    monkeypatch.setattr(sys, "platform", "linux")
    filesystem.rename_exclusive(source, destination)
    function.assert_called_once_with(-100, os.fsencode(source), -100, os.fsencode(destination), 1)
    function.return_value = -1
    function.side_effect = lambda *args: ctypes.set_errno(errno.ENOTSUP) or -1
    with pytest.raises(OSError) as caught:
        filesystem.rename_exclusive(source, destination)
    assert caught.value.errno == errno.ENOTSUP
    monkeypatch.setattr(ctypes, "CDLL", lambda *args, **kwargs: SimpleNamespace())
    with pytest.raises(OSError) as missing:
        filesystem.rename_exclusive(source, destination)
    assert missing.value.errno == errno.ENOTSUP


def test_windows_adapter_uses_os_rename(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    rename = Mock()
    monkeypatch.setattr(filesystem, "os", SimpleNamespace(name="nt", rename=rename))
    filesystem.rename_exclusive(tmp_path / "a", tmp_path / "b")
    rename.assert_called_once_with(tmp_path / "a", tmp_path / "b")
