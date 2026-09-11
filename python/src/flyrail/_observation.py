import os
import stat
from pathlib import Path

from flyrail._sources import _signature
from flyrail._validation import portable_path_key, validate_relative_path
from flyrail.models import BundleEntry
from flyrail.observations import ErrorCode, TargetError


class ObservationFailure(Exception):
    def __init__(self, code: ErrorCode, message: str, path: Path) -> None:
        super().__init__(message)
        self.error = TargetError(code, message, path)


class Observer:
    def __init__(self) -> None:
        self.observed: dict[Path, tuple[int, ...] | None] = {}

    def metadata(self, path: Path) -> os.stat_result | None:
        try:
            metadata = path.lstat()
        except FileNotFoundError:
            metadata = None
        signature = None if metadata is None else _signature(metadata)
        if path in self.observed and self.observed[path] != signature:
            raise ObservationFailure(
                ErrorCode.CONCURRENT_CHANGE, "path changed during inspection", path
            )
        self.observed[path] = signature
        if metadata is not None and (
            getattr(metadata, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT
            or not (stat.S_ISREG(metadata.st_mode) or stat.S_ISDIR(metadata.st_mode))
        ):
            raise ObservationFailure(
                ErrorCode.UNSAFE_PATH,
                "managed symlinks, reparse points and special files are unsupported",
                path,
            )
        return metadata

    def directory(self, path: Path, *, matching: str | None = None) -> tuple[Path, ...]:
        metadata = self.metadata(path)
        if metadata is None:
            return ()
        if not stat.S_ISDIR(metadata.st_mode):
            raise ObservationFailure(ErrorCode.UNSAFE_PATH, "expected a directory", path)
        children = tuple(
            sorted(
                (
                    child
                    for child in path.iterdir()
                    if matching is None
                    or portable_path_key(child.name) == portable_path_key(matching)
                ),
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
        self.metadata(path)
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
        with path.open("rb") as stream:
            if _signature(os.fstat(stream.fileno())) != _signature(before):
                raise ObservationFailure(
                    ErrorCode.CONCURRENT_CHANGE, "file changed while opening", path
                )
            data = stream.read()
            if _signature(os.fstat(stream.fileno())) != _signature(before):
                raise ObservationFailure(
                    ErrorCode.CONCURRENT_CHANGE, "file changed while reading", path
                )
        self.metadata(path)
        return data

    def tree(self, root: Path, name: str, executables: frozenset[str]) -> tuple[BundleEntry, ...]:
        entries: list[BundleEntry] = []

        def walk(path: Path, relative: str) -> None:
            metadata = self.metadata(path)
            if metadata is None:
                return
            try:
                validate_relative_path(relative, "installed path")
            except ValueError as error:
                raise ObservationFailure(ErrorCode.UNSAFE_PATH, str(error), path) from error
            if stat.S_ISDIR(metadata.st_mode):
                entries.append(BundleEntry(relative))
                for child in self.directory(path):
                    walk(child, relative + "/" + child.name)
            else:
                executable = (
                    relative in executables if os.name == "nt" else bool(metadata.st_mode & 0o111)
                )
                entries.append(BundleEntry(relative, self.file(path), executable))

        walk(root / name, name)
        return tuple(entries)

    def finish(self) -> None:
        for path in self.observed:
            self.metadata(path)
