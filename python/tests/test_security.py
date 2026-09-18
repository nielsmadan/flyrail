import ctypes
import errno
import json
import os
import shutil
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from threading import Barrier, Event
from typing import Any
from unittest.mock import Mock

import pytest

from flyrail import _security as security

pytestmark = pytest.mark.skipif(sys.platform != "darwin", reason="native macOS metadata")


def test_reused_darwin_bindings_observe_added_changed_and_removed_xattrs(tmp_path: Path) -> None:
    path = tmp_path / "ancestor"
    path.mkdir()
    before = security.security(path, ancestor=True)
    resource_before = security.security(path)
    name = "flyrail.security-test"
    executable = shutil.which("xattr")
    assert executable is not None
    for payload in (b"first", b"changed and longer", b"short"):
        subprocess.run(  # noqa: S603
            [executable, "-w", name, payload.decode(), str(path)], check=True, capture_output=True
        )
        captured = json.loads(security.security(path, ancestor=True))
        assert dict(captured["xattrs"])[name.encode().hex()] == payload.hex()
        with pytest.raises(OSError) as caught:
            security.security(path)
        assert caught.value.errno == errno.ENOTSUP
    subprocess.run(  # noqa: S603
        [executable, "-d", name, str(path)], check=True, capture_output=True
    )
    assert security.security(path, ancestor=True) == before
    assert security.security(path) == resource_before


def test_darwin_initialization_is_shared_by_concurrent_observations(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    before = security.security(tmp_path, ancestor=True)
    original = ctypes.CDLL
    started, release = Event(), Event()

    def load(*args: Any, **kwargs: Any) -> ctypes.CDLL:
        started.set()
        assert release.wait(5)
        return original(*args, **kwargs)

    loader = Mock(side_effect=load)
    monkeypatch.setattr(security, "_darwin_library", None)
    monkeypatch.setattr(ctypes, "CDLL", loader)
    with ThreadPoolExecutor(max_workers=2) as executor:
        first = executor.submit(security.security, tmp_path, ancestor=True)
        try:
            assert started.wait(5)
            second = executor.submit(security.security, tmp_path, ancestor=True)
            with pytest.raises(TimeoutError):
                second.result(timeout=0.05)
        finally:
            release.set()
        assert first.result(timeout=5) == before
        assert second.result(timeout=5) == before
    loader.assert_called_once_with(None, use_errno=True)


def test_darwin_initialization_failure_does_not_publish_partial_bindings(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    before = security.security(tmp_path, ancestor=True)
    native = ctypes.CDLL(None, use_errno=True)

    class IncompleteLibrary:
        def __getattr__(self, name: str) -> Any:
            if name == "acl_to_text":
                raise OSError(errno.EIO, "cannot bind ACL text")
            return getattr(native, name)

    loader = Mock(side_effect=[IncompleteLibrary(), native])
    monkeypatch.setattr(security, "_darwin_library", None)
    monkeypatch.setattr(ctypes, "CDLL", loader)
    with pytest.raises(OSError, match="cannot bind ACL text") as caught:
        security.security(tmp_path, ancestor=True)
    assert caught.value.errno == errno.EIO
    assert security.security(tmp_path, ancestor=True) == before
    assert loader.call_count == 2


def test_reused_darwin_bindings_preserve_thread_local_native_errors(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    security.security(tmp_path, ancestor=True)
    regular = tmp_path / "regular"
    regular.write_bytes(b"file")
    missing, non_directory = tmp_path / "missing", regular / "child"
    original_lstat, original_errno = Path.lstat, ctypes.get_errno
    sampled = regular.lstat()
    finished = Barrier(2)

    def lstat(path: Path) -> os.stat_result:
        return sampled if path in {missing, non_directory} else original_lstat(path)

    def get_errno() -> int:
        finished.wait(timeout=5)
        return original_errno()

    def observe(path: Path) -> int | None:
        ctypes.set_errno(errno.EIO)
        with pytest.raises(OSError, match="cannot inspect extended attributes") as caught:
            security.security(path)
        return caught.value.errno

    monkeypatch.setattr(Path, "lstat", lstat)
    monkeypatch.setattr(ctypes, "get_errno", get_errno)
    with ThreadPoolExecutor(max_workers=2) as executor:
        assert list(executor.map(observe, [missing, non_directory])) == [
            errno.ENOENT,
            errno.ENOTDIR,
        ]
