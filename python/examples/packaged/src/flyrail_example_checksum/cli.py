import argparse
import hashlib
import json
import sys
from dataclasses import asdict
from importlib.metadata import version
from pathlib import Path

from flyrail import (
    Bundle,
    OperationStatus,
    Target,
    TargetInspection,
    TargetResult,
    inspect,
    install,
    uninstall,
    update,
)
from flyrail_example_checksum import configuration

BUNDLE_ID = "example-checksum"


def result_payload(result: TargetInspection | TargetResult) -> dict[str, object]:
    return {
        **asdict(result),
        "target": {
            "root": str(result.target.root),
            "agent": result.target.agent,
            "scope": result.target.scope,
        },
        "is_current": result.observation.is_current,
    }


def main() -> int:
    app_version = version("flyrail-example-checksum")
    parser = argparse.ArgumentParser(description="Calculate the SHA-256 digest of a file.")
    parser.add_argument("--version", action="version", version=f"%(prog)s {app_version}")
    commands = parser.add_subparsers(dest="command", required=True)
    digest = commands.add_parser("digest", help="print a file's SHA-256 digest")
    digest.add_argument("file", type=Path)
    configuration.configure_parser(
        commands.add_parser("config", help="manage the combined agent configuration")
    )
    ai = commands.add_parser("ai", help="manage the bundled checksum skill")
    actions = ai.add_subparsers(dest="action", required=True)
    for action in ("status", "install", "update", "uninstall"):
        command = actions.add_parser(action)
        command.add_argument("--target", type=Path, action="append", required=True)
        if action in {"update", "uninstall"}:
            command.add_argument("--replace-modified", action="store_true")
    arguments = parser.parse_args()
    try:
        if arguments.command == "digest":
            with arguments.file.open("rb") as stream:
                print(hashlib.file_digest(stream, "sha256").hexdigest())
            return 0
        if arguments.command == "config":
            return configuration.main(arguments, app_version)
        targets = [Target.directory(path) for path in arguments.target]
        bundle_version = None
        if arguments.action == "uninstall":
            results = uninstall(BUNDLE_ID, targets, replace_modified=arguments.replace_modified)
        else:
            bundle = Bundle.from_package("flyrail_example_checksum")
            if bundle.id != BUNDLE_ID:
                raise ValueError("packaged bundle ID differs from the host's stable ID")
            bundle_version = bundle.version
            if arguments.action == "status":
                observations = inspect(bundle, targets)
                print(
                    json.dumps(
                        {
                            "app_version": app_version,
                            "bundle_id": BUNDLE_ID,
                            "bundle_version": bundle_version,
                            "results": [result_payload(result) for result in observations],
                        },
                        default=str,
                    )
                )
                return int(any(not result.observation.is_current for result in observations))
            if arguments.action == "install":
                results = install(bundle, targets)
            else:
                results = update(bundle, targets, replace_modified=arguments.replace_modified)
        print(
            json.dumps(
                {
                    "app_version": app_version,
                    "bundle_id": BUNDLE_ID,
                    "bundle_version": bundle_version,
                    "results": [result_payload(result) for result in results],
                },
                default=str,
            )
        )
        return int(
            any(
                result.status not in {OperationStatus.APPLIED, OperationStatus.UNCHANGED}
                or result.error is not None
                or result.observation.error is not None
                or result.recovery_paths
                for result in results
            )
        )
    except (OSError, ValueError) as error:
        print(f"{parser.prog}: {error}", file=sys.stderr)
        return 2
