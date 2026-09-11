import ctypes
import errno
import os
import sys
import time
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

from flyrail._observation import ObservationFailure, Observer
from flyrail.observations import ErrorCode


def rename_exclusive(source: Path, destination: Path) -> None:
    if os.name == "nt":
        os.rename(source, destination)
        return
    library = ctypes.CDLL(None, use_errno=True)
    if sys.platform == "darwin" and hasattr(library, "renamex_np"):
        function = library.renamex_np
        function.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
        arguments = (os.fsencode(source), os.fsencode(destination), 4)
    elif sys.platform.startswith("linux") and hasattr(library, "renameat2"):
        function = library.renameat2
        function.argtypes = [
            ctypes.c_int,
            ctypes.c_char_p,
            ctypes.c_int,
            ctypes.c_char_p,
            ctypes.c_uint,
        ]
        arguments = (-100, os.fsencode(source), -100, os.fsencode(destination), 1)
    else:
        raise OSError(errno.ENOTSUP, "exclusive rename is unsupported", destination)
    function.restype = ctypes.c_int
    ctypes.set_errno(0)
    if function(*arguments) != 0:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code), destination)


def _lock(fd: int, *, release: bool = False) -> None:
    if sys.platform == "win32":
        import msvcrt

        os.lseek(fd, 0, os.SEEK_SET)
        msvcrt.locking(fd, msvcrt.LK_UNLCK if release else msvcrt.LK_NBLCK, 1)
    else:
        import fcntl

        fcntl.flock(fd, fcntl.LOCK_UN if release else fcntl.LOCK_EX | fcntl.LOCK_NB)


@contextmanager
def target_lock(state: Path, timeout: float) -> Iterator[None]:
    if sys.platform == "win32" and (
        sys.version_info < (3, 11, 10) or (3, 12) <= sys.version_info < (3, 12, 4)
    ):
        raise ObservationFailure(
            ErrorCode.UNSUPPORTED,
            "private management directories require Windows Python 3.11.10, 3.12.4, or 3.13+",
            state,
        )
    observer = Observer()
    observer.managed_directory(state)
    observer.metadata(state / "lock")
    observer.finish()
    state.mkdir(mode=0o700, parents=True, exist_ok=True)
    with (state / "lock").open("a+b") as stream:
        metadata = Observer().metadata(state / "lock")
        opened = os.fstat(stream.fileno())
        if metadata is None or (metadata.st_dev, metadata.st_ino) != (opened.st_dev, opened.st_ino):
            raise ObservationFailure(
                ErrorCode.CONCURRENT_CHANGE, "lock file changed", state / "lock"
            )
        deadline = time.monotonic() + timeout
        while True:
            try:
                _lock(stream.fileno())
                break
            except OSError as error:
                if error.errno not in {errno.EACCES, errno.EAGAIN, errno.EDEADLK}:
                    raise
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise ObservationFailure(
                        ErrorCode.BUSY, "target lock is busy", state / "lock"
                    ) from error
                time.sleep(min(0.02, remaining))
        try:
            yield
        finally:
            _lock(stream.fileno(), release=True)
