from pathlib import Path
from typing import Any

from hatchling.builders.hooks.plugin.interface import BuildHookInterface
from hatchling.builders.sdist import SdistBuilderConfig


class CustomBuildHook(BuildHookInterface[SdistBuilderConfig]):
    def initialize(self, version: str, build_data: dict[str, Any]) -> None:
        root = Path(self.root)
        fixtures = root / "spec/fixtures"
        if not fixtures.is_dir():
            fixtures = root.parent / "spec/fixtures"
        for name in (
            "bundles.json",
            "configurations.json",
            "edits.json",
            "translations.json",
            "hooks.json",
        ):
            build_data["force_include"][str(fixtures / name)] = f"spec/fixtures/{name}"
