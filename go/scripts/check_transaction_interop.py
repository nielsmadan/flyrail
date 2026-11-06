"""Compare Go's interop driver with Python, Rust and (on macOS) Swift on the shared transaction protocol."""

import itertools
import json
import os
import re
import shutil
import subprocess
import sys
import unicodedata
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "swift/scripts"))

from harness import (
    ENV,
    GO,
    PYTHON,
    RUST,
    SWIFT,
    Application,
    available_drivers,
    build_go_driver,
    build_rust_examples,
    case_base,
    driver_error,
    driver_name,
    kill,
    normalized_path,
    store_delete,
    store_write,
    tree_snapshot,
    use_python_paths,
    wait_for,
)

use_python_paths("python/src")

from flyrail._codec import decode
from flyrail._resource_models import Receipt
from flyrail._state import resource_key

SOURCE_OPTIONS = ROOT / "spec/fixtures/interop-source-options.json"
BARRIERS = itertools.count()
# Scenarios whose error messages carry parser-detail wording are compared by error code only.
MESSAGE_RELAXATIONS = {}
# Python's driver injects test faults as OSError, whose text gains an errno prefix the TargetError drivers lack.
INJECTED_FAULT = re.compile(r"(?:\[Errno 5\] )?(injected [a-z-]+ fault)")
SKIPPED_SWIFT_ONLY = [
    "write/recovery SIGKILL boundaries and incomplete preparations (barriers beyond prepare|receipt-committed)",
    "same-generation repairs (--fault-phase and the backup barrier)",
    "predictable_oversize (Swift's whole-plan record budget)",
    "readonly_tree (Python's and Rust's drivers lack `--kind tree`; at library level Python, Rust and Go all end"
    " `partial` then stuck `incomplete` on a read-only tree takeover, while Swift refuses it up front)",
]


def comparable(value, root):
    if isinstance(value, list):
        return [comparable(item, root) for item in value]
    if isinstance(value, dict):
        return {key: comparable(item, root) for key, item in value.items()}
    if isinstance(value, str):
        injected = INJECTED_FAULT.fullmatch(value)
        return injected.group(1) if injected else normalized_path(value, root)
    return value


def without_messages(value):
    if isinstance(value, list):
        return [without_messages(item) for item in value]
    if isinstance(value, dict):
        return {key: without_messages(item) for key, item in value.items() if key != "message"}
    return value


def security_records(value, root, found=None):
    """List every Node and Ancestor security payload, which harness.normalized_record blanks, with its path."""
    found = [] if found is None else found
    if isinstance(value, list):
        for item in value:
            security_records(item, root, found)
    elif isinstance(value, dict):
        fields = value.get("fields")
        if value.get("type") in {"Node", "Ancestor"} and isinstance(fields, dict):
            path = fields["path"]["path"] if isinstance(fields["path"], dict) else fields["path"]
            found.append([value["type"], normalized_path(path, root), fields["security"]["bytes"]])
        for item in value.values():
            security_records(item, root, found)
    return found


class GoApplication(Application):
    def snapshot(self):
        raw = {key: json.loads(data) for key, data in self.records().items()}
        return {
            "records": self.normalized_records(),
            "security": {normalized_path(key, self.root): security_records(record, self.root)
                         for key, record in raw.items()},
        }

    def step(self, value, *paths):
        return {"result": comparable(value, self.root), "files": self.files(*paths), **self.snapshot()}


def first_difference(left, right, where=""):
    if isinstance(left, dict) and isinstance(right, dict):
        if left.keys() != right.keys():
            return where, sorted(left.keys() ^ right.keys())
        return next((found for key in left if (found := first_difference(left[key], right[key], f"{where}/{key}"))),
                    None)
    if isinstance(left, list) and isinstance(right, list) and len(left) == len(right):
        return next((found for index, (a, b) in enumerate(zip(left, right))
                     if (found := first_difference(a, b, f"{where}/{index}"))), None)
    return None if left == right else (where, left, right)


