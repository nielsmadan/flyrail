import ctypes
import errno
import os
import sys
from pathlib import Path

from flyrail._observation import ObservationFailure
from flyrail.observations import ErrorCode


def rename_exclusive(source: Path, destination: Path) -> None:
    if sys.platform == "win32" or os.name == "nt":
        os.rename(source, destination)
        return
    library = ctypes.CDLL(None, use_errno=True)
    arguments: tuple[bytes | int, ...]
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


def require_private_directories(path: Path) -> None:
    if sys.platform == "win32" and (
        sys.version_info < (3, 11, 10) or (3, 12) <= sys.version_info < (3, 12, 4)
    ):
        raise ObservationFailure(
            ErrorCode.UNSUPPORTED,
            "private management directories require Windows Python 3.11.10, 3.12.4, or 3.13+",
            path,
        )
