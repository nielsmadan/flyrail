import ctypes
import errno
import json
import os
import stat
import sys
from pathlib import Path
from threading import Lock
from typing import Any, cast

_native: Any = ctypes
_darwin_library: ctypes.CDLL | None = None
_darwin_lock = Lock()


def _darwin() -> ctypes.CDLL:
    global _darwin_library
    with _darwin_lock:
        if _darwin_library is None:
            library = ctypes.CDLL(None, use_errno=True)
            library.listxattr.argtypes = [
                ctypes.c_char_p,
                ctypes.c_void_p,
                ctypes.c_size_t,
                ctypes.c_int,
            ]
            library.listxattr.restype = ctypes.c_ssize_t
            library.getxattr.argtypes = [
                ctypes.c_char_p,
                ctypes.c_char_p,
                ctypes.c_void_p,
                ctypes.c_size_t,
                ctypes.c_uint,
                ctypes.c_int,
            ]
            library.getxattr.restype = ctypes.c_ssize_t
            library.acl_get_file.argtypes = [ctypes.c_char_p, ctypes.c_int]
            library.acl_get_file.restype = ctypes.c_void_p
            library.acl_get_entry.argtypes = [
                ctypes.c_void_p,
                ctypes.c_int,
                ctypes.POINTER(ctypes.c_void_p),
            ]
            library.acl_get_entry.restype = ctypes.c_int
            library.acl_free.argtypes = [ctypes.c_void_p]
            library.acl_free.restype = ctypes.c_int
            library.acl_to_text.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ssize_t)]
            library.acl_to_text.restype = ctypes.c_void_p
            _darwin_library = library
        return _darwin_library


def _windows() -> ctypes.CDLL:
    return cast(ctypes.CDLL, _native.WinDLL("advapi32", use_last_error=True))


def security(path: Path, *, ancestor: bool = False) -> bytes:
    if sys.platform == "win32":
        library = _windows()
        function = library.GetFileSecurityW
        function.argtypes = [
            ctypes.c_wchar_p,
            ctypes.c_uint,
            ctypes.c_void_p,
            ctypes.c_uint,
            ctypes.POINTER(ctypes.c_uint),
        ]
        function.restype = ctypes.c_int
        size = ctypes.c_uint()
        function(str(path), 7, None, 0, ctypes.byref(size))
        if not size.value:
            raise cast(OSError, _native.WinError(_native.get_last_error()))
        buffer = ctypes.create_string_buffer(size.value)
        if not function(str(path), 7, buffer, size.value, ctypes.byref(size)):
            raise cast(OSError, _native.WinError(_native.get_last_error()))
        return buffer.raw[: size.value]
    flags = getattr(path.lstat(), "st_flags", 0)
    if flags and not ancestor:
        raise OSError(errno.ENOTSUP, "file flags cannot be preserved safely", path)
    provenance = b""
    attributes: set[bytes] = set()
    values: list[tuple[bytes, bytes]] = []
    acl_text = b""
    if sys.platform == "darwin":
        library = _darwin()
        count = library.listxattr(os.fsencode(path), None, 0, 1)
        if count < 0:
            raise OSError(ctypes.get_errno(), "cannot inspect extended attributes", path)
        if count:
            names = ctypes.create_string_buffer(count)
            if library.listxattr(os.fsencode(path), names, count, 1) != count:
                raise OSError(errno.EAGAIN, "extended attributes changed", path)
            attributes = set(names.raw.rstrip(b"\0").split(b"\0"))
            if not ancestor and attributes - {b"com.apple.provenance"}:
                raise OSError(errno.ENOTSUP, "extended attributes are unsupported", path)
        if count:
            values = []
            for attribute in sorted(attributes):
                size = library.getxattr(os.fsencode(path), attribute, None, 0, 0, 1)
                if size < 0:
                    raise OSError(ctypes.get_errno(), "cannot read extended metadata", path)
                payload = ctypes.create_string_buffer(size)
                if library.getxattr(os.fsencode(path), attribute, payload, size, 0, 1) != size:
                    raise OSError(errno.EAGAIN, "extended metadata changed", path)
                values.append((attribute, payload.raw))
            if not ancestor:
                provenance = b"com.apple.provenance\0" + values[0][1]
        acl = library.acl_get_file(os.fsencode(path), 0x100)
        if not acl:
            if ctypes.get_errno() == errno.ENOENT:
                path.lstat()
                return ancestor_metadata(flags, values, acl_text) if ancestor else provenance
            raise OSError(ctypes.get_errno(), "cannot inspect file ACL", path)
        try:
            entry = ctypes.c_void_p()
            code = library.acl_get_entry(acl, 0, ctypes.byref(entry))
            if code != -1:
                if not ancestor:
                    raise OSError(errno.ENOTSUP, "extended ACLs are unsupported", path)
                length = ctypes.c_ssize_t()
                payload = library.acl_to_text(acl, ctypes.byref(length))
                if not payload:
                    raise OSError(ctypes.get_errno(), "cannot capture ancestor ACL", path)
                try:
                    acl_text = ctypes.string_at(payload, length.value)
                finally:
                    library.acl_free(payload)
        finally:
            library.acl_free(acl)
    else:
        names = os.listxattr(path, follow_symlinks=False)
        if names and not ancestor:
            raise OSError(errno.ENOTSUP, "extended attributes and ACLs are unsupported", path)
        if ancestor:
            values = [
                (os.fsencode(name), os.getxattr(path, name, follow_symlinks=False))
                for name in sorted(names)
            ]
    return ancestor_metadata(flags, values, acl_text) if ancestor else provenance


