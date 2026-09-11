import sys
from pathlib import Path
from typing import assert_type

import flyrail
from flyrail import (
    Bundle,
    BundleEntry,
    BundleIdentity,
    Installation,
    InventoryEntry,
    Observation,
    OperationStatus,
    SkillSpec,
    Target,
    TargetInspection,
    TargetResult,
    inspect,
    install,
    uninstall,
    update,
)


def consume(bundle: Bundle, target: Target) -> None:
    assert_type(bundle.identity, BundleIdentity)
    assert_type(bundle.entries, tuple[BundleEntry, ...])
    assert_type(bundle.skills, tuple[SkillSpec, ...])
    observations = inspect(bundle, [target])
    assert_type(observations, tuple[TargetInspection, ...])
    assert_type(observations[0].observation, Observation)
    assert_type(observations[0].observation.installed, Installation | None)
    installed = observations[0].observation.installed
    if installed is not None:
        assert_type(installed.entries, tuple[InventoryEntry, ...])
    results = install(bundle, [target])
    assert_type(results, tuple[TargetResult, ...])
    assert_type(results[0].status, OperationStatus)
    assert results[0].status is OperationStatus.APPLIED
    assert_type(update(bundle, [target], replace_modified=False), tuple[TargetResult, ...])
    assert_type(uninstall(bundle.id, [target]), tuple[TargetResult, ...])
    assert uninstall(bundle.id, [target])[0].status is OperationStatus.UNCHANGED


if __name__ == "__main__":
    assert Path(flyrail.__file__).is_relative_to(Path(sys.prefix))
    assert Path(flyrail.__file__).with_name("py.typed").is_file()
    consume(Bundle.from_directory("notes-bundle"), Target.directory("typed-skills"))
    print("Built-wheel typed consumer passed.")
