import errno
import os
import sys
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest
from test_configuration_lifecycle import bundle, rendered

from flyrail import InstallationTarget, OperationStatus, ResourceAuthority, remove, sync
from flyrail import _resource_io as io
from flyrail import _resource_transaction as tx
from flyrail._resource_models import Receipt, Revision


@pytest.mark.parametrize(
    "seam",
    [
        "success",
        "write-dac",
        "narrowed",
        "before-replace",
        "1175",
        "1176",
        "1177",
        "published-private",
        "published-restored",
        "descriptor-edit",
        "backup-edit",
    ],
)
def test_windows_document_security_transitions_keep_backup_private(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, seam: str
) -> None:
    destination = tmp_path / "AGENTS.md"
    destination.write_bytes(b"foreign")
    target = InstallationTarget(tmp_path / "index")
    descriptors = {destination.stat().st_ino: b"public"}
    calls: list[str] = []
    armed = True

    def descriptor(path: Path, *, ancestor: bool = False) -> bytes:
        return descriptors.get(path.stat().st_ino, b"private")

    def apply(path: Path, value: bytes, *, protected: bool | None = None) -> None:
        nonlocal armed
        if path == destination and value == b"private" and path.read_bytes() == b"foreign":
            calls.append("narrow")
            if armed and seam == "write-dac":
                armed = False
                raise OSError(errno.EACCES, "WRITE_DAC unavailable", path)
        descriptors[path.stat().st_ino] = value
        if armed and path == destination:
            if seam == "narrowed" and value == b"private" and path.read_bytes() == b"foreign":
                armed = False
                raise SystemExit("crash after narrowing")
            if (
                seam == "published-restored"
                and value == b"public"
                and path.read_bytes() != b"foreign"
            ):
                armed = False
                raise SystemExit("crash after restoring publication security")
            if seam == "descriptor-edit" and value == b"private":
                descriptors[path.stat().st_ino] = b"external descriptor"
                armed = False

    def replace_file(path: Path, staged: Path, backup: Path) -> None:
        nonlocal armed
        calls.append("replace")
        assert descriptor(path) == descriptor(staged) == b"private"
        if armed and seam == "before-replace":
            armed = False
            raise SystemExit("crash before replacement")
        if seam == "1175":
            raise OSError(errno.EIO, "1175")
        path.rename(backup)
        assert descriptor(backup) == b"private"
        if seam in {"1176", "1177"}:
            raise OSError(errno.EIO, seam)
        staged.rename(path)
        if armed and seam == "published-private":
            armed = False
            raise SystemExit("crash after replacement")
        if seam == "backup-edit":
            backup.write_bytes(b"external backup edit")

    def private(path: Path) -> None:
        if descriptor(path) != b"private":
            raise OSError(errno.ENOTSUP, "foreign payload access", path)

    with monkeypatch.context() as patch:
        patch.setattr(io, "ensure_private", private)
        patch.setattr(tx, "ensure_private", private)
        patch.setattr(io, "os", SimpleNamespace(name="nt", replace=os.replace))
        patch.setattr(tx, "os", SimpleNamespace(name="nt", replace=os.replace))
        patch.setattr(io, "security", descriptor)
        patch.setattr(io, "set_security", apply)
        patch.setattr(tx, "set_security", apply)
        patch.setattr(tx, "principals", lambda value: ("owner", "group"))
        patch.setattr(tx, "replace_file", replace_file)
        if seam in {"narrowed", "before-replace", "published-private", "published-restored"}:
            with pytest.raises(SystemExit):
                sync(bundle(), rendered(destination), target)
            result = remove("team", target)
            assert result.status is OperationStatus.APPLIED
            assert destination.read_bytes() == b"foreign"
            assert descriptor(destination) == b"public"
        else:
            result = sync(bundle(), rendered(destination), target)
            if seam == "success":
                assert result.status is OperationStatus.APPLIED
                assert b"generated" in destination.read_bytes()
                assert descriptor(destination) == b"public"
                assert remove("team", target).status is OperationStatus.APPLIED
                assert destination.read_bytes() == b"foreign"
                assert descriptor(destination) == b"public"
            elif seam in {"descriptor-edit", "backup-edit"}:
                assert result.status is OperationStatus.INCOMPLETE
                assert result.resources[0].recovery_paths
                if seam == "descriptor-edit":
                    assert destination.read_bytes() == b"foreign"
                    assert descriptor(destination) == b"external descriptor"
                    assert calls == ["narrow"]
                else:
                    assert (
                        ResourceAuthority(destination).state_root / "backup/resource"
                    ).read_bytes() == b"external backup edit"
            else:
                assert result.status is OperationStatus.FAILED
                assert destination.read_bytes() == b"foreign"
                assert descriptor(destination) == b"public"
                if seam == "write-dac":
                    assert calls == ["narrow"]
        if seam not in {"descriptor-edit", "backup-edit"}:
            assert not (ResourceAuthority(destination).state_root / "transaction.json").exists()


@pytest.mark.skipif(sys.platform != "win32", reason="native Windows sharing and DACL transitions")
@pytest.mark.parametrize("seam", ["sharing", "narrowed", "published-private", "published-restored"])
def test_native_windows_shared_file_interruption_recovers_original_security(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, seam: str
) -> None:
    import ctypes

    from flyrail import _security as security

    native: Any = ctypes
    destination = tmp_path / "AGENTS.md"
    destination.write_bytes(b"foreign")
    original = security.security(destination)
    target = InstallationTarget(tmp_path / "index")
    held = None
    if seam == "sharing":
        library = native.WinDLL("kernel32", use_last_error=True)
        library.CreateFileW.argtypes = [
            ctypes.c_wchar_p,
            ctypes.c_uint,
            ctypes.c_uint,
            ctypes.c_void_p,
            ctypes.c_uint,
            ctypes.c_uint,
            ctypes.c_void_p,
        ]
        library.CreateFileW.restype = ctypes.c_void_p
        library.CloseHandle.argtypes = [ctypes.c_void_p]
        held = library.CreateFileW(str(destination), 0x80000000, 1 | 2, None, 3, 0, None)
        assert held != ctypes.c_void_p(-1).value
        try:
            result = sync(bundle(), rendered(destination), target)
            assert result.status in {OperationStatus.FAILED, OperationStatus.INCOMPLETE}
            assert destination.read_bytes() == b"foreign"
        finally:
            library.CloseHandle(held)
    else:
        original_protect = io.protect
        original_replace = security.replace_file
        original_set = security.set_security

        def protect(path: Path, revision: Revision, template: bytes) -> None:
            original_protect(path, revision, template)
            if seam == "narrowed":
                raise SystemExit("interrupted restricted destination")

        def replace_file(path: Path, staged: Path, backup: Path) -> None:
            security.ensure_private(path)
            original_replace(path, staged, backup)
            security.ensure_private(backup)
            if seam == "published-private":
                raise SystemExit("interrupted private publication")

        def set_security(path: Path, value: bytes) -> None:
            original_set(path, value)
            if seam == "published-restored":
                raise SystemExit("interrupted restored publication")

        with monkeypatch.context() as patch:
            patch.setattr(tx, "protect", protect)
            patch.setattr(tx, "replace_file", replace_file)
            patch.setattr(tx, "set_security", set_security)
            with pytest.raises(SystemExit):
                sync(bundle(), rendered(destination), target)
        state = ResourceAuthority(destination).state_root
        assert io.read(state / "receipt.json", Receipt) is None
    assert remove("team", target).status is OperationStatus.APPLIED
    assert destination.read_bytes() == b"foreign"
    assert security.security(destination) == original
