from collections.abc import Iterable

from flyrail.content import snapshot_entries
from flyrail.models import BundleEntry, SkillSpec


class MemorySource:
    def __init__(self, manifest: bytes, entries: Iterable[BundleEntry]) -> None:
        self.manifest = manifest
        self.entries = {entry.path: entry for entry in snapshot_entries(entries)}

    def read_manifest(self) -> bytes:
        return self.manifest

    def read_file(self, path: str) -> bytes:
        entry = self.entries.get(path)
        if entry is None or entry.data is None:
            raise ValueError(f"expected a source file: {path}")
        return entry.data

    def skill_entries(self, spec: SkillSpec) -> tuple[BundleEntry, ...]:
        return self.tree_entries(spec.path, spec.name, spec.executables)

    def tree_entries(
        self, path: str, name: str, executables: tuple[str, ...]
    ) -> tuple[BundleEntry, ...]:
        root = self.entries.get(path)
        if root is None or not root.is_directory:
            raise ValueError(f"skill source must be an existing directory: {path}")
        result = [BundleEntry(name)]
        for entry in self.entries.values():
            if entry.path.startswith(path + "/"):
                relative = entry.path[len(path) + 1 :]
                result.append(
                    BundleEntry(name + "/" + relative, entry.data, relative in executables)
                )
        return tuple(result)