def compare_with_go(base, scenario, drivers, name=None):
    name = name or scenario.__name__
    root = Path(base) / "case"
    outcomes = {}
    for driver in drivers:
        root.mkdir()
        try:
            outcomes[driver_name(driver)] = scenario(GoApplication(driver, root, root / "state"))
        except AssertionError as error:
            raise AssertionError(f"{name}: {driver_name(driver)}: {error}") from error
        finally:
            shutil.rmtree(root)
    expected = outcomes.pop("go")
    relaxed = name in MESSAGE_RELAXATIONS
    for other, outcome in outcomes.items():
        left, right = (without_messages(expected), without_messages(outcome)) if relaxed else (expected, outcome)
        assert left == right, (name, "go", other, first_difference(left, right))
    return list(outcomes)


def success(result, value):
    assert result.returncode == 0, (result.returncode, result.stderr, value)
    assert value["status"] in {"applied", "unchanged"}, value
    assert value["error"] is None, value


def failed(result, value, code):
    assert result.returncode == 0, (result.returncode, result.stderr, value)
    assert value["status"] == "failed" and value["error"]["code"] == code, value


def resource_records(records, name):
    return [value for key, value in records.items() if key.endswith(f"/{name}")]


def start_barrier(app, command, phase, *arguments):
    number = next(BARRIERS)
    ready, release = app.root / f"ready-{number}", app.root / f"release-{number}"
    process = subprocess.Popen(
        app.command(command, *arguments, "--barrier-phase", phase, "--barrier-ready", ready,
                    "--barrier-release", release, "--barrier-timeout", "60"),
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=ENV, cwd=ROOT,
    )
    try:
        wait_for(ready, process, timeout=60)
    except BaseException:
        if process.poll() is None:
            process.kill()
        process.communicate(timeout=15)
        raise
    return process


def takeover_update_remove(app):
    destination = app.root / "AGENTS.md"
    destination.write_bytes(b"original\n")
    destination.chmod(0o600)
    written, value = app.write("write", destination, "--payload", "first\n", "--acquisition", "takeover")
    success(written, value)
    assert destination.read_bytes() == b"first\n"
    steps = [app.step(value, destination)]
    inspected, observation = app.target_command("inspect")
    assert inspected.returncode == 0 and observation["is_current"] and observation["version"] == "1", observation
    steps.append(comparable(observation, app.root))
    updated, value = app.write("update", destination, "--version", "2", "--payload", "second\n")
    success(updated, value)
    assert destination.read_bytes() == b"second\n"
    steps.append(app.step(value, destination))
    removed, value = app.target_command("remove")
    success(removed, value)
    assert destination.read_bytes() == b"original\n"
    assert destination.stat().st_mode & 0o777 == 0o600
    steps.append(app.step(value, destination))
    return steps


def adoption(app):
    destination = app.root / "adopted"
    destination.write_bytes(b"same bytes")
    identity = destination.stat().st_dev, destination.stat().st_ino
    written, value = app.write("write", destination, "--payload", "same bytes", "--acquisition", "adopt")
    success(written, value)
    assert (destination.stat().st_dev, destination.stat().st_ino) == identity
    receipts = [decode(raw) for key, raw in app.records().items() if key.endswith("/receipt")]
    assert len(receipts) == 1 and isinstance(receipts[0], Receipt), receipts
    assert receipts[0].claims[0].disposition == "adopted", receipts
    steps = [app.step(value, destination)]
    removed, value = app.target_command("remove")
    success(removed, value)
    assert not destination.exists()
    steps.append(app.step(value, destination))
    return steps


def retarget(app):
    first, second = app.root / "first", app.root / "second"
    first.write_bytes(b"first baseline")
    second.write_bytes(b"second baseline")
    first.chmod(0o600)
    second.chmod(0o640)
    steps = []
    for destination, version, data in [(first, "1", "first generated"), (second, "2", "second generated")]:
        result, value = app.write("update", destination, "--version", version, "--payload", data,
                                  "--acquisition", "takeover")
        success(result, value)
        steps.append(app.step(value, first, second))
    assert first.read_bytes() == b"first baseline" and first.stat().st_mode & 0o777 == 0o600
    assert second.read_bytes() == b"second generated"
    inspected, observation = app.target_command("inspect")
    assert inspected.returncode == 0 and observation["version"] == "2", observation
    steps.append(comparable(observation, app.root))
    removed, value = app.target_command("remove")
    success(removed, value)
    assert second.read_bytes() == b"second baseline" and second.stat().st_mode & 0o777 == 0o640
    steps.append(app.step(value, first, second))
    return steps


