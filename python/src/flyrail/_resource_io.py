import errno
import os
import stat
import sys
from dataclasses import replace
from pathlib import Path
from typing import TypeVar

from flyrail._codec import decode, encode
from flyrail._filesystem import rename_exclusive
from flyrail._observation import ObservationFailure, Observer
from flyrail._resource_models import Ancestor, Node, ResourceRef, Revision
from flyrail._security import ensure_private, security, set_security, validate_windows_file
from flyrail.authority import hierarchy_conflicts
from flyrail.observations import ErrorCode

T = TypeVar("T")


def failure(code: ErrorCode, message: str, path: Path) -> ObservationFailure:
    return ObservationFailure(code, message, path)


def ancestors(path: Path) -> tuple[Ancestor, ...]:
    result = []
    observer = Observer()
    for parent in reversed(path.parents):
        current = observer.ancestor(parent)
        if current is not None:
            result.append(current)
    observer.finish()
    return tuple(result)


def observe(path: Path) -> Revision:
    observer = Observer()
    ancestors(path)
    nodes: list[Node] = []

    def walk(current: Path, relative: str) -> None:
        metadata = observer.metadata(current)
        if metadata is None:
            return
        if stat.S_IMODE(metadata.st_mode) & ~0o777:
            raise failure(ErrorCode.UNSAFE_PATH, "special permission bits are unsupported", current)
        if sys.platform == "win32":
            validate_windows_file(current)
        descriptor = security(current)
        data = None if stat.S_ISDIR(metadata.st_mode) else observer.file(current)
        nodes.append(
            Node(
                relative,
                data,
                stat.S_IMODE(metadata.st_mode),
                metadata.st_uid,
                metadata.st_gid,
                metadata.st_dev,
                metadata.st_ino,
                descriptor,
            )
        )
        if data is None:
            for child in observer.directory(current):
                walk(child, f"{relative}/{child.name}" if relative else child.name)
        if security(current) != descriptor:
            raise failure(ErrorCode.CONCURRENT_CHANGE, "security metadata changed", current)

    walk(path, "")
    observer.finish()
    return Revision(tuple(sorted(nodes, key=lambda node: node.path)))


def require(path: Path, revision: Revision) -> None:
    if observe(path) != revision:
        raise failure(ErrorCode.CONCURRENT_CHANGE, "complete resource revision changed", path)


def read(path: Path, cls: type[T]) -> T | None:
    observer = Observer()
    ancestors(path)
    if observer.metadata(path) is None:
        observer.finish()
        return None
    try:
        value = decode(observer.file(path))
        if type(value) is not cls:
            raise ValueError("metadata record has the wrong type")
    except (ValueError, UnicodeError) as error:
        raise failure(ErrorCode.INVALID_STATE, str(error), path) from error
    observer.finish()
    if not isinstance(value, cls):
        raise ValueError("metadata class disagrees")
    return value


def atomic(
    path: Path, value: object, *, expected_ancestors: tuple[Ancestor, ...] | None = None
) -> None:
    before = observe(path)
    parents = ancestors(path)
    if expected_ancestors is not None and parents != expected_ancestors:
        raise failure(ErrorCode.CONCURRENT_CHANGE, "metadata ancestor changed", path)
    temporary = path.with_name(path.name + ".next")
    if temporary.exists() or temporary.is_symlink():
        raise failure(ErrorCode.RECOVERY_NEEDED, "unrecognized metadata preparation", temporary)
    payload = encode(value)
    with temporary.open("xb") as stream:
        if os.name != "nt":
            temporary.chmod(0o600)
        ensure_private(temporary)
        stream.write(payload)
    if os.name != "nt":
        temporary.chmod(0o600)
    snapshot = observe(temporary)
    if snapshot.data != payload:
        raise failure(ErrorCode.CONCURRENT_CHANGE, "metadata staging changed", temporary)
    require(path, before)
    if ancestors(path) != parents:
        raise failure(ErrorCode.CONCURRENT_CHANGE, "metadata ancestor changed", path)
    require(temporary, snapshot)
    os.replace(temporary, path)
    require(path, snapshot)


