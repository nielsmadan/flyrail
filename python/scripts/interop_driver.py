import argparse
import base64
import errno
import json
import math
import sys
import time
from pathlib import Path
from typing import cast

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from flyrail import (
    Acquisition,
    Bundle,
    BundleIdentity,
    Family,
    FileContent,
    FileMode,
    InstallationObservation,
    InstallationResult,
    InstallationTarget,
    InstructionArtifact,
    RenderedArtifact,
    RenderedBundle,
    ResourceAuthority,
    ResourceKind,
    ResourceRef,
    inspect_installation,
    recover_installation,
    remove,
    sync,
)
from flyrail import _lifecycle as lifecycle
from flyrail._codec import decode, encode
from flyrail._filesystem import target_lock
from flyrail._observation import ObservationFailure
from flyrail._resource_io import admit

STATE_RECORD_NAMES = frozenset(
    {
        "authority.json",
        "authority.json.next",
        "index.json",
        "index.json.next",
        "receipt.json",
        "receipt.json.next",
        "transaction.json",
        "transaction.json.next",
    }
)
MAX_PAYLOAD_SIZE = 1024 * 1024


def compact(value: object) -> None:
    sys.stdout.write(json.dumps(value, ensure_ascii=False, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def bundle(version: str) -> Bundle:
    return Bundle.from_artifacts(
        BundleIdentity("team", version), [InstructionArtifact("guide", "text")]
    )


def payload(arguments: argparse.Namespace) -> bytes:
    if arguments.large is not None:
        size = cast(int, arguments.large)
        if size < 0:
            raise ValueError("large payload size must be nonnegative")
        if size > MAX_PAYLOAD_SIZE:
            raise ValueError(f"large payload size must not exceed {MAX_PAYLOAD_SIZE}")
        pattern = b"flyrail-interop-payload\n"
        return (pattern * ((size + len(pattern) - 1) // len(pattern)))[:size]
    return cast(str, arguments.payload).encode()


def installation(arguments: argparse.Namespace) -> InstallationTarget:
    return InstallationTarget(Path(arguments.index).resolve())


def rendered(arguments: argparse.Namespace) -> RenderedBundle:
    mode = {
        "private": FileMode.PRIVATE,
        "readable": FileMode.READABLE,
        "executable": FileMode.EXECUTABLE,
    }[arguments.mode]
    return RenderedBundle(
        [
            RenderedArtifact(
                "guide",
                Family.INSTRUCTIONS,
                Path(arguments.destination).resolve(),
                FileContent(payload(arguments), mode),
            )
        ]
    )


def error_value(error: object | None) -> dict[str, object] | None:
    if error is None:
        return None
    code = getattr(getattr(error, "code", None), "value", None)
    path = getattr(error, "path", None)
    return {
        "code": code,
        "message": str(getattr(error, "message", error)),
        "path": None if path is None else str(path),
    }


def observation_value(observation: InstallationObservation) -> dict[str, object]:
    return {
        "bundle_id": observation.bundle_id,
        "is_current": observation.is_current,
        "pending": observation.pending,
        "version": observation.version,
        "error": error_value(observation.error),
        "resources": [
            {
                "destination": str(item.destination),
                "state_root": str(item.state_root),
                "claims": [
                    {
                        "artifact_id": claim.artifact_id,
                        "disposition": claim.status.value,
                    }
                    for claim in item.claims
                ],
                "error": error_value(item.error),
                "recovery_paths": [str(path) for path in item.recovery_paths],
            }
            for item in observation.resources
        ],
    }


def result_value(result: InstallationResult) -> dict[str, object]:
    return {
        "status": result.status.value,
        "error": error_value(result.error),
        "observation": observation_value(result.observation),
        "resources": [
            {
                "destination": str(item.resource.destination),
                "status": item.status.value,
                "error": error_value(item.error),
                "recovery_paths": [str(path) for path in item.recovery_paths],
            }
            for item in result.resources
        ],
    }


def write_command(arguments: argparse.Namespace) -> None:
    configure_interop_seam(arguments)
    acquisition = Acquisition(arguments.acquisition)
    result = sync(
        bundle(arguments.version),
        rendered(arguments),
        installation(arguments),
        acquisition=acquisition,
        replace_modified=arguments.replace_modified,
        lock_timeout=arguments.lock_timeout,
    )
    compact(result_value(result))


def inspect_command(arguments: argparse.Namespace) -> None:
    compact(observation_value(inspect_installation(arguments.bundle_id, installation(arguments))))


def remove_command(arguments: argparse.Namespace) -> None:
    result = remove(
        arguments.bundle_id,
        installation(arguments),
        replace_modified=arguments.replace_modified,
        lock_timeout=arguments.lock_timeout,
    )
    compact(result_value(result))


def recover_command(arguments: argparse.Namespace) -> None:
    result = recover_installation(
        arguments.bundle_id,
        installation(arguments),
        lock_timeout=arguments.lock_timeout,
    )
    compact(result_value(result))


def record_paths(arguments: argparse.Namespace) -> list[Path]:
    roots = [Path(arguments.index).resolve()]
    if arguments.destination is not None:
        roots.append(ResourceAuthority(Path(arguments.destination).resolve()).state_root)
    return sorted(
        (
            path
            for root in roots
            if root.is_dir()
            for path in root.iterdir()
            if path.name in STATE_RECORD_NAMES and path.is_file()
        ),
        key=lambda path: str(path),
    )


def records_command(arguments: argparse.Namespace) -> None:
    records = []
    for path in record_paths(arguments):
        raw = path.read_bytes()
        value = decode(raw)
        records.append(
            {
                "path": str(path),
                "type": type(value).__name__,
                "bytes": base64.b64encode(raw).decode("ascii"),
                "canonical": base64.b64encode(encode(value)).decode("ascii"),
            }
        )
    compact({"records": records})


def hold_command(arguments: argparse.Namespace) -> None:
    destination = Path(arguments.destination).resolve()
    authority = ResourceAuthority(destination)
    reference = ResourceRef(destination, ResourceKind(arguments.kind), authority.state_root)
    barrier = Path(arguments.barrier).resolve()
    validate_timeout(arguments.lock_timeout, "lock timeout")
    validate_timeout(arguments.wait_timeout, "wait timeout")
    with target_lock(reference.state_root, arguments.lock_timeout):
        compact({"status": "reserved"})
        deadline = time.monotonic() + arguments.wait_timeout
        while not barrier.exists():
            if time.monotonic() >= deadline:
                raise TimeoutError("barrier timed out")
            time.sleep(0.005)
        try:
            admit(reference)
        except ObservationFailure as error:
            compact({"status": error.error.code.value})
        else:
            compact({"status": "admitted"})


def validate_timeout(value: float, name: str) -> None:
    if value < 0 or not math.isfinite(value):
        raise ValueError(f"{name} must be finite and nonnegative")


def barrier(ready: Path, release: Path, timeout: float) -> None:
    ready.write_text("ready\n")
    deadline = time.monotonic() + timeout
    while not release.exists():
        if time.monotonic() >= deadline:
            raise TimeoutError("barrier timed out")
        time.sleep(0.005)


def configure_interop_seam(arguments: argparse.Namespace) -> None:
    phase = arguments.barrier_phase
    ready = arguments.barrier_ready
    release = arguments.barrier_release
    timeout = arguments.barrier_timeout
    if arguments.fail_index_completion and any(
        value is not None for value in (phase, ready, release)
    ):
        raise ValueError("fault and barrier options cannot be combined")
    if phase is not None:
        if ready is None or release is None:
            raise ValueError("--barrier-phase requires --barrier-ready and --barrier-release")
        validate_timeout(timeout, "barrier timeout")

        def wait() -> None:
            barrier(Path(ready).resolve(), Path(release).resolve(), timeout)

        if phase == "prepare":
            original_prepare = lifecycle.__dict__["prepare"]

            def prepare(plan: object) -> object:
                journal = original_prepare(plan)
                wait()
                return journal

            lifecycle.__dict__["prepare"] = prepare
        else:
            original_publish = lifecycle.__dict__["publish"]

            def publish(journal: object) -> None:
                original_publish(journal)
                wait()

            lifecycle.__dict__["publish"] = publish
    elif ready is not None or release is not None:
        raise ValueError("--barrier-ready and --barrier-release require --barrier-phase")
    if arguments.fail_index_completion:
        original_atomic = lifecycle.__dict__["atomic"]
        index_writes = 0

        def atomic(destination: Path, value: object, **keywords: object) -> None:
            nonlocal index_writes
            if destination.name == "index.json":
                index_writes += 1
                if index_writes == 2:
                    raise OSError(
                        errno.EIO,
                        "injected index-completion failure",
                        destination,
                    )
            original_atomic(destination, value, **keywords)

        lifecycle.__dict__["atomic"] = atomic


def encode_command() -> None:
    sys.stdout.buffer.write(encode(decode(sys.stdin.buffer.read())))


def add_target_arguments(selected: argparse.ArgumentParser) -> None:
    selected.add_argument("--index", required=True)
    selected.add_argument("--bundle-id", default="team")
    selected.add_argument("--lock-timeout", type=float, default=0)


def parser() -> argparse.ArgumentParser:
    selected = argparse.ArgumentParser()
    commands = selected.add_subparsers(dest="command", required=True)
    encode_parser = commands.add_parser("encode")
    encode_parser.set_defaults(handler=lambda _: encode_command())
    for name in ("write", "install", "update"):
        write_parser = commands.add_parser(name)
        add_target_arguments(write_parser)
        write_parser.add_argument("--destination", required=True)
        write_parser.add_argument("--version", default="1")
        write_parser.add_argument("--payload", default="generated\n")
        write_parser.add_argument("--large", type=int)
        write_parser.add_argument(
            "--mode", choices=("private", "readable", "executable"), default="readable"
        )
        write_parser.add_argument(
            "--acquisition",
            choices=tuple(item.value for item in Acquisition),
            default=Acquisition.CONFLICT.value,
        )
        write_parser.add_argument("--replace-modified", action="store_true")
        write_parser.add_argument("--barrier-phase", choices=("prepare", "receipt-committed"))
        write_parser.add_argument("--barrier-ready")
        write_parser.add_argument("--barrier-release")
        write_parser.add_argument("--barrier-timeout", type=float, default=30)
        write_parser.add_argument("--fail-index-completion", action="store_true")
        write_parser.set_defaults(handler=write_command)
    inspect_parser = commands.add_parser("inspect")
    add_target_arguments(inspect_parser)
    inspect_parser.set_defaults(handler=inspect_command)
    remove_parser = commands.add_parser("remove")
    add_target_arguments(remove_parser)
    remove_parser.add_argument("--replace-modified", action="store_true")
    remove_parser.set_defaults(handler=remove_command)
    recover_parser = commands.add_parser("recover")
    add_target_arguments(recover_parser)
    recover_parser.set_defaults(handler=recover_command)
    records_parser = commands.add_parser("records")
    records_parser.add_argument("--index", required=True)
    records_parser.add_argument("--destination")
    records_parser.set_defaults(handler=records_command)
    hold_parser = commands.add_parser("hold")
    hold_parser.add_argument("--destination", required=True)
    hold_parser.add_argument("--barrier", required=True)
    hold_parser.add_argument("--kind", choices=("document", "tree"), default="document")
    hold_parser.add_argument("--lock-timeout", type=float, default=0)
    hold_parser.add_argument("--wait-timeout", type=float, default=30)
    hold_parser.set_defaults(handler=hold_command)
    return selected


def main() -> int:
    options = [argument for argument in sys.argv[2:] if argument.startswith("--")]
    duplicate = next(
        (option for index, option in enumerate(options) if option in options[:index]), None
    )
    if duplicate is not None:
        parser().error(f"duplicate option: {duplicate}")
    arguments = parser().parse_args()
    try:
        arguments.handler(arguments)
    except (OSError, TimeoutError, TypeError, ValueError) as error:
        if arguments.command == "encode":
            print(str(error), file=sys.stderr)
            return 1
        compact({"error": error_value(error)})
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