def refused_spelling(app, destination, watched):
    before = app.records() if app.state.exists() else {}, watched.read_bytes()
    result, value = app.write("update", destination, "--version", "9", "--payload", "refused\n")
    # Python's lifecycle raises this ValueError to the driver instead of returning a failed result.
    if app.driver is PYTHON and result.returncode != 0:
        assert result.returncode == 1 and value["error"]["code"] is None, (result, value)
    else:
        failed(result, value, "invalid_state")
    assert (app.records() if app.state.exists() else {}, watched.read_bytes()) == before
    return value["error"]["message"]


def distinct_spellings(app, destination, decomposed, aliases):
    """On a filesystem that keeps every spelling distinct, record each driver's outcome for compare_with_go."""
    spellings = [destination, decomposed, *aliases]
    steps = []

    def attempt(spelling, version):
        result, value = app.write("update", spelling, "--version", str(version), "--payload", f"v{version}\n",
                                  "--acquisition", "takeover")
        steps.append({"exit": result.returncode, **app.step(value, *spellings)})

    attempt(destination, 1)
    decomposed.unlink()
    destination.write_bytes(b"baseline\n")
    attempt(destination, 2)
    steps.append(refused_spelling(app, decomposed, destination))
    for version, alias in enumerate(aliases, start=3):
        attempt(alias, version)
    removed, value = app.target_command("remove")
    steps.append({"exit": removed.returncode, **app.step(value, *spellings)})
    return steps


def unicode_destination(app):
    name = "Ångström-ΣΊΣΥΦΟΣ.md"
    destination = app.root / unicodedata.normalize("NFC", name)
    decomposed = app.root / unicodedata.normalize("NFD", name)
    assert destination.name != decomposed.name
    aliases = [app.root / unicodedata.normalize("NFC", name.lower()),
               app.root / unicodedata.normalize("NFC", name.casefold())]
    decomposed.write_bytes(b"decomposed\n")
    if not all(spelling.exists() for spelling in [destination, *aliases]):
        return distinct_spellings(app, destination, decomposed, aliases)
    steps = [refused_spelling(app, destination, decomposed)]
    decomposed.unlink()
    destination.write_bytes(b"baseline\n")
    for alias in aliases:
        assert alias.exists() and alias.samefile(destination), f"{alias.name} does not resolve to {destination.name}"
    spellings = [destination, *aliases]
    for version, spelling in enumerate(spellings, start=1):
        arguments = ["--acquisition", "takeover"] if version == 1 else []
        result, value = app.write("update", spelling, "--version", str(version), "--payload", f"v{version}\n",
                                  *arguments)
        success(result, value)
        steps.append(app.step(value, destination))
        authorities = [key for key in app.records() if key.endswith("/authority")]
        assert authorities == [resource_key(destination, "authority")], authorities
    steps.append(refused_spelling(app, decomposed, destination))
    names = [path.name for path in app.root.iterdir() if path.name.endswith(".md")]
    assert names == [destination.name], names
    assert destination.read_bytes() == f"v{len(spellings)}\n".encode()
    removed, value = app.target_command("remove")
    success(removed, value)
    assert destination.read_bytes() == b"baseline\n"
    steps.append(app.step(value, destination))
    return steps


