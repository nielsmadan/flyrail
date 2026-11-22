"""Check the npm package as consumers install it: Node, Bun and, on macOS, an Electron app."""

import fcntl
import json
import os
import platform
import re
import signal
import shutil
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PACKAGE = ROOT / "typescript"
CACHE = ROOT / ".cache"
RELEASE = CACHE / "typescript/release"
RUNS = CACHE / "typescript/package-qa"
CONSUMER = PACKAGE / "scripts/consumer"
EXAMPLE_BUNDLE = PACKAGE / "examples/flyrail-notes/bundle"
BIN = PACKAGE / "node_modules/.bin"

COMMAND_TIMEOUT = 900
CONSUMER_TIMEOUT = 180
ELECTRON_TIMEOUT = 120
ABORT_LIMIT_MS = 2000
ELECTRON_SANDBOX_DENIAL = (
    "bootstrap_check_in com.github.Electron.MachPortRendezvousServer",
    "Permission denied (1100)",
)
UNSANDBOXED_COMMAND = "cd typescript && just check-package"
TRIPLES = {
    ("darwin", "arm64"): "darwin-arm64",
    ("darwin", "x64"): "darwin-x64",
    ("linux", "arm64"): "linux-arm64-gnu",
    ("linux", "x64"): "linux-x64-gnu",
}


class StepFailedError(Exception):
    pass


class StepBlockedError(Exception):
    pass


class ConsumerExitedError(Exception):
    pass


@dataclass(frozen=True)
class Host:
    os: str
    arch: str

    @property
    def triple(self) -> str:
        return TRIPLES[(self.os, self.arch)]

    @property
    def platform_package(self) -> str:
        return f"flyrail-{self.os}-{self.arch}"


@dataclass(frozen=True)
class Packages:
    version: str
    flyrail: Path
    platform: Path


def current_host() -> Host:
    machine = {"x86_64": "x64", "amd64": "x64", "arm64": "arm64", "aarch64": "arm64"}.get(
        platform.machine().lower(), platform.machine().lower()
    )
    host = Host(sys.platform, machine)
    if (host.os, host.arch) not in TRIPLES:
        raise StepFailedError(f"unsupported host {host.os}-{host.arch}")
    return host


def environment(extra: dict[str, str] | None = None) -> dict[str, str]:
    values = {
        **os.environ,
        "npm_config_cache": str(CACHE / "npm"),
        "npm_config_update_notifier": "false",
        "npm_config_fund": "false",
        "npm_config_audit": "false",
        "ELECTRON_CACHE": str(CACHE / "electron"),
        "electron_config_cache": str(CACHE / "electron"),
    }
    return {**values, **(extra or {})}


def kill_group(process: subprocess.Popen[bytes] | subprocess.Popen[str]) -> None:
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()


