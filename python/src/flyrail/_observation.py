import os
import stat
from pathlib import Path

from flyrail._resource_models import Ancestor
from flyrail._security import security
from flyrail._sources import (
    _descriptor_signature,
    _path_descriptor_signature,
    _path_signature,
)
from flyrail._validation import portable_path_key
from flyrail.observations import ErrorCode, TargetError


class ObservationFailure(Exception):
    def __init__(self, code: ErrorCode, message: str, path: Path) -> None:
        super().__init__(message)
        self.error = TargetError(code, message, path)


class Observer:
    def __init__(self) -> None:
        self.observed: dict[Path, tuple[int, ...] | None] = {}
        self.ancestors: dict[Path, Ancestor | None] = {}
        self.directories: dict[Path, tuple[str, ...]] = {}
        self.matches: dict[tuple[Path, str], tuple[str, ...]] = {}
        self.listed: set[str] = set()

    def ancestor(self, path: Path) -> Ancestor | None:
        try:
            metadata = path.lstat()
        except FileNotFoundError:
            current = None
        else:
            if not stat.S_ISDIR(metadata.st_mode) or (
                getattr(metadata, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT
            ):
                raise ObservationFailure(
                    ErrorCode.UNSAFE_PATH, "resource ancestor is not an ordinary directory", path
                )
            current = Ancestor(
                path,
                metadata.st_dev,
                metadata.st_ino,
                stat.S_IMODE(metadata.st_mode),
                metadata.st_uid,
                metadata.st_gid,
                security(path, ancestor=True),
            )
        if path in self.ancestors and self.ancestors[path] != current:
            raise ObservationFailure(
                ErrorCode.CONCURRENT_CHANGE, "ancestor changed during inspection", path
            )
        self.ancestors[path] = current
        return current

    def metadata(self, path: Path) -> os.stat_result | None:
        try:
            metadata = path.lstat()
        except FileNotFoundError:
            metadata = None
        signature = None if metadata is None else _path_signature(metadata)
        if metadata is None and os.fspath(path) in self.listed:
            raise ObservationFailure(
                ErrorCode.CONCURRENT_CHANGE, "listed path disappeared during inspection", path
            )
        if path in self.observed and self.observed[path] != signature:
            raise ObservationFailure(
                ErrorCode.CONCURRENT_CHANGE, "path changed during inspection", path
            )
        self.observed[path] = signature
        if metadata is not None and (
            getattr(metadata, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT
            or not (stat.S_ISREG(metadata.st_mode) or stat.S_ISDIR(metadata.st_mode))
            or (stat.S_ISREG(metadata.st_mode) and metadata.st_nlink != 1)
        ):
            raise ObservationFailure(
                ErrorCode.UNSAFE_PATH,
                "managed symlinks, reparse points and special files are unsupported",
                path,
            )
        return metadata

    def directory(self, path: Path, *, matching: str | None = None) -> tuple[Path, ...]:
        if matching is None:
            metadata = self.metadata(path)
            present = metadata is not None
            if metadata is not None and not stat.S_ISDIR(metadata.st_mode):
                raise ObservationFailure(ErrorCode.UNSAFE_PATH, "expected a directory", path)
        else:
            present = self.ancestor(path) is not None
        try:
            children = tuple(path.iterdir()) if present else ()
        except FileNotFoundError as error:
            raise ObservationFailure(
                ErrorCode.CONCURRENT_CHANGE, "directory disappeared during inspection", path
            ) from error
        if matching is not None:
            key = portable_path_key(matching)
            children = tuple(child for child in children if portable_path_key(child.name) == key)
        children = tuple(
            sorted(
                children,
                key=lambda child: child.name.encode("utf-8", errors="surrogatepass"),
            )
        )
        keys: set[str] = set()
        for child in children:
            key = portable_path_key(child.name)
            if key in keys:
                raise ObservationFailure(
                    ErrorCode.UNSAFE_PATH, "portable filename collision", child
                )
            keys.add(key)
            self.listed.add(os.fspath(child))
        names = tuple(child.name for child in children)
        if matching is None:
            self.metadata(path)
            if path in self.directories and self.directories[path] != names:
                raise ObservationFailure(
                    ErrorCode.CONCURRENT_CHANGE, "directory entries changed", path
                )
            self.directories[path] = names
        else:
            self.ancestor(path)
            match_key = (path, matching)
            if match_key in self.matches and self.matches[match_key] != names:
                raise ObservationFailure(
                    ErrorCode.CONCURRENT_CHANGE, "selected directory entries changed", path
                )
            self.matches[match_key] = names
        return children

    def managed_directory(self, path: Path) -> None:
        matches = self.directory(path.parent, matching=path.name)
        if matches and matches[0].name != path.name:
            raise ObservationFailure(
                ErrorCode.UNSAFE_PATH, "managed directory spelling mismatch", path
            )
        metadata = self.metadata(path)
        if metadata is not None and not stat.S_ISDIR(metadata.st_mode):
            raise ObservationFailure(ErrorCode.UNSAFE_PATH, "expected a directory", path)

    def file(self, path: Path) -> bytes:
        before = self.metadata(path)
        if before is None or not stat.S_ISREG(before.st_mode):
            raise ObservationFailure(ErrorCode.INVALID_STATE, "expected a regular file", path)
        try:
            stream = path.open("rb")
        except FileNotFoundError as error:
            raise ObservationFailure(
                ErrorCode.CONCURRENT_CHANGE, "file disappeared while opening", path
            ) from error
        with stream:
            opened = os.fstat(stream.fileno())
            if _path_descriptor_signature(opened) != _path_descriptor_signature(before):
                raise ObservationFailure(
                    ErrorCode.CONCURRENT_CHANGE, "file changed while opening", path
                )
            data = stream.read()
            if _descriptor_signature(os.fstat(stream.fileno())) != _descriptor_signature(opened):
                raise ObservationFailure(
                    ErrorCode.CONCURRENT_CHANGE, "file changed while reading", path
                )
        self.metadata(path)
        return data

    def finish(self) -> None:
        for path in self.observed:
            self.metadata(path)
        for path in self.ancestors:
            self.ancestor(path)
        for path in self.directories:
            self.directory(path)
        for path, matching in self.matches:
            self.directory(path, matching=matching)