def index_failure(app):
    destination = app.root / "resource"
    destination.write_bytes(b"foreign")
    success(*app.write("write", destination, "--payload", "first", "--acquisition", "takeover"))
    result, value = app.write("update", destination, "--version", "2", "--payload", "second",
                              "--fail-index-completion")
    assert result.returncode == 0 and value["status"] == "partial", value
    assert value["error"]["code"] == "io_error", value
    steps = [app.step(value, destination)]
    inspected, observation = app.target_command("inspect")
    assert inspected.returncode == 0, inspected
    assert not observation["is_current"] and observation["pending"] and observation["version"] == "1", observation
    assert destination.read_bytes() == b"second"
    steps.append(comparable(observation, app.root))
    recovered, value = app.target_command("recover")
    success(recovered, value)
    steps.append(app.step(value, destination))
    inspected, observation = app.target_command("inspect")
    assert not observation["is_current"] and observation["pending"] and observation["version"] == "1", observation
    assert observation["resources"][0]["recovery_paths"] == [], observation
    steps.append(comparable(observation, app.root))
    removed, value = app.target_command("remove")
    success(removed, value)
    assert destination.read_bytes() == b"foreign"
    steps.append(app.step(value, destination))
    return steps


def interrupted_writer(app, phase):
    destination = app.root / "large"
    destination.write_bytes(b"baseline")
    success(*app.write("write", destination, "--payload", "installed", "--acquisition", "takeover"))
    steps = []
    process = start_barrier(app, "update", phase, "--destination", destination, "--version", "2",
                            "--payload", "updated")
    try:
        snapshot = app.snapshot()
        records = snapshot["records"]
        assert len(resource_records(records, "preparation")) == 1, records
        assert len(resource_records(records, "journal")) == 1, records
        steps.append(snapshot)
        competing, busy = app.write("update", destination, "--version", "3", "--payload", "competing",
                                    "--lock-timeout", "0.01")
        failed(competing, busy, "busy")
        steps.append(comparable(busy, app.root))
    finally:
        kill(process)
    expected = b"installed" if phase == "prepare" else b"updated"
    assert destination.read_bytes() == expected
    inspected, observation = app.target_command("inspect")
    assert inspected.returncode == 0 and observation["pending"], observation
    recovery_paths = observation["resources"][0]["recovery_paths"]
    assert recovery_paths and [Path(path).parent for path in recovery_paths] == [app.state / "staging"], observation
    steps.append(comparable(observation, app.root))
    recovered, value = app.target_command("recover")
    success(recovered, value)
    steps.append(app.step(value, destination))
    inspected, observation = app.target_command("inspect")
    assert observation["pending"] and observation["version"] == "1", observation
    assert observation["resources"][0]["recovery_paths"] == [], observation
    assert destination.read_bytes() == expected
    steps.append(comparable(observation, app.root))
    removed, value = app.target_command("remove")
    success(removed, value)
    assert destination.read_bytes() == b"baseline"
    steps.append(app.step(value, destination))
    return steps


def foreign_security(app, attribute):
    destination = app.root / attribute
    destination.write_bytes(b"foreign")
    destination.chmod(0o640)
    if attribute == "acl":
        command = ["chmod", "+a", "everyone allow read", str(destination)]
        cleanup = ["chmod", "-N", str(destination)]
    else:
        command = ["xattr", "-w", "flyrail.interop", "value", str(destination)]
        cleanup = ["xattr", "-d", "flyrail.interop", str(destination)]
    subprocess.run(command, check=True, capture_output=True, timeout=30)
    try:
        result, value = app.write("write", destination, "--payload", "generated", "--acquisition", "takeover")
        failed(result, value, "unsupported")
        assert destination.read_bytes() == b"foreign"
        assert destination.stat().st_mode & 0o777 == 0o640
        return [app.step(value, destination)]
    finally:
        subprocess.run(cleanup, check=True, capture_output=True, timeout=30)


def write_manifest(source, manifest):
    (source / "flyrail.json").write_text(json.dumps(manifest))


def source_project(app, manifest, agent):
    project, source = app.root / "project", app.root / "source"
    project.mkdir()
    source.mkdir()
    write_manifest(source, manifest)
    node = shutil.which("node")
    assert node, "node is required for the hook runtime source graphs"
    return project, source, ("--source", source, "--agent", agent, "--node", node)


def project_snapshot(app):
    return {path: data for path, data in tree_snapshot(app.root, modes=True).items()
            if not Path(path).is_relative_to(app.state)}