def ancestor_metadata(flags: int, attributes: list[tuple[bytes, bytes]], acl: bytes) -> bytes:
    return json.dumps(
        {
            "schema_version": 1,
            "flags": flags,
            "xattrs": [[name.hex(), value.hex()] for name, value in sorted(attributes)],
            "acl": acl.hex(),
        },
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")


def set_security(path: Path, descriptor: bytes, *, protected: bool | None = None) -> None:
    function = _windows().SetFileSecurityW
    function.argtypes = [ctypes.c_wchar_p, ctypes.c_uint, ctypes.c_void_p]
    function.restype = ctypes.c_int
    buffer = ctypes.create_string_buffer(descriptor)
    control, revision = ctypes.c_ushort(), ctypes.c_uint()
    inspect = _windows().GetSecurityDescriptorControl
    inspect.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(ctypes.c_ushort),
        ctypes.POINTER(ctypes.c_uint),
    ]
    inspect.restype = ctypes.c_int
    if not inspect(buffer, ctypes.byref(control), ctypes.byref(revision)):
        raise cast(OSError, _native.WinError(_native.get_last_error()))
    restrictive = bool(control.value & 0x1000) if protected is None else protected
    flags = 0x80000000 if restrictive else 0x20000000
    if not function(str(path), 4 | flags, buffer):
        raise cast(OSError, _native.WinError(_native.get_last_error()))


def replace_file(destination: Path, staged: Path, backup: Path) -> None:
    library = _native.WinDLL("kernel32", use_last_error=True)
    function = library.ReplaceFileW
    function.argtypes = [
        ctypes.c_wchar_p,
        ctypes.c_wchar_p,
        ctypes.c_wchar_p,
        ctypes.c_uint,
        ctypes.c_void_p,
        ctypes.c_void_p,
    ]
    function.restype = ctypes.c_int
    if not function(str(destination), str(staged), str(backup), 0, None, None):
        raise cast(OSError, _native.WinError(_native.get_last_error()))