def run(command: list[str], cwd: Path, timeout: int = COMMAND_TIMEOUT) -> str:
    process = subprocess.Popen(  # noqa: S603
        command,
        cwd=cwd,
        env=environment(),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired as error:
        kill_group(process)
        raise StepFailedError(f"{' '.join(command)} timed out after {timeout} s") from error
    if process.returncode != 0:
        raise StepFailedError(
            f"{' '.join(command)} exited {process.returncode}\n{stdout[-4000:]}\n{stderr[-4000:]}"
        )
    return stdout


def private_directory(path: Path) -> Path:
    path.mkdir(mode=0o700, parents=True)
    path.chmod(0o700)
    return path


def expect(label: str, actual: object, wanted: object) -> list[str]:
    return [] if actual == wanted else [f"{label}: {actual!r} != {wanted!r}"]


def package_version() -> str:
    return json.loads((PACKAGE / "package.json").read_text())["version"]


def check_build_outputs(host: Host) -> Path:
    release = RELEASE / f"flyrail.{host.triple}.node"
    if not release.is_file():
        raise StepFailedError(f"{release} is missing: run just native-build-release")
    missing = [
        name
        for name in ("index.js", "index.d.ts", "native/loader.js")
        if not (PACKAGE / "dist" / name).is_file()
    ]
    if missing:
        raise StepFailedError(f"dist/ lacks {', '.join(missing)}: run just build")
    return release


def pack(directory: Path, destination: Path) -> tuple[Path, set[str]]:
    output = json.loads(
        run(["npm", "pack", "--json", "--pack-destination", str(destination)], directory)
    )
    [entry] = output
    return destination / entry["filename"], {item["path"] for item in entry["files"]}


def pack_flyrail(run_dir: Path) -> Path:
    tarball, files = pack(PACKAGE, run_dir)
    dist = PACKAGE / "dist"
    wanted = {
        f"dist/{path.relative_to(dist).as_posix()}" for path in dist.rglob("*") if path.is_file()
    }
    wanted |= {"package.json", "README.md", "LICENSE"}
    if files != wanted:
        raise StepFailedError(
            f"package files differ: unexpected {sorted(files - wanted)}, "
            f"missing {sorted(wanted - files)}"
        )
    return tarball


def pack_platform(run_dir: Path, host: Host, version: str, release: Path) -> Path:
    directory = run_dir / "platform" / host.platform_package
    shutil.rmtree(directory, ignore_errors=True)
    private_directory(directory)
    manifest = {
        "name": host.platform_package,
        "version": version,
        "description": f"The flyrail native addon for {host.os}-{host.arch}",
        "license": "MIT",
        "repository": {
            "type": "git",
            "url": "git+https://github.com/nielsmadan/flyrail.git",
            "directory": "typescript",
        },
        "os": [host.os],
        "cpu": [host.arch],
        **({"libc": ["glibc"]} if host.os == "linux" else {}),
        "files": ["flyrail.node"],
    }
    (directory / "package.json").write_text(json.dumps(manifest, indent=2) + "\n")
    shutil.copyfile(release, directory / "flyrail.node")
    shutil.copyfile(PACKAGE / "LICENSE", directory / "LICENSE")
    tarball, files = pack(directory, run_dir)
    if files != {"package.json", "flyrail.node", "LICENSE"}:
        raise StepFailedError(f"platform package files: {sorted(files)}")
    return tarball


def lint_step(_run_dir: Path, packages: Packages, _host: Host) -> str:
    run([str(BIN / "publint"), "--strict", "--pack", "npm"], PACKAGE)
    run([str(BIN / "attw"), "--profile", "esm-only", str(packages.flyrail)], PACKAGE)
    return "clean"


def node_consumer(run_dir: Path, packages: Packages) -> Path:
    directory = run_dir / "node-consumer"
    if directory.is_dir():
        return directory
    private_directory(directory)
    (directory / "package.json").write_text(json.dumps({"type": "module", "private": True}) + "\n")
    run(["npm", "install", str(packages.flyrail), str(packages.platform)], directory)
    for name in ("core.mjs", "node-consumer.mjs"):
        shutil.copyfile(CONSUMER / name, directory / name)
    return directory


def scratch(run_dir: Path, label: str) -> Path:
    directory = private_directory(run_dir / f"scratch-{label}")
    shutil.copytree(EXAMPLE_BUNDLE, directory / "bundle")
    return directory


def wait_for(path: Path, process: subprocess.Popen[bytes], deadline: float) -> str:
    while not path.is_file():
        if process.poll() is not None:
            raise ConsumerExitedError(
                f"consumer exited {process.returncode} before writing {path.name}"
            )
        if time.monotonic() > deadline:
            raise StepFailedError(f"timed out waiting for {path}")
        time.sleep(0.02)
    return path.read_text()


def hold_lock_during_abort(
    directory: Path, process: subprocess.Popen[bytes], deadline: float
) -> dict[str, object]:
    handshake = directory / "handshake"
    try:
        announced = json.loads(wait_for(handshake / "lock.json", process, deadline))
        store = directory / "state"
        if announced["storeLocation"] != str(store):
            raise StepFailedError(f"consumer store {announced['storeLocation']} is not {store}")
        lock_file = store / "locks" / announced["lockName"].replace("/", "~")
        descriptor = os.open(lock_file, os.O_RDWR | os.O_CREAT, 0o600)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            (handshake / "held").touch()
            aborted = json.loads(wait_for(handshake / "aborted.json", process, deadline))
        finally:
            os.close(descriptor)
    except (ValueError, KeyError, OSError) as error:
        raise StepFailedError(f"lock handshake failed: {error!r}") from error
    (handshake / "released").touch()
    return {"lockName": announced["lockName"], **aborted}


def run_consumer(
    label: str,
    command: list[str],
    cwd: Path,
    directory: Path,
    run_dir: Path,
    timeout: int,
    extra_env: dict[str, str],
) -> tuple[dict[str, object], dict[str, object]]:
    stdout_path = run_dir / f"{label}.stdout"
    stderr_path = run_dir / f"{label}.stderr"
    deadline = time.monotonic() + timeout
    with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
        process = subprocess.Popen(  # noqa: S603
            command,
            cwd=cwd,
            env=environment(extra_env),
            stdout=stdout,
            stderr=stderr,
            start_new_session=True,
        )
        try:
            handshake = hold_lock_during_abort(directory, process, deadline)
            process.wait(timeout=max(1.0, deadline - time.monotonic()))
        except ConsumerExitedError as error:
            raise_for_exit(label, command, stderr_path, str(error))
            raise
        except subprocess.TimeoutExpired as error:
            raise StepFailedError(f"{label} did not exit within {timeout} s") from error
        finally:
            kill_group(process)
    if process.returncode != 0:
        raise_for_exit(label, command, stderr_path, f"consumer exited {process.returncode}")
    lines = stdout_path.read_text().strip().splitlines()
    if not lines:
        raise StepFailedError(f"{label} printed no report; stderr in {stderr_path}")
    try:
        return json.loads(lines[-1]), handshake
    except ValueError as error:
        raise StepFailedError(f"{label} printed a non-JSON last line: {lines[-1][:200]!r}") from error


def raise_for_exit(label: str, command: list[str], stderr_path: Path, reason: str) -> None:
    stderr = stderr_path.read_text(errors="replace")
    if all(marker in stderr for marker in ELECTRON_SANDBOX_DENIAL):
        raise StepBlockedError(
            f"{label} cannot start inside this sandbox ({ELECTRON_SANDBOX_DENIAL[0]}: "
            f"{ELECTRON_SANDBOX_DENIAL[1]}); run `{UNSANDBOXED_COMMAND}` outside the sandbox"
        )
    raise StepFailedError(f"{label}: {reason}\ncommand: {' '.join(command)}\n{stderr[-4000:]}")


def abort_failures(abort: object, handshake: dict[str, object]) -> list[str]:
    if not isinstance(abort, dict):
        return [f"abort: {abort!r}"]
    elapsed = abort.get("elapsedMs")
    in_range = isinstance(elapsed, int) and 90 <= elapsed < ABORT_LIMIT_MS
    return [
        *expect("abort.lockName", abort.get("lockName"), handshake["lockName"]),
        *expect("abort.firstSync", abort.get("firstSync"), "applied"),
        *expect("abort.outcome", abort.get("outcome"), "reason"),
        *expect("abort.error", abort.get("error"), None),
        *expect("abort.afterRelease", abort.get("afterRelease"), "unchanged"),
        *expect("abort.removed", abort.get("removed"), "applied"),
        *expect("handshake.outcome", handshake.get("outcome"), "reason"),
        *([] if in_range else [f"abort.elapsedMs: {elapsed!r} is not in [90, {ABORT_LIMIT_MS})"]),
    ]


def loaded_failures(loaded: object, native_dir: Path | None) -> list[str]:
    if native_dir is None:
        return []
    wanted = os.path.realpath(native_dir / "flyrail.node")
    if isinstance(loaded, str) and os.path.realpath(loaded) == wanted:
        return []
    return [f"loadedFrom: {loaded!r} is not {wanted}"]


def check_report(
    report: dict[str, object],
    handshake: dict[str, object],
    runtime: str,
    version: str,
    native_dir: Path | None,
) -> None:
    manifest = json.loads((EXAMPLE_BUNDLE / "flyrail.json").read_text())
    failures = [
        *expect("runtime", report.get("runtime"), runtime),
        *expect("flyrailVersion", report.get("flyrailVersion"), version),
        *expect("bundleId", report.get("bundleId"), manifest["id"]),
        *expect("bundleVersion", report.get("bundleVersion"), manifest["version"]),
        *expect("install", report.get("install"), ["applied"]),
        *expect("inspect", report.get("inspect"), [True]),
        *expect("concurrent", report.get("concurrent"), ["applied"] * 8),
        *expect("uninstall", report.get("uninstall"), ["applied"]),
        *expect("skillRemoved", report.get("skillRemoved"), True),
        *abort_failures(report.get("abort"), handshake),
        *loaded_failures(report.get("loadedFrom"), native_dir),
    ]
    if failures:
        raise StepFailedError("; ".join(failures))


def node_step(run_dir: Path, packages: Packages, host: Host) -> str:
    directory = node_consumer(run_dir, packages)
    work = scratch(run_dir, "node")
    report, handshake = run_consumer(
        "node",
        ["node", "node-consumer.mjs", str(work)],
        directory,
        work,
        run_dir,
        CONSUMER_TIMEOUT,
        {},
    )
    native = directory / "node_modules" / host.platform_package
    check_report(report, handshake, "node", packages.version, native)
    return (
        f"node {report['versions']['node']}, abort settled after {report['abort']['elapsedMs']} ms"  # type: ignore[index]
    )


def missing_addon_step(run_dir: Path, packages: Packages, _host: Host) -> str:
    directory = private_directory(run_dir / "missing-addon-consumer")
    (directory / "package.json").write_text(json.dumps({"type": "module", "private": True}) + "\n")
    run(["npm", "install", str(packages.flyrail)], directory)
    shutil.copyfile(CONSUMER / "missing-addon.mjs", directory / "missing-addon.mjs")
    report = json.loads(run(["node", "missing-addon.mjs"], directory).splitlines()[-1])
    failures = [
        *expect("loaded", report.get("loaded"), False),
        *expect("nativeLoadError", report.get("nativeLoadError"), True),
        *expect("name", report.get("name"), "NativeLoadError"),
        *expect("attempts", report.get("attempts"), 2),
    ]
    if failures:
        raise StepFailedError("; ".join(failures))
    return "flyrail/errors imports without the addon; import('flyrail') rejects with NativeLoadError"


def bun_executable() -> str:
    try:
        result = subprocess.run(  # noqa: S603
            [sys.executable, "-I", "-B", str(PACKAGE / "scripts/tools.py"), "bun"],
            capture_output=True,
            text=True,
            check=False,
            timeout=COMMAND_TIMEOUT,
        )
    except subprocess.TimeoutExpired as error:
        raise StepFailedError(f"installing the pinned Bun timed out after {COMMAND_TIMEOUT} s") from error
    if result.returncode != 0:
        raise StepFailedError(f"the pinned Bun could not be installed: {result.stderr.strip()}")
    return result.stdout.strip()


def pinned_bun_version() -> str:
    match = re.search(r'^BUN_VERSION = "([^"]+)"', (PACKAGE / "scripts/tools.py").read_text(), re.M)
    if match is None:
        raise StepFailedError("scripts/tools.py does not define BUN_VERSION")
    return match.group(1)


def bun_step(run_dir: Path, packages: Packages, host: Host) -> str:
    directory = node_consumer(run_dir, packages)
    work = scratch(run_dir, "bun")
    bun = bun_executable()
    report, handshake = run_consumer(
        "bun",
        [bun, "run", "node-consumer.mjs", str(work)],
        directory,
        work,
        run_dir,
        CONSUMER_TIMEOUT,
        {},
    )
    native = directory / "node_modules" / host.platform_package
    pinned = pinned_bun_version()
    if report["versions"]["bun"] != pinned:  # type: ignore[index]
        raise StepFailedError(f"bun {report['versions']['bun']} ran, not the pinned {pinned}")  # type: ignore[index]
    check_report(
        report, handshake, "bun", packages.version, native if report.get("loadedFrom") else None
    )
    loaded = (
        "loaded from the platform package"
        if report.get("loadedFrom")
        else "no shared-object report"
    )
    elapsed = report["abort"]["elapsedMs"]  # type: ignore[index]
    return f"bun {report['versions']['bun']}, {loaded}, abort settled after {elapsed} ms"  # type: ignore[index]


def build_electron_app(run_dir: Path, packages: Packages, host: Host) -> Path:
    directory = private_directory(run_dir / "electron")
    shutil.copyfile(CONSUMER / "core.mjs", run_dir / "core.mjs")
    for name in ("package.json", "main.ts"):
        shutil.copyfile(CONSUMER / "electron" / name, directory / name)
    run(["npm", "install", str(packages.flyrail), str(packages.platform)], directory)
    run(["node", "node_modules/electron/install.js"], directory)
    app = private_directory(directory / "app")
    run(
        [
            "node_modules/.bin/esbuild",
            "main.ts",
            "--bundle",
            "--platform=node",
            "--format=esm",
            "--target=node24",
            "--outfile=app/main.mjs",
            "--external:electron",
            "--external:flyrail",
            "--external:flyrail-*",
        ],
        directory,
    )
    source = json.loads((CONSUMER / "electron/package.json").read_text())
    manifest = {key: value for key, value in source.items() if key != "devDependencies"}
    (app / "package.json").write_text(json.dumps(manifest, indent=2) + "\n")
    for name in ("core.mjs", "node-consumer.mjs"):
        shutil.copyfile(CONSUMER / name, app / name)
    for name in ("flyrail", host.platform_package):
        shutil.copytree(directory / "node_modules" / name, app / "node_modules" / name)
    run(["node_modules/.bin/asar", "pack", "app", "app.asar", "--unpack", "**/*.node"], directory)
    listing = run(
        ["node_modules/.bin/asar", "list", "--is-pack", "app.asar"], directory
    ).splitlines()
    entries = [
        tuple(part.strip() for part in line.split(":", 1)) for line in listing if ":" in line
    ]
    packed_native = [path for state, path in entries if state == "pack" and path.endswith(".node")]
    if packed_native:
        raise StepFailedError(
            f"app.asar packs native files {packed_native} instead of unpacking them"
        )
    if ("pack", "/node_modules/flyrail/dist/index.js") not in entries:
        raise StepFailedError("app.asar lacks /node_modules/flyrail/dist/index.js")
    unpacked = directory / "app.asar.unpacked/node_modules" / host.platform_package / "flyrail.node"
    if not unpacked.is_file():
        raise StepFailedError(f"{unpacked} is missing")
    return directory


def electron_app_step(run_dir: Path, packages: Packages, host: Host) -> str:
    directory = build_electron_app(run_dir, packages, host)
    return f"app.asar packed in {directory.name}/ with every .node unpacked"


def electron_binary(directory: Path) -> Path:
    return directory / "node_modules/electron/dist/Electron.app/Contents/MacOS/Electron"


def electron_as_node_step(run_dir: Path, packages: Packages, host: Host) -> str:
    directory = run_dir / "electron"
    work = scratch(run_dir, "electron-as-node")
    report, handshake = run_consumer(
        "electron-as-node",
        [str(electron_binary(directory)), str(directory / "app.asar/node-consumer.mjs"), str(work)],
        directory,
        work,
        run_dir,
        ELECTRON_TIMEOUT,
        {"ELECTRON_RUN_AS_NODE": "1"},
    )
    native = directory / "app.asar.unpacked/node_modules" / host.platform_package
    check_report(report, handshake, "electron-run-as-node", packages.version, native)
    return f"electron {report['versions']['electron']} as node, loaded {report['loadedFrom']}"


def electron_main_step(run_dir: Path, packages: Packages, host: Host) -> str:
    directory = run_dir / "electron"
    work = scratch(run_dir, "electron")
    report, handshake = run_consumer(
        "electron-main",
        [str(electron_binary(directory)), str(directory / "app.asar"), str(work)],
        directory,
        work,
        run_dir,
        ELECTRON_TIMEOUT,
        {},
    )
    native = directory / "app.asar.unpacked/node_modules" / host.platform_package
    check_report(report, handshake, "electron", packages.version, native)
    return f"electron {report['versions']['electron']} main process, loaded {report['loadedFrom']}"


Step = Callable[[Path, Packages, Host], str]


def prepare(run_dir: Path, host: Host) -> Packages:
    version = package_version()
    release = check_build_outputs(host)
    flyrail = pack_flyrail(run_dir)
    print(f"ok    pack {flyrail.name}")
    platform_tarball = pack_platform(run_dir, host, version, release)
    print(f"ok    pack {platform_tarball.name}")
    return Packages(version, flyrail, platform_tarball)


def steps(host: Host) -> list[tuple[str, Step | None]]:
    electron_steps: list[tuple[str, Step | None]] = [
        ("electron app", electron_app_step),
        ("electron as node", electron_as_node_step),
        ("electron main process", electron_main_step),
    ]
    return [
        ("publint and attw", lint_step),
        ("node consumer", node_step),
        ("missing addon", missing_addon_step),
        ("bun consumer", bun_step),
        *(electron_steps if host.os == "darwin" else [("electron", None)]),
    ]


def pack_only(destination: Path) -> int:
    destination.mkdir(parents=True, exist_ok=True)
    try:
        packages = prepare(destination, current_host())
    except StepFailedError as error:
        print(f"FAILED pack: {error}")
        return 1
    print(packages.flyrail)
    print(packages.platform)
    return 0


def main(arguments: list[str]) -> int:
    if arguments[:1] == ["--pack-only"] and len(arguments) == 2:
        return pack_only(Path(arguments[1]).resolve())
    if arguments:
        print("usage: package_qa.py [--pack-only DIR]", file=sys.stderr)
        return 2
    host = current_host()
    RUNS.mkdir(parents=True, exist_ok=True)
    run_dir = Path(tempfile.mkdtemp(prefix="run-", dir=RUNS))
    print(f"package QA in {run_dir}")
    try:
        packages = prepare(run_dir, host)
    except StepFailedError as error:
        print(f"FAILED prepare: {error}\nkept {run_dir}")
        return 1
    outcomes: dict[str, str] = {}
    for name, step in steps(host):
        if step is None:
            outcomes[name] = "skipped"
            print(f"skip  {name}: Electron is tested on macOS only")
            continue
        if name.startswith("electron") and outcomes.get("electron app", "passed") != "passed":
            outcomes[name] = "failed"
            print(f"FAILED {name}: the Electron app was not built")
            continue
        try:
            detail = step(run_dir, packages, host)
        except StepBlockedError as error:
            outcomes[name] = "blocked"
            print(f"BLOCKED {name}: {error}")
        except Exception as error:  # noqa: BLE001
            outcomes[name] = "failed"
            print(f"FAILED {name}: {error if isinstance(error, StepFailedError) else repr(error)}")
        else:
            outcomes[name] = "passed"
            print(f"ok    {name}: {detail}")
    states = ("passed", "failed", "blocked", "skipped")
    counts = {state: sum(1 for value in outcomes.values() if value == state) for state in states}
    summary = ", ".join(f"{count} {state}" for state, count in counts.items())
    if counts["failed"]:
        print(f"package QA FAILED: {summary}; kept {run_dir}")
        return 1
    if counts["blocked"]:
        print(
            f"package QA BLOCKED: {summary}; run `{UNSANDBOXED_COMMAND}` outside the sandbox; "
            f"kept {run_dir}"
        )
        return 3
    shutil.rmtree(run_dir)
    print(f"package QA passed: {summary}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
