import ctypes
import errno
import os
import stat
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace
from typing import Any
from unittest.mock import Mock

import pytest
from test_configuration_lifecycle import bundle, rendered

from flyrail import InstallationTarget, OperationStatus, remove, sync
from flyrail import _security as security
from flyrail._resource_io import observe


def fake_windows(monkeypatch: pytest.MonkeyPatch, library: Any) -> Any:
    native = SimpleNamespace(
        WinDLL=Mock(return_value=library),
        get_last_error=Mock(return_value=5),
        WinError=lambda code: OSError(errno.EACCES, str(code)),
    )
    monkeypatch.setattr(security, "_native", native)
    monkeypatch.setattr(security, "sys", SimpleNamespace(platform="win32"))
    return native


@pytest.mark.parametrize("failure", ["none", "size", "read"])
def test_windows_descriptor_snapshot_checks_native_size_and_failure(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, failure: str
) -> None:
    def get(path: str, flags: int, buffer: Any, size: int, needed: Any) -> int:
        assert flags == 7
        if failure == "size":
            return 0
        needed._obj.value = 4
        if buffer is None:
            return 0
        if failure == "read":
            return 0
        ctypes.memmove(buffer, b"dacl", 4)
        return 1

    library = SimpleNamespace(GetFileSecurityW=Mock(side_effect=get))
    fake_windows(monkeypatch, library)
    if failure != "none":
        with pytest.raises(OSError):
            security.security(tmp_path / "resource")
    else:
        assert security.security(tmp_path / "resource") == b"dacl"


@pytest.mark.parametrize("operation", ["protect", "replace"])
@pytest.mark.parametrize("success", [True, False])
def test_windows_security_and_replace_calls_never_ignore_acl_errors(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, operation: str, success: bool
) -> None:
    function = Mock(return_value=int(success))
    library = SimpleNamespace(
        SetFileSecurityW=function,
        ReplaceFileW=function,
        GetSecurityDescriptorControl=Mock(return_value=1),
    )
    fake_windows(monkeypatch, library)

    def call() -> None:
        if operation == "protect":
            security.set_security(tmp_path / "file", b"dacl")
        else:
            security.replace_file(tmp_path / "file", tmp_path / "stage", tmp_path / "backup")

    if success:
        call()
    else:
        with pytest.raises(OSError):
            call()
    args = function.call_args.args
    assert args[1] == 0x20000004 if operation == "protect" else args[3] == 0


@pytest.mark.parametrize(
    "case",
    [
        "normal",
        "readonly",
        "compressed",
        "encrypted",
        "sparse",
        "directory",
        "stream",
        "first-fail",
        "next-fail",
    ],
)
def test_windows_attribute_and_stream_boundary(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, case: str
) -> None:
    path = tmp_path / "file"
    path.write_bytes(b"data")
    attributes = {
        "readonly": stat.FILE_ATTRIBUTE_READONLY,
        "compressed": stat.FILE_ATTRIBUTE_COMPRESSED,
        "encrypted": stat.FILE_ATTRIBUTE_ENCRYPTED,
        "sparse": stat.FILE_ATTRIBUTE_SPARSE_FILE,
    }.get(case, 0)
    monkeypatch.setattr(
        Path,
        "lstat",
        lambda self: SimpleNamespace(
            st_mode=stat.S_IFDIR if case == "directory" else stat.S_IFREG,
            st_file_attributes=attributes,
        ),
    )

    def first(path: str, level: int, output: Any, flags: int) -> int | None:
        output._obj.name = ":secret:$DATA" if case == "stream" else "::$DATA"
        return ctypes.c_void_p(-1).value if case == "first-fail" else 42

    library = SimpleNamespace(
        FindFirstStreamW=Mock(side_effect=first),
        FindNextStreamW=Mock(return_value=0),
        FindClose=Mock(),
    )
    native = fake_windows(monkeypatch, library)
    native.get_last_error.return_value = 5 if case == "next-fail" else 38
    if case in {"normal", "directory"}:
        security.validate_windows_file(path)
    else:
        with pytest.raises(OSError):
            security.validate_windows_file(path)
    if case in {"normal", "stream", "next-fail"}:
        library.FindClose.assert_called_once_with(42)


@pytest.mark.parametrize("case", ["ok", "failure"])
def test_windows_sid_strings_release_native_memory(
    monkeypatch: pytest.MonkeyPatch, case: str
) -> None:
    def convert(pointer: Any, output: Any) -> int:
        output._obj.value = "S-1-5-18"
        return int(case == "ok")

    library = SimpleNamespace(ConvertSidToStringSidW=Mock(side_effect=convert), LocalFree=Mock())
    fake_windows(monkeypatch, library)
    if case == "ok":
        assert security._sid_text(ctypes.c_void_p(1)) == "S-1-5-18"
        assert library.LocalFree.call_count == 1
    else:
        with pytest.raises(OSError):
            security._sid_text(ctypes.c_void_p(1))


@pytest.mark.parametrize("case", ["ok", "owner-fail", "group-fail"])
def test_windows_principal_binding(monkeypatch: pytest.MonkeyPatch, case: str) -> None:
    def owner(buffer: Any, sid: Any, defaulted: Any) -> int:
        sid._obj.value = 1
        return int(case != "owner-fail")

    def group(buffer: Any, sid: Any, defaulted: Any) -> int:
        sid._obj.value = 2
        return int(case != "group-fail")

    library = SimpleNamespace(
        GetSecurityDescriptorOwner=Mock(side_effect=owner),
        GetSecurityDescriptorGroup=Mock(side_effect=group),
    )
    fake_windows(monkeypatch, library)
    monkeypatch.setattr(security, "_sid_text", lambda pointer: str(pointer.value))
    if case == "ok":
        assert security.principals(b"descriptor") == ("1", "2")
    else:
        with pytest.raises(OSError):
            security.principals(b"descriptor")


