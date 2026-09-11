import os
import stat
import zipfile
from pathlib import Path

from flyrail._validation import portable_path_key, validate_relative_path
from flyrail.models import BundleEntry, SkillSpec


def _ordinary(metadata: os.stat_result, path: Path) -> None:
    if getattr(metadata, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT:
        raise ValueError(f"source reparse points are not supported: {path}")
    if not (stat.S_ISREG(metadata.st_mode) or stat.S_ISDIR(metadata.st_mode)):
        raise ValueError(f"source symlinks and special files are not supported: {path}")


def _signature(metadata: os.stat_result) -> tuple[int, ...]:
    return (
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_mode,
        metadata.st_size,
        metadata.st_mtime_ns,
        metadata.st_ctime_ns,
    )


class DirectorySource:
    def __init__(self, root: Path) -> None:
        _ordinary(root.lstat(), root)
        self.root = root.resolve(strict=True)
        self.observed: dict[Path, tuple[int, ...]] = {}
        if not stat.S_ISDIR(self._observe(self.root).st_mode):
            raise ValueError("bundle root must be a directory")

    def _observe(self, path: Path) -> os.stat_result:
        metadata = path.lstat()
        _ordinary(metadata, path)
        signature = _signature(metadata)
        if path in self.observed and self.observed[path] != signature:
            raise ValueError(f"bundle source changed while loading: {path}")
        self.observed[path] = signature
        return metadata

    def _find(self, relative: str) -> Path:
        parent = self.root
        for part in relative.split("/"):
            self._observe(parent)
            matches = [
                child
                for child in parent.iterdir()
                if portable_path_key(child.name) == portable_path_key(part)
            ]
            if len(matches) != 1 or matches[0].name != part:
                raise ValueError(f"missing, case-mismatched or ambiguous source path: {relative}")
            parent = matches[0]
            self._observe(parent)
        return parent

    def _read(self, path: Path) -> bytes:
        before = self._observe(path)
        if not stat.S_ISREG(before.st_mode):
            raise ValueError(f"expected a source file: {path}")
        with path.open("rb") as stream:
            if _signature(os.fstat(stream.fileno())) != _signature(before):
                raise ValueError(f"bundle source changed while opening: {path}")
            data = stream.read()
            if _signature(os.fstat(stream.fileno())) != _signature(before):
                raise ValueError(f"bundle source changed while reading: {path}")
        self._observe(path)
        return data

    def read_manifest(self) -> bytes:
        return self._read(self._find("flyrail.json"))

    def skill_entries(self, spec: SkillSpec) -> tuple[BundleEntry, ...]:
        root = self._find(spec.path)
        if not stat.S_ISDIR(self._observe(root).st_mode):
            raise ValueError(f"skill source must be a directory: {spec.path}")
        entries: list[BundleEntry] = []

        def walk(directory: Path, relative: str) -> None:
            self._observe(directory)
            entries.append(BundleEntry(relative))
            seen: set[str] = set()
            for child in directory.iterdir():
                validate_relative_path(child.name, "source name")
                key = portable_path_key(child.name)
                if key in seen:
                    raise ValueError(f"portable source name collision: {child}")
                seen.add(key)
                path = f"{relative}/{child.name}"
                if stat.S_ISDIR(self._observe(child).st_mode):
                    walk(child, path)
                else:
                    executable = child.relative_to(root).as_posix() in spec.executables
                    entries.append(BundleEntry(path, self._read(child), executable))
            self._observe(directory)

        walk(root, spec.name)
        return tuple(entries)

    def finish(self) -> None:
        for path in self.observed:
            self._observe(path)


class ZipSource:
    def __init__(self, archive: zipfile.ZipFile, root: str) -> None:
        self.archive = archive
        self.root = root.rstrip("/")
        self.entries: dict[str, zipfile.ZipInfo | None] = {}
        explicit: set[str] = set()
        portable: dict[str, str] = {}
        root_key = portable_path_key(self.root)
        for info in archive.infolist():
            name = info.filename.rstrip("/")
            name_key = portable_path_key(name)
            parts = name.split("/")
            for index in range(1, len(parts) + 1):
                path = "/".join(parts[:index])
                key = portable_path_key(path)
                if not (
                    key == root_key
                    or key.startswith(root_key + "/")
                    or root_key.startswith(key + "/")
                ):
                    continue
                validate_relative_path(path, "archive path")
                if key in portable and portable[key] != path:
                    raise ValueError(f"portable archive name collision: {path}")
                portable[key] = path
            if not (
                name_key == root_key
                or name_key.startswith(root_key + "/")
                or root_key.startswith(name_key + "/")
            ):
                continue
            if info.orig_filename != info.filename:
                raise ValueError(f"ambiguous archive entry: {info.orig_filename!r}")
            if info.filename != name + ("/" if info.is_dir() else "") or name in explicit:
                raise ValueError(f"duplicate or ambiguous archive entry: {info.filename}")
            explicit.add(name)
            mode = info.external_attr >> 16
            kind = stat.S_IFMT(mode) if info.create_system == 3 else 0
            if kind not in {0, stat.S_IFDIR if info.is_dir() else stat.S_IFREG}:
                raise ValueError(f"archive symlinks and special files are not supported: {name}")
            if info.external_attr & stat.FILE_ATTRIBUTE_REPARSE_POINT:
                raise ValueError(f"archive reparse points are not supported: {name}")
            for index in range(1, len(parts) + 1):
                path = "/".join(parts[:index])
                is_file = index == len(parts) and not info.is_dir()
                if path in self.entries and (is_file or self.entries[path] is not None):
                    raise ValueError(f"archive file/directory collision: {path}")
                self.entries[path] = info if is_file else None
        if self.root not in self.entries or self.entries[self.root] is not None:
            raise ValueError("package resource must be an existing directory")

    def read_manifest(self) -> bytes:
        path = self.root + "/flyrail.json"
        info = self.entries.get(path)
        if info is None:
            raise ValueError("package resource must contain a flyrail.json file")
        return self.archive.read(info)

    def skill_entries(self, spec: SkillSpec) -> tuple[BundleEntry, ...]:
        root = self.root + "/" + spec.path
        if root not in self.entries or self.entries[root] is not None:
            raise ValueError(f"skill source must be an existing directory: {spec.path}")
        result: list[BundleEntry] = []
        for path, info in self.entries.items():
            if path == root:
                result.append(BundleEntry(spec.name))
            elif path.startswith(root + "/"):
                relative = path[len(root) + 1 :]
                data = None if info is None else self.archive.read(info)
                result.append(
                    BundleEntry(spec.name + "/" + relative, data, relative in spec.executables)
                )
        return tuple(result)