def validate_state(ref: ResourceRef) -> tuple[Path, ...]:
    observer = Observer()
    observer.managed_directory(ref.state_root)
    recovery: list[Path] = []
    for child in observer.directory(ref.state_root):
        if child.name not in {
            "lock",
            "authority.json",
            "receipt.json",
            "transaction.json",
            "staging",
            "backup",
            "authority.json.next",
            "receipt.json.next",
            "transaction.json.next",
        }:
            raise failure(ErrorCode.INVALID_STATE, "unknown resource state entry", child)
        metadata = observer.metadata(child)
        if child.name in {"staging", "backup"}:
            if observer.directory(child):
                recovery.append(child)
        elif metadata is not None and not stat.S_ISREG(metadata.st_mode):
            raise failure(ErrorCode.INVALID_STATE, "metadata must be a regular file", child)
        if child.name == "transaction.json" or child.name.endswith(".next"):
            recovery.append(child)
    observer.finish()
    header = read(ref.state_root / "authority.json", ResourceRef)
    if header is not None and header != ref:
        raise failure(ErrorCode.INVALID_STATE, "authority identity disagrees", ref.state_root)
    return tuple(recovery)


def validate_kind(ref: ResourceRef) -> None:
    metadata = Observer().metadata(ref.destination)
    if metadata is not None and stat.S_ISREG(metadata.st_mode) != (ref.kind == "document"):
        raise failure(
            ErrorCode.UNSAFE_PATH,
            "resource kind disagrees with existing destination",
            ref.destination,
        )


def admit(ref: ResourceRef) -> None:
    authority = ref.authority
    validate_state(ref)
    header = ref.state_root / "authority.json"
    if read(header, ResourceRef) is None:
        prepared = header.with_name("authority.json.next")
        if read(prepared, ResourceRef) == ref:
            os.replace(prepared, header)
        else:
            atomic(header, ref)
    validate_kind(ref)
    conflicts = hierarchy_conflicts(authority, whole_tree=ref.kind != "document")
    if conflicts:
        raise failure(
            ErrorCode.CONFLICT, "ancestor or descendant resource fence conflicts", conflicts[0]
        )


def write_revision(path: Path, revision: Revision) -> Revision:
    for node in revision.nodes:
        destination = path / node.path if node.path else path
        if node.data is None:
            destination.mkdir(mode=0o700)
        else:
            with destination.open("xb") as stream:
                if os.name == "nt":
                    ensure_private(destination)
                stream.write(node.data)
        if os.name == "nt":
            ensure_private(destination)
            set_security(destination, security(destination), protected=True)
        else:
            metadata = destination.stat()
            if sys.platform != "win32" and (metadata.st_uid, metadata.st_gid) != (
                node.uid,
                node.gid,
            ):
                os.chown(destination, node.uid, node.gid)
            destination.chmod(node.mode)
    result = observe(path)
    for original, staged in zip(revision.nodes, result.nodes, strict=True):
        if (
            original.path != staged.path
            or original.data != staged.data
            or (
                os.name != "nt"
                and (original.mode, original.uid, original.gid)
                != (staged.mode, staged.uid, staged.gid)
            )
        ):
            raise failure(
                ErrorCode.CONCURRENT_CHANGE, "staged resource differs from intended revision", path
            )
    if os.name != "nt":
        for original, staged in zip(revision.nodes, result.nodes, strict=True):
            if original.inode and original.security != staged.security:
                raise OSError(errno.ENOTSUP, "cannot preserve resource security metadata", path)
    return result


def private_revision(revision: Revision, template: bytes) -> Revision:
    if os.name != "nt":
        return revision
    return Revision(tuple(replace(node, security=template) for node in revision.nodes))


def protect(path: Path, revision: Revision, template: bytes) -> None:
    if os.name == "nt":
        for node in revision.nodes:
            set_security(path / node.path if node.path else path, template)


def cleanup_revision(current: Revision, expected: Revision) -> bool:
    return current.nodes == expected.nodes[: len(current.nodes)]


def destroy(path: Path, revision: Revision) -> None:
    actual = observe(path)
    if not actual.nodes:
        return
    if actual != revision:
        raise failure(ErrorCode.RECOVERY_NEEDED, "cleanup revision changed", path)
    for node in reversed(revision.nodes):
        selected = path / node.path if node.path else path
        if node.data is None:
            selected.rmdir()
        else:
            selected.unlink()


def move(source: Path, destination: Path, revision: Revision) -> None:
    require(source, revision)
    rename_exclusive(source, destination)
    require(destination, revision)


def unsupported(error: OSError) -> bool:
    return error.errno in {errno.ENOTSUP, errno.ENOSYS, errno.EXDEV, errno.EINVAL}