@pytest.mark.parametrize(
    "case",
    [
        "private",
        "owner-rights",
        "deny",
        "foreign",
        "unknown-ace",
        "missing",
        "null",
        "dacl-fail",
        "ace-fail",
    ],
)
def test_private_backup_dacl_is_checked_on_the_payload_not_only_its_parent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, case: str
) -> None:
    acl = ctypes.create_string_buffer(b"\2\0\10\0\1\0\0\0")
    ace = ctypes.create_string_buffer(
        bytes([1 if case == "deny" else 5 if case == "unknown-ace" else 0]) + b"\0" * 15
    )

    def dacl(buffer: Any, present: Any, output: Any, defaulted: Any) -> int:
        present._obj.value = int(case != "missing")
        output._obj.value = 0 if case == "null" else ctypes.addressof(acl)
        return int(case != "dacl-fail")

    def get_ace(pointer: Any, index: int, output: Any) -> int:
        output._obj.value = ctypes.addressof(ace)
        return int(case != "ace-fail")

    library = SimpleNamespace(
        GetSecurityDescriptorDacl=Mock(side_effect=dacl), GetAce=Mock(side_effect=get_ace)
    )
    fake_windows(monkeypatch, library)
    monkeypatch.setattr(security, "security", lambda path: b"dacl")
    monkeypatch.setattr(security, "principals", lambda descriptor: ("owner", "group"))
    monkeypatch.setattr(
        security,
        "_sid_text",
        lambda pointer: (
            "world" if case == "foreign" else "S-1-3-4" if case == "owner-rights" else "owner"
        ),
    )
    if case in {"private", "owner-rights", "deny"}:
        security.ensure_private(tmp_path / "backup")
    else:
        with pytest.raises(OSError):
            security.ensure_private(tmp_path / "backup")


@pytest.mark.skipif(sys.platform != "darwin", reason="native macOS metadata")
def test_native_macos_acl_and_foreign_xattr_refuse_before_publication(tmp_path: Path) -> None:
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"foreign")
    target = InstallationTarget(tmp_path / "index")
    for command, cleanup in [
        (["chmod", "+a", "everyone allow read", str(path)], ["chmod", "-N", str(path)]),
        (
            ["xattr", "-w", "flyrail.test", "value", str(path)],
            ["xattr", "-d", "flyrail.test", str(path)],
        ),
    ]:
        subprocess.run(command, check=True, capture_output=True)  # noqa: S603
        try:
            result = sync(bundle(), rendered(path), target)
            assert result.status is OperationStatus.FAILED and result.error is not None
            assert path.read_bytes() == b"foreign"
        finally:
            subprocess.run(cleanup, check=True, capture_output=True)  # noqa: S603
    before = observe(path)
    assert sync(bundle(), rendered(path), target).status is OperationStatus.APPLIED
    assert observe(path).nodes[0].security == before.nodes[0].security
    assert remove("team", target).status is OperationStatus.APPLIED
    assert observe(path).nodes[0].security == before.nodes[0].security


@pytest.mark.skipif(sys.platform != "linux", reason="native Linux metadata")
def test_native_linux_xattr_refuses_before_publication(tmp_path: Path) -> None:
    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"foreign")
    if sys.platform != "linux":
        pytest.skip("Linux xattr API")
    os.setxattr(path, "user.flyrail", b"private")
    result = sync(bundle(), rendered(path), InstallationTarget(tmp_path / "index"))
    assert (
        result.status is OperationStatus.FAILED and os.getxattr(path, "user.flyrail") == b"private"
    )
    assert path.read_bytes() == b"foreign"


@pytest.mark.skipif(sys.platform != "win32", reason="native Windows descriptors and ReplaceFileW")
def test_native_windows_shared_file_security_preservation_and_private_backup(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from flyrail import _lifecycle as lifecycle
    from flyrail import _resource_transaction as transaction
    from flyrail._resource_transaction import Journal, paths

    path = tmp_path / "AGENTS.md"
    path.write_bytes(b"foreign")
    before = security.security(path)
    replacements: list[tuple[int, int, int, int]] = []
    original_replace = security.replace_file

    def replace_file(destination: Path, staged: Path, backup: Path) -> None:
        destination_id = destination.stat().st_ino
        staged_id = staged.stat().st_ino
        original_replace(destination, staged, backup)
        replacements.append(
            (destination_id, staged_id, destination.stat().st_ino, backup.stat().st_ino)
        )

    monkeypatch.setattr(transaction, "replace_file", replace_file)
    target = InstallationTarget(tmp_path / "index")
    initial = sync(bundle(), rendered(path), target)
    assert initial.status is OperationStatus.APPLIED, initial.error
    assert len(replacements) == 1
    destination_id, staged_id, published_id, backup_id = replacements[0]
    assert (published_id, backup_id) == (staged_id, destination_id)
    assert security.security(path) == before
    backups = []

    def retain(journal: Journal) -> None:
        backup = paths(journal.resource)[2]
        security.ensure_private(backup)
        backups.append(security.security(backup))
        raise OSError(errno.EIO, "retain backup")

    monkeypatch.setattr(lifecycle, "cleanup", retain)
    result = sync(bundle("2"), rendered(path, "next"), target)
    assert result.resources[0].status is OperationStatus.APPLIED and backups
    assert security.security(path) == before
    path.chmod(0o400)
    try:
        result = sync(bundle("3"), rendered(path, "third"), target)
        assert result.status is OperationStatus.FAILED
        assert b"next" in path.read_bytes()
    finally:
        path.chmod(0o600)
