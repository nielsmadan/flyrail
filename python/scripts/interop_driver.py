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
    HookRuntime,
    InstallationObservation,
    InstallationResult,
    InstallationTarget,
    InstructionArtifact,
    Platform,
    RenderContext,
    RenderedArtifact,
    RenderedBundle,
    ResourceAuthority,
    ResourceKind,
    ResourceRef,
    SectionContent,
    Target,
    inspect_installation,
    recover_installation,
    remove,
    render,
    sync,
)
from flyrail import _lifecycle as lifecycle
from flyrail._codec import decode, encode
from flyrail._observation import ObservationFailure
from flyrail._resource_io import admit
from flyrail._state import destination_key, store_lock
from flyrail.stores import DirectoryStore

MAX_PAYLOAD_SIZE = 1024 * 1024


def compact(value: object) -> None:
    sys.stdout.write(json.dumps(value, ensure_ascii=False, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def bundle(bundle_id: str, version: str) -> Bundle:
    return Bundle.from_artifacts(
        BundleIdentity(bundle_id, version), [InstructionArtifact("guide", "text")]
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


def store(arguments: argparse.Namespace) -> DirectoryStore:
    return DirectoryStore(arguments.state)


def installation(arguments: argparse.Namespace) -> InstallationTarget:
    return InstallationTarget(store(arguments))


def rendered(arguments: argparse.Namespace) -> RenderedBundle:
    mode = {
        "private": FileMode.PRIVATE,
        "readable": FileMode.READABLE,
        "executable": FileMode.EXECUTABLE,
    }[arguments.mode]
    content: FileContent | SectionContent = (
        FileContent(payload(arguments), mode)
        if arguments.section is None
        else SectionContent(arguments.section, payload(arguments).decode())
    )
    return RenderedBundle(
        [
            RenderedArtifact(
                "guide",
                Family.INSTRUCTIONS,
                Path(arguments.destination).resolve(),
                content,
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


def source_installation(arguments: argparse.Namespace) -> tuple[Bundle, RenderedBundle]:
    source_bundle = Bundle.from_directory(Path(arguments.source))
    if source_bundle.id != arguments.bundle_id:
        raise ValueError("source bundle id must match --bundle-id")
    context = RenderContext(
        Target.project(arguments.agent, Path(arguments.destination).resolve()),
        Platform.MACOS,
        hook_runtime=HookRuntime(Path(arguments.node) if arguments.node is not None else None),
    )
    return source_bundle, render(source_bundle, context)


def write_command(arguments: argparse.Namespace) -> None:
    configure_interop_seam(arguments)
    acquisition = Acquisition(arguments.acquisition)
    source_bundle, source_rendered = (
        source_installation(arguments)
        if arguments.source is not None
        else (bundle(arguments.bundle_id, arguments.version), rendered(arguments))
    )
    result = sync(
        source_bundle,
        source_rendered,
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


def records_command(arguments: argparse.Namespace) -> None:
    selected = store(arguments)
    records = []
    for key in selected.keys(""):
        raw = selected.read(key)
        if raw is None:
            continue
        value = decode(raw)
        records.append(
            {
                "key": key,
                "type": type(value).__name__,
                "bytes": base64.b64encode(raw).decode("ascii"),
                "canonical": base64.b64encode(encode(value)).decode("ascii"),
            }
        )
    compact({"records": records})


def hold_command(arguments: argparse.Namespace) -> None:
    selected = store(arguments)
    destination = ResourceAuthority(Path(arguments.destination).resolve()).destination
    reference = ResourceRef(destination, ResourceKind(arguments.kind))
    barrier = Path(arguments.barrier).resolve()
    validate_timeout(arguments.lock_timeout, "lock timeout")
    validate_timeout(arguments.wait_timeout, "wait timeout")
    with store_lock(selected, f"resource/{destination_key(destination)}", arguments.lock_timeout):
        compact({"status": "reserved"})
        deadline = time.monotonic() + arguments.wait_timeout
        while not barrier.exists():
            if time.monotonic() >= deadline:
                raise TimeoutError("barrier timed out")
            time.sleep(0.005)
        try:
            admit(selected, reference)
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

            def prepare(store: object, plan: object) -> object:
                journal = original_prepare(store, plan)
                wait()
                return journal

            lifecycle.__dict__["prepare"] = prepare
        else:
            original_publish = lifecycle.__dict__["publish"]

            def publish(store: object, journal: object) -> None:
                original_publish(store, journal)
                wait()

            lifecycle.__dict__["publish"] = publish
    elif ready is not None or release is not None:
        raise ValueError("--barrier-ready and --barrier-release require --barrier-phase")
    if arguments.fail_index_completion:
        original_write = lifecycle.__dict__["write_record"]
        index_writes = 0

        def write_record(store: object, key: str, value: object, **keywords: object) -> None:
            nonlocal index_writes
            if key.startswith("index/"):
                index_writes += 1
                if index_writes == 2:
                    raise OSError(errno.EIO, "injected index-completion fault")
            original_write(store, key, value, **keywords)

        lifecycle.__dict__["write_record"] = write_record


def encode_command() -> None:
    sys.stdout.buffer.write(encode(decode(sys.stdin.buffer.read())))


def add_target_arguments(selected: argparse.ArgumentParser) -> None:
    selected.add_argument("--state", required=True)
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
        write_parser.add_argument("--source")
        write_parser.add_argument("--node")
        write_parser.add_argument("--agent", default="codex")
        write_parser.add_argument("--version", default="1")
        write_parser.add_argument("--payload", default="generated\n")
        write_parser.add_argument("--large", type=int)
        write_parser.add_argument("--section")
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
    records_parser.add_argument("--state", required=True)
    records_parser.set_defaults(handler=records_command)
    hold_parser = commands.add_parser("hold")
    hold_parser.add_argument("--state", required=True)
    hold_parser.add_argument("--destination", required=True)
    hold_parser.add_argument("--barrier", required=True)
    hold_parser.add_argument("--kind", choices=("document", "tree"), default="document")
    hold_parser.add_argument("--lock-timeout", type=float, default=0)
    hold_parser.add_argument("--wait-timeout", type=float, default=30)
    hold_parser.set_defaults(handler=hold_command)
    return selected


def main() -> int:
    options = [argument for argument in sys.argv[2:] if argument.startswith("--")]
    if "--source" in options:
        incompatible = next(
            (
                option
                for option in ("--mode", "--large", "--kind", "--section", "--payload", "--version")
                if option in options
            ),
            None,
        )
        if incompatible is not None:
            compact(
                {
                    "error": error_value(
                        ValueError(f"{incompatible} cannot be combined with --source")
                    )
                }
            )
            return 1
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
