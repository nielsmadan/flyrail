from dataclasses import dataclass, field
from enum import StrEnum
from importlib.resources import files
from pathlib import Path

from flyrail.models import BundleEntry


class HookShell(StrEnum):
    POSIX = "posix"
    CMD = "cmd"


@dataclass(frozen=True, slots=True)
class HookRuntime:
    node: Path | None = None
    shell: HookShell | None = None
    resources: tuple[BundleEntry, ...] = field(init=False, repr=False)

    def __post_init__(self) -> None:
        if self.node is not None and (
            not isinstance(self.node, Path)
            or not self.node.is_absolute()
            or self.node == self.node.parent
            or ".." in self.node.parts
            or any(ord(character) < 32 for character in str(self.node))
        ):
            raise ValueError("hook runtime requires an absolute Node 24+ executable path")
        if self.shell is not None and not isinstance(self.shell, HookShell):
            raise TypeError("hook shell must be a HookShell")
        source = files("flyrail").joinpath("runtime")
        object.__setattr__(
            self,
            "resources",
            tuple(
                BundleEntry(name, source.joinpath(name).read_bytes())
                for name in ("runner.mts", "bridge.mts", "process.mts")
            ),
        )
