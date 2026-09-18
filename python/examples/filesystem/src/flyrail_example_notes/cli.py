import argparse
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
    app_version = version("flyrail-example-notes")
    parser = argparse.ArgumentParser(description="Count words in a UTF-8 notes file.")
    parser.add_argument("--version", action="version", version=f"%(prog)s {app_version}")
    commands = parser.add_subparsers(dest="command", required=True)
    words = commands.add_parser("words", help="count whitespace-separated words")
    words.add_argument("file", type=Path)
    ai = commands.add_parser("ai", help="manage an explicitly selected skill bundle")
    actions = ai.add_subparsers(dest="action", required=True)
    for action in ("status", "install", "update", "uninstall"):
        command = actions.add_parser(action)
        command.add_argument("--target", type=Path, action="append", required=True)
        if action == "uninstall":
            command.add_argument("--bundle-id", required=True)
        else:
            command.add_argument("--source", type=Path, required=True)
        if action in {"update", "uninstall"}:
            command.add_argument("--replace-modified", action="store_true")
    arguments = parser.parse_args()
    try:
        if arguments.command == "words":
            print(len(arguments.file.read_text(encoding="utf-8").split()))
            return 0
        targets = [Target.directory(path) for path in arguments.target]
        bundle_version = None
        if arguments.action == "uninstall":
            bundle_id = arguments.bundle_id
            results = uninstall(bundle_id, targets, replace_modified=arguments.replace_modified)
        else:
            bundle = Bundle.from_directory(arguments.source)
            bundle_id, bundle_version = bundle.id, bundle.version
            if arguments.action == "status":
                observations = inspect(bundle, targets)
                print(
                    json.dumps(
                        {
                            "app_version": app_version,
                            "bundle_id": bundle_id,
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
                    "bundle_id": bundle_id,
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
