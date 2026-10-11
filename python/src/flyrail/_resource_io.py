import errno
import os
import stat
import sys
from dataclasses import replace
from pathlib import Path

from flyrail._filesystem import rename_exclusive
from flyrail._observation import ObservationFailure, Observer
from flyrail._resource_models import Ancestor, Node, ResourceRef, Revision
from flyrail._security import ensure_private, security, set_security, validate_windows_file
from flyrail._state import hierarchy_conflicts, read_record, resource_key, write_record
from flyrail.observations import ErrorCode
from flyrail.stores import StateStore


def failure(code: ErrorCode, message: str, path: Path | None) -> ObservationFailure:
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


def validate_kind(ref: ResourceRef) -> None:
    metadata = Observer().metadata(ref.destination)
    if metadata is not None and stat.S_ISREG(metadata.st_mode) != (ref.kind == "document"):
        raise failure(
            ErrorCode.UNSAFE_PATH,
            "resource kind disagrees with existing destination",
            ref.destination,
        )


def admit(store: StateStore, ref: ResourceRef) -> None:
    key = resource_key(ref.destination, "authority")
    current = read_record(store, key, ResourceRef)
    if current is None:
        write_record(store, key, ref, expected=None)
    elif current != ref:
        raise failure(ErrorCode.INVALID_STATE, "authority identity disagrees", ref.destination)
    validate_kind(ref)
    conflicts = hierarchy_conflicts(store, ref.authority, whole_tree=ref.kind != "document")
    if conflicts:
        raise failure(ErrorCode.CONFLICT, "ancestor or descendant resource conflicts", conflicts[0])


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