def validate_windows_file(path: Path) -> None:
    metadata = path.lstat()
    attributes = getattr(metadata, "st_file_attributes", 0)
    unsupported = (
        stat.FILE_ATTRIBUTE_READONLY
        | stat.FILE_ATTRIBUTE_ENCRYPTED
        | stat.FILE_ATTRIBUTE_COMPRESSED
        | stat.FILE_ATTRIBUTE_SPARSE_FILE
    )
    if attributes & unsupported:
        raise OSError(errno.ENOTSUP, "unsupported Windows file attributes", path)
    if stat.S_ISDIR(metadata.st_mode):
        return
    library = _native.WinDLL("kernel32", use_last_error=True)

    class Stream(ctypes.Structure):
        _fields_ = [("size", ctypes.c_longlong), ("name", ctypes.c_wchar * 296)]

    library.FindFirstStreamW.argtypes = [
        ctypes.c_wchar_p,
        ctypes.c_uint,
        ctypes.POINTER(Stream),
        ctypes.c_uint,
    ]
    library.FindFirstStreamW.restype = ctypes.c_void_p
    library.FindNextStreamW.argtypes = [ctypes.c_void_p, ctypes.POINTER(Stream)]
    library.FindClose.argtypes = [ctypes.c_void_p]
    stream = Stream()
    handle = library.FindFirstStreamW(str(path), 0, ctypes.byref(stream), 0)
    if handle == ctypes.c_void_p(-1).value:
        raise cast(OSError, _native.WinError(_native.get_last_error()))
    try:
        while True:
            if stream.name != "::$DATA":
                raise OSError(errno.ENOTSUP, "alternate data streams are unsupported", path)
            if not library.FindNextStreamW(handle, ctypes.byref(stream)):
                if _native.get_last_error() != 38:
                    raise cast(OSError, _native.WinError(_native.get_last_error()))
                break
    finally:
        library.FindClose(handle)


def _sid_text(pointer: ctypes.c_void_p) -> str:
    library = _windows()
    function = library.ConvertSidToStringSidW
    function.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_wchar_p)]
    function.restype = ctypes.c_int
    text = ctypes.c_wchar_p()
    if not function(pointer, ctypes.byref(text)):
        raise cast(OSError, _native.WinError(_native.get_last_error()))
    try:
        return str(text.value)
    finally:
        kernel = _native.WinDLL("kernel32", use_last_error=True)
        kernel.LocalFree.argtypes = [ctypes.c_void_p]
        kernel.LocalFree(text)


def principals(descriptor: bytes) -> tuple[str, str]:
    library = _windows()
    buffer = ctypes.create_string_buffer(descriptor)
    result = []
    for name in ("GetSecurityDescriptorOwner", "GetSecurityDescriptorGroup"):
        function = getattr(library, name)
        function.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_void_p),
            ctypes.POINTER(ctypes.c_int),
        ]
        function.restype = ctypes.c_int
        sid = ctypes.c_void_p()
        defaulted = ctypes.c_int()
        if not function(buffer, ctypes.byref(sid), ctypes.byref(defaulted)):
            raise cast(OSError, _native.WinError(_native.get_last_error()))
        result.append(_sid_text(sid))
    return result[0], result[1]


def ensure_private(path: Path) -> None:
    if sys.platform != "win32":
        metadata = path.lstat()
        if stat.S_IMODE(metadata.st_mode) & 0o077 or metadata.st_uid != os.geteuid():
            raise OSError(errno.ENOTSUP, "management storage must be private to its owner", path)
        security(path)
        return
    descriptor = security(path)
    owner, _ = principals(descriptor)
    library = _windows()
    buffer = ctypes.create_string_buffer(descriptor)
    acl = ctypes.c_void_p()
    present, defaulted = ctypes.c_int(), ctypes.c_int()
    function = library.GetSecurityDescriptorDacl
    function.argtypes = [
        ctypes.c_void_p,
        ctypes.POINTER(ctypes.c_int),
        ctypes.POINTER(ctypes.c_void_p),
        ctypes.POINTER(ctypes.c_int),
    ]
    function.restype = ctypes.c_int
    if not function(buffer, ctypes.byref(present), ctypes.byref(acl), ctypes.byref(defaulted)):
        raise cast(OSError, _native.WinError(_native.get_last_error()))
    if not present.value or not acl.value:
        raise OSError(errno.ENOTSUP, "private metadata requires a restrictive DACL", path)
    count = ctypes.c_ushort.from_address(acl.value + 4).value
    library.GetAce.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.POINTER(ctypes.c_void_p)]
    library.GetAce.restype = ctypes.c_int
    for index in range(count):
        ace = ctypes.c_void_p()
        if not library.GetAce(acl, index, ctypes.byref(ace)) or not ace.value:
            raise cast(OSError, _native.WinError(_native.get_last_error()))
        kind = ctypes.c_ubyte.from_address(ace.value).value
        if kind == 1:
            continue
        if kind != 0 or _sid_text(ctypes.c_void_p(ace.value + 8)) not in {
            owner,
            "S-1-5-18",
            "S-1-5-32-544",
        }:
            raise OSError(errno.ENOTSUP, "management payload DACL grants foreign access", path)
