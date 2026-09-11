import ctypes
import errno
import os
import sys
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock

import pytest
from test_inspection import make_bundle, snapshot

from flyrail import Bundle, OperationStatus, Target, inspect, install, uninstall, update
from flyrail import _filesystem as filesystem
from flyrail._observation import ObservationFailure
from flyrail.observations import ErrorCode


@pytest.mark.parametrize("version", [(3, 11, 9), (3, 12, 3)])
@pytest.mark.parametrize("operation", ["install", "update", "uninstall"])
def test_old_windows_python_rejects_mutations_before_writes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, version: tuple[int, int, int], operation: str
) -> None:
    source = tmp_path / "source"
    make_bundle(source)
    targets = [Target.directory(tmp_path / name / "skills") for name in ("one", "two")]
    before = snapshot(tmp_path)
    monkeypatch.setattr(filesystem, "sys", SimpleNamespace(platform="win32", version_info=version))
    bundle = Bundle.from_directory(source)
    assert all(result.observation.error is None for result in inspect(bundle, targets))

    if operation == "install":
        results = install(bundle, targets)
    elif operation == "update":
        results = update(bundle, targets)
    else:
        results = uninstall(bundle.id, targets)

    assert len(results) == 2
    for result in results:
        assert result.status is OperationStatus.FAILED
        assert result.error is not None and result.error.code is ErrorCode.UNSUPPORTED
        assert result.error.path == result.state_root
        assert result.observation.error is None
        assert result.recovery_paths == ()
    assert snapshot(tmp_path) == before


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
    lock = Mock()
    monkeypatch.setattr(filesystem, "_lock", lock)
    state = tmp_path / "state"

    with filesystem.target_lock(state, 0):
        assert (state / "lock").is_file()
        assert lock.call_count == 1

    assert lock.call_count == 2
    assert lock.call_args.kwargs == {"release": True}


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


def test_windows_lock_seeks_byte_zero_and_never_uses_blocking_mode(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls: list[tuple[int, int, int]] = []
    with (tmp_path / "lock").open("w+b") as stream:
        stream.write(b"data")
        stream.flush()

        def lock(fd: int, mode: int, size: int) -> None:
            calls.append((os.lseek(fd, 0, os.SEEK_CUR), mode, size))

        with monkeypatch.context() as patch:
            patch.setitem(
                sys.modules, "msvcrt", SimpleNamespace(locking=lock, LK_NBLCK=2, LK_UNLCK=0)
            )
            patch.setattr(sys, "platform", "win32")
            filesystem._lock(stream.fileno())
            stream.seek(3)
            filesystem._lock(stream.fileno(), release=True)
    assert calls == [(0, 2, 1), (0, 0, 1)]


def test_lock_io_error_is_not_retried(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    lock = Mock(side_effect=OSError(errno.EIO, "device failure"))
    monkeypatch.setattr(filesystem, "_lock", lock)
    with pytest.raises(OSError) as caught, filesystem.target_lock(tmp_path / "state", 0.1):
        pytest.fail("failed locking must not enter the context")
    assert caught.value.errno == errno.EIO
    assert lock.call_count == 1


def test_replaced_lock_is_detected(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    original = os.fstat

    def wrong_identity(fd: int) -> SimpleNamespace:
        metadata = original(fd)
        return SimpleNamespace(st_dev=metadata.st_dev, st_ino=metadata.st_ino + 1)

    monkeypatch.setattr(os, "fstat", wrong_identity)
    with pytest.raises(ObservationFailure) as caught, filesystem.target_lock(tmp_path / "state", 0):
        pytest.fail("replaced locking file must not enter the context")
    assert caught.value.error.code is ErrorCode.CONCURRENT_CHANGE