def first_install(app, project, arguments, steps):
    result, value = app.write("write", project, *arguments)
    success(result, value)
    steps.append(app.step(value))
    inspected, observation = app.target_command("inspect")
    assert inspected.returncode == 0 and observation["is_current"], observation
    steps.append(comparable(observation, app.root))
    return observation


def missing_receipt(app, observation, steps):
    key = resource_key(Path(observation["resources"][0]["destination"]), "receipt")
    original = app.records()
    store_delete(app.state, key, original[key])
    remaining = {name: raw for name, raw in original.items() if name != key}
    before = project_snapshot(app)
    for command in ["inspect", "recover", "remove"]:
        result, value = app.target_command(command)
        assert result.returncode == 0, (command, result.stderr)
        if command == "inspect":
            assert not value["is_current"], value
            assert value["resources"][0]["error"]["code"] == "invalid_state", value
        else:
            failed(result, value, "invalid_state")
        steps.append(comparable(value, app.root))
        assert project_snapshot(app) == before, command
        assert app.records() == remaining, command
    store_write(app.state, key, original[key], None)
    assert app.records() == original


def upgrade(app, project, arguments, steps):
    result, value = app.write("write", project, *arguments)
    success(result, value)
    steps.append(app.step(value))
    inspected, observation = app.target_command("inspect")
    assert inspected.returncode == 0 and observation["is_current"], observation
    steps.append(comparable(observation, app.root))
    return observation


def partial_upgrade(app, project, arguments, steps):
    result, value = app.write("write", project, *arguments, "--fail-index-completion")
    assert result.returncode == 0 and value["status"] == "partial", value
    steps.append(app.step(value))
    return value


def source_free_removal(app, source, steps):
    shutil.rmtree(source)
    recovered, value = app.target_command("recover")
    success(recovered, value)
    steps.append(app.step(value))
    removed, value = app.target_command("remove")
    success(removed, value)
    steps.append(app.step(value))
    inspected, observation = app.target_command("inspect")
    assert inspected.returncode == 0 and observation["resources"] == [], observation
    steps.append(comparable(observation, app.root))


def asset_source_graph(app):
    manifest = json.loads((ROOT / "spec/fixtures/translations.json").read_bytes())["asset_manifest"]
    manifest |= {"id": "team", "version": "1"}
    manifest["artifacts"].append({"kind": "instruction", "id": "guide", "text": "first"})
    project, source, arguments = source_project(app, manifest, "codex")
    (source / "runtime/data").mkdir(parents=True)
    (source / "runtime/server.js").write_bytes(b'console.log("MCP");')
    (project / "AGENTS.md").write_bytes(b"foreign\n")
    steps = []
    first = first_install(app, project, arguments, steps)
    assert list(project.rglob("server.js")), first
    missing_receipt(app, first, steps)
    manifest["version"] = "2"
    manifest["artifacts"][-1]["text"] = "updated"
    (source / "runtime/server.js").write_bytes(b'console.log("updated");')
    write_manifest(source, manifest)
    upgrade(app, project, arguments, steps)
    manifest["version"] = "3"
    write_manifest(source, manifest)
    partial_upgrade(app, project, arguments, steps)
    source_free_removal(app, source, steps)
    assert (project / "AGENTS.md").read_bytes() == b"foreign\n"
    assert not list(project.rglob("server.js"))
    return steps


def stable_hook_contract(app):
    receipts = [decode(raw) for key, raw in app.records().items() if key.endswith("/receipt")]
    stable = tuple(pair for receipt in receipts for claim in receipt.claims for pair in claim.stable_requirements)
    assert len(stable) == 1, stable
    launcher, claim_id = stable[0]
    routing = next(receipt for receipt in receipts if receipt.resource.destination == launcher)
    owned = next(claim for claim in routing.claims if claim.claim_id == claim_id)
    assert owned.requirements and owned.stable_requirements == ()
    assert launcher.is_file()
    return stable


def hook_source_graph(app):
    manifest = {
        "schema_version": 2, "id": "team", "version": "1",
        "artifacts": [{
            "kind": "hook", "id": "start", "protocol_version": 1,
            "event": "session-start", "command": {"argv": ["/bin/echo", "synthetic"]},
            "outcomes": ["continue"], "timeout_ms": 1000,
        }],
    }
    project, source, arguments = source_project(app, manifest, "claude")
    steps = []
    first = first_install(app, project, arguments, steps)
    stable = stable_hook_contract(app)
    config = project / ".claude/settings.json"
    config_identity = config.stat().st_ino, config.read_bytes()

    def launcher():
        assert stable_hook_contract(app) == stable
        assert (config.stat().st_ino, config.read_bytes()) == config_identity
        return stable[0][0].read_bytes()

    first_launcher = launcher()
    assert list(project.rglob("runner.mts")), first
    missing_receipt(app, first, steps)
    manifest["version"] = "2"
    manifest["artifacts"][0]["command"]["argv"][-1] = "synthetic-updated"
    write_manifest(source, manifest)
    upgrade(app, project, arguments, steps)
    second_launcher = launcher()
    assert second_launcher != first_launcher
    manifest["version"] = "3"
    manifest["artifacts"][0]["command"]["argv"][-1] = "synthetic-partial"
    write_manifest(source, manifest)
    partial_upgrade(app, project, arguments, steps)
    partial_launcher = launcher()
    assert partial_launcher != second_launcher
    shutil.rmtree(source)
    recovered, value = app.target_command("recover")
    success(recovered, value)
    steps.append(app.step(value))
    assert launcher() == partial_launcher
    removed, value = app.target_command("remove")
    success(removed, value)
    steps.append(app.step(value))
    inspected, observation = app.target_command("inspect")
    assert inspected.returncode == 0 and observation["resources"] == [], observation
    steps.append(comparable(observation, app.root))
    assert not list(project.rglob("runner.mts"))
    return steps


def source_option_validation(app):
    source = app.root / "source"
    source.mkdir()
    write_manifest(source, {
        "schema_version": 2, "id": "team", "version": "1",
        "artifacts": [{"kind": "instruction", "id": "guide", "text": "safe"}],
    })
    options = json.loads(SOURCE_OPTIONS.read_bytes())["cases"]
    assert options
    cases = [(option, value, f"{option} cannot be combined with --source") for option, value in options]
    cases.append(("--bundle-id", "other", "source bundle id must match --bundle-id"))
    messages = []
    for option, value, expected in cases:
        before = tree_snapshot(app.root, modes=True)
        command = app.command("write", "--destination", app.root / "project", "--source", source, option, value)
        result = subprocess.run(command, env=ENV, cwd=ROOT, capture_output=True, timeout=30, check=False)
        assert result.returncode == 1, (option, result)
        message = driver_error(result)
        assert message == expected, (option, message)
        assert tree_snapshot(app.root, modes=True) == before
        assert not app.state.exists() and not (app.root / "project").exists()
        messages.append(message)
    return messages


def section_block(marker, text):
    return f"<!-- flyrail:{marker}:start -->\n{text}<!-- flyrail:{marker}:end -->\n".encode()


def coexistence(base, first, second):
    root = base / f"coexistence-{driver_name(first)}-{driver_name(second)}"
    root.mkdir()
    names = [driver_name(first), driver_name(second)]
    apps = {name: Application(driver, root, root / f"{name}-state") for name, driver in zip(names, [first, second])}
    shared = root / "AGENTS.md"
    baseline = b"# Project\n"
    shared.write_bytes(baseline)
    for name in names:
        success(*apps[name].write("write", shared, "--section", name, "--payload", f"{name} guidance\n"))
    data = shared.read_bytes()
    assert data.startswith(baseline)
    for name in names:
        assert section_block(name, f"{name} guidance\n") in data, (name, data)
        inspected, observation = apps[name].target_command("inspect")
        assert inspected.returncode == 0 and observation["is_current"], (name, observation)
    survivor = section_block(names[1], f"{names[1]} guidance\n")
    success(*apps[names[0]].target_command("remove"))
    data = shared.read_bytes()
    assert section_block(names[0], f"{names[0]} guidance\n") not in data, data
    assert data.count(survivor) == 1 and data.startswith(baseline), data
    inspected, observation = apps[names[1]].target_command("inspect")
    assert observation["is_current"], observation
    assert observation["resources"][0]["claims"][0]["disposition"] == "current", observation
    success(*apps[names[1]].target_command("remove"))
    assert shared.read_bytes() == baseline

    whole = root / "owned"
    apps = {name: Application(driver, root, root / f"{name}-whole-state")
            for name, driver in zip(names, [first, second])}
    success(*apps[names[0]].write("write", whole, "--payload", f"{names[0]} file\n"))
    whole_mode = whole.stat().st_mode & 0o777
    result, value = apps[names[1]].write("write", whole, "--payload", f"{names[1]} file\n")
    failed(result, value, "conflict")
    assert whole.read_bytes() == f"{names[0]} file\n".encode()
    assert whole.stat().st_mode & 0o777 == whole_mode
    inspected, observation = apps[names[0]].target_command("inspect")
    assert observation["is_current"], observation


def rust_available():
    if shutil.which("cargo", path=ENV.get("PATH")) is None:
        assert os.environ.get("CI") != "true", "cargo not found on PATH, but CI requires comparing Go with Rust"
        print("cargo not found on PATH, comparing Go without Rust", file=sys.stderr)
        return False
    build_rust_examples("interop")
    return True


def swift_available():
    if sys.platform != "darwin":
        return False
    if shutil.which("swift", path=ENV.get("PATH")) is None:
        assert os.environ.get("CI") != "true", "swift not found on PATH, but CI requires comparing Go with Swift on macOS"
        print("swift not found on PATH, comparing Go without Swift", file=sys.stderr)
        return False
    subprocess.run(["just", "build"], env=ENV, cwd=ROOT / "swift", check=True, timeout=3600)
    return True


def main():
    build_go_driver()
    names = ["go", "python"] + (["rust"] if rust_available() else []) + (["swift"] if swift_available() else [])
    drivers = available_drivers(*names)
    assert [driver_name(driver) for driver in drivers] == names, ("missing interop driver", names)
    assert drivers[0] is GO and PYTHON in drivers
    assert RUST in drivers or "rust" not in names
    assert SWIFT in drivers or "swift" not in names
    base = case_base("go-transaction-interop")
    counts = dict.fromkeys(names[1:], 0)
    scenarios = [source_option_validation, takeover_update_remove, adoption, retarget, unicode_destination,
                 index_failure, asset_source_graph, hook_source_graph]
    scenarios += [lambda app, phase=phase: interrupted_writer(app, phase) for phase in ["prepare", "receipt-committed"]]
    labels = [scenario.__name__ for scenario in scenarios[:-2]]
    labels += ["interrupted_writer-prepare", "interrupted_writer-receipt-committed"]
    if sys.platform == "darwin":
        for attribute in ["acl", "xattr"]:
            scenarios.append(lambda app, attribute=attribute: foreign_security(app, attribute))
            labels.append(f"foreign_security-{attribute}")
    try:
        for scenario, label in zip(scenarios, labels, strict=True):
            for other in compare_with_go(base, scenario, drivers, label):
                counts[other] += 1
        pairs = [pair for pair in itertools.permutations(drivers, 2) if GO in pair]
        for first, second in pairs:
            coexistence(base, first, second)
    finally:
        shutil.rmtree(base)
    compared = ", ".join(f"Go/{name.capitalize()} {count}" for name, count in counts.items())
    relaxed = ", ".join(sorted(MESSAGE_RELAXATIONS)) or "none"
    print(f"Go transaction interop: {len(scenarios)} scenarios with equal results, files, store records and "
          f"unnormalized security records ({compared}); {len(pairs)} separate-store coexistence pairs; "
          f"message relaxations: {relaxed} (injected-fault errno prefix normalized); skipped Swift-only: {'; '.join(SKIPPED_SWIFT_ONLY)}")


if __name__ == "__main__":
    main()
