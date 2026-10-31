import itertools
import json
import re
import shutil
import subprocess
import sys
import unicodedata
from pathlib import Path

from flyrail._codec import decode
from flyrail._resource_models import Receipt
from flyrail._state import resource_key

from harness import (
    DRIVERS, ENV, PYTHON, ROOT, SWIFT, Application, build_rust_examples, case_base, comparable,
    compare_languages, driver_error, driver_name, kill, store_delete, store_write, tree_snapshot, wait_for,
)

SOURCE_OPTIONS = ROOT / "swift/Tests/FlyrailTests/Fixtures/interop-source-options.json"
KERNEL = ROOT / "swift/Sources/Flyrail/ResourceTransaction.swift"
INCOMPLETE = ["preparation", "directory", "stage-node", "staged", "journal"]
RECOVERY_KILLS = ["rollback-stage", "rollback-backup", "rollback", "cleanup-node", "cleanup"]
BARRIERS = itertools.count()


def kernel_boundaries():
    source = KERNEL.read_text()

    def names(label):
        match = re.search(rf"static let {label} = \[(.*?)\]", source, re.S)
        assert match, label
        elements = [element.strip() for element in match.group(1).split(",") if element.strip()]
        parsed = [re.fullmatch(r'"([^"]+)"', element) for element in elements]
        assert all(parsed), (label, elements)
        return [item.group(1) for item in parsed]

    recovery = names("recoveryBoundaries")
    forward = [name for name in names("boundaries") if name not in recovery]
    assert set(recovery) <= set(names("boundaries")) and "receipt-reread" in forward
    seams = "".join(path.read_text() for path in KERNEL.parent.glob("*.swift"))
    unused = [name for name in names("boundaries") if f'seam("{name}")' not in seams]
    assert not unused, unused
    return forward


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


def staging_entries(app):
    staging = app.state / "staging"
    return sorted(path.name for path in staging.iterdir()) if staging.exists() else []


def transaction_entries(app, directory):
    return sorted(path.name for path in directory.iterdir() if path.name.startswith(".flyrail-"))


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


def unicode_destination(app):
    name = "Ångström-ΣΊΣΥΦΟΣ.md"
    destination = app.root / unicodedata.normalize("NFC", name)
    decomposed = app.root / unicodedata.normalize("NFD", name)
    assert destination.name != decomposed.name
    decomposed.write_bytes(b"decomposed\n")
    steps = [refused_spelling(app, destination, decomposed)]
    decomposed.unlink()
    destination.write_bytes(b"baseline\n")
    aliases = [app.root / unicodedata.normalize("NFC", name.lower()),
               app.root / unicodedata.normalize("NFC", name.casefold())]
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
        records = app.normalized_records()
        assert len(resource_records(records, "preparation")) == 1, records
        assert len(resource_records(records, "journal")) == 1, records
        steps.append(records)
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
    assert node
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
        result = subprocess.run(command, env=ENV, cwd=ROOT, capture_output=True, timeout=30)
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


def installed(app, destination, baseline=b"baseline", payload="first"):
    destination.write_bytes(baseline)
    success(*app.write("write", destination, "--payload", payload, "--acquisition", "takeover"))


def converged(app, destination, label, receipt):
    inspected, observation = app.target_command("inspect")
    assert inspected.returncode == 0 and observation["pending"], (label, observation)
    assert observation["version"] == "1" and not observation["is_current"], (label, observation)
    assert all(not item["recovery_paths"] for item in observation["resources"]), (label, observation)
    records = app.records()
    assert records[resource_key(destination, "receipt")] == receipt, label
    assert not [key for key in records if key.endswith(("/preparation", "/journal"))], (label, list(records))
    assert staging_entries(app) == [], (label, staging_entries(app))
    assert transaction_entries(app, destination.parent) == [], label


def write_interruption(app, boundary, forward):
    destination = app.root / "document"
    installed(app, destination)
    receipt = resource_key(destination, "receipt")
    previous = app.records()[receipt]
    committed = forward.index(boundary) >= forward.index("receipt-reread")
    process = start_barrier(app, "write", boundary, "--destination", destination, "--payload", "second",
                            "--version", "2")
    try:
        records = app.records()
        expected = records[receipt]
        if committed:
            assert expected != previous, boundary
            if resource_key(destination, "journal") in records:
                assert decode(expected) == decode(records[resource_key(destination, "journal")]).receipt, boundary
        else:
            assert expected == previous, boundary
    finally:
        kill(process)
    for _ in range(2):
        success(*app.target_command("recover"))
    assert destination.read_bytes() == (b"second" if committed else b"first"), boundary
    converged(app, destination, boundary, expected)
    success(*app.target_command("remove"))
    assert destination.read_bytes() == b"baseline", boundary


def recovery_interruption(app, boundary):
    destination = app.root / "document"
    installed(app, destination)
    previous = app.records()[resource_key(destination, "receipt")]
    kill(start_barrier(app, "write", "publish", "--destination", destination, "--payload", "second",
                       "--version", "2"))
    kill(start_barrier(app, "recover", boundary))
    success(*app.target_command("recover"))
    assert destination.read_bytes() == b"first", boundary
    converged(app, destination, boundary, previous)
    success(*app.target_command("remove"))
    assert destination.read_bytes() == b"baseline", boundary


def incomplete_preparation(app, boundary):
    destination = app.root / "document"
    installed(app, destination)
    previous = app.records()[resource_key(destination, "receipt")]
    kill(start_barrier(app, "write", boundary, "--destination", destination, "--payload", "second",
                       "--version", "2"))
    records = app.records()
    assert len([key for key in records if key.endswith("/preparation")]) == 1, (boundary, list(records))
    assert not [key for key in records if key.endswith("/journal")], (boundary, list(records))
    success(*app.target_command("recover"))
    assert destination.read_bytes() == b"first", boundary
    converged(app, destination, boundary, previous)
    success(*app.target_command("remove"))
    assert destination.read_bytes() == b"baseline", boundary


def same_generation_repair(app, interrupted):
    destination = app.root / "document"
    installed(app, destination, b"baseline\n", "generated\n")
    receipt = resource_key(destination, "receipt")
    previous = app.records()[receipt]
    destination.write_bytes(b"local edited content\n")
    identity = destination.stat().st_ino
    arguments = ("--destination", destination, "--payload", "generated\n", "--replace-modified")
    if interrupted:
        kill(start_barrier(app, "write", "backup", *arguments))
        records = app.records()
        journal = decode(records[resource_key(destination, "journal")])
        preparation = decode(records[resource_key(destination, "preparation")])
        assert journal.previous.transaction_id != journal.receipt.transaction_id
        assert not destination.exists()
        assert preparation.directory.parent == app.state / "staging", preparation
        assert (preparation.directory / "backup/resource").read_bytes() == b"local edited content\n"
        assert (preparation.directory / "staging/resource").read_bytes() == b"generated\n"
    else:
        result, value = app.invoke("write", *app.target, *map(str, arguments), "--fault-phase", "backup")
        failed(result, value, "io_error")
        assert value["resources"][0]["status"] == "failed", value
    for _ in range(2):
        success(*app.target_command("recover"))
        assert destination.read_bytes() == b"local edited content\n"
        assert destination.stat().st_ino == identity
        assert app.records()[receipt] == previous
        converged(app, destination, ("same-generation", interrupted), previous)
    success(*app.invoke("write", *app.target, *map(str, arguments)))
    success(*app.target_command("remove"))
    assert destination.read_bytes() == b"baseline\n"


def refused_without_transaction(app, destination, code, message=None):
    result, value = app.write("write", destination, *(["--kind", "tree"] if code == "unsupported" else []),
                              "--payload", "new\n", "--acquisition", "takeover")
    failed(result, value, code)
    if message is not None:
        assert value["error"]["message"] == message, value
    records = app.records()
    assert list(records) == [resource_key(destination, "authority")], list(records)
    assert staging_entries(app) == [] and transaction_entries(app, destination.parent) == []
    success(*app.target_command("recover"))
    success(*app.target_command("remove"))


def predictable_oversize(app):
    destination = app.root / "document"
    large = b"a" * (3 * 1024 * 1024)
    destination.write_bytes(large)
    identity = destination.stat().st_ino
    for _ in range(2):
        refused_without_transaction(app, destination, "invalid_state", "JSON input exceeds the byte limit")
        assert destination.stat().st_ino == identity and destination.read_bytes() == large
    installed(app, destination, b"small baseline")
    success(*app.target_command("remove"))
    assert destination.read_bytes() == b"small baseline"


def readonly_tree(app):
    destination = app.root / "tree"
    readonly = destination / "readonly"
    readonly.mkdir(parents=True)
    (readonly / "old").write_bytes(b"baseline")
    readonly.chmod(0o555)
    try:
        for _ in range(2):
            refused_without_transaction(app, destination, "unsupported")
            assert (readonly / "old").read_bytes() == b"baseline"
            assert readonly.stat().st_mode & 0o777 == 0o555
    finally:
        readonly.chmod(0o755)
    success(*app.write("write", destination, "--kind", "tree", "--acquisition", "takeover"))
    success(*app.target_command("remove"))
    assert (readonly / "old").read_bytes() == b"baseline"
    assert readonly.stat().st_mode & 0o777 == 0o755


def swift_case(base, name, scenario, *arguments):
    root = base / name
    root.mkdir()
    scenario(Application(SWIFT, root, root / "state"), *arguments)


def main():
    build_rust_examples("interop")
    forward = kernel_boundaries()
    base = case_base("swift-transaction-interop")
    try:
        options = compare_languages(base, source_option_validation)
        compared = [takeover_update_remove, adoption, retarget, unicode_destination, index_failure,
                    asset_source_graph, hook_source_graph]
        for scenario in compared:
            compare_languages(base, scenario)
        phases = ["prepare", "receipt-committed"]
        for phase in phases:
            compare_languages(base, lambda app, phase=phase: interrupted_writer(app, phase))
        attributes = ["acl", "xattr"] if sys.platform == "darwin" else []
        for attribute in attributes:
            compare_languages(base, lambda app, attribute=attribute: foreign_security(app, attribute))
        pairs = [pair for pair in itertools.permutations(DRIVERS, 2) if SWIFT in pair]
        for first, second in pairs:
            coexistence(base, first, second)
        for boundary in forward:
            swift_case(base, f"write-{boundary}", write_interruption, boundary, forward)
        for boundary in RECOVERY_KILLS:
            swift_case(base, f"recover-{boundary}", recovery_interruption, boundary)
        for boundary in INCOMPLETE:
            swift_case(base, f"incomplete-{boundary}", incomplete_preparation, boundary)
        for interrupted in [False, True]:
            swift_case(base, f"same-generation-{interrupted}", same_generation_repair, interrupted)
        for scenario in [predictable_oversize, readonly_tree]:
            swift_case(base, scenario.__name__, scenario)
    finally:
        shutil.rmtree(base)
    print(f"Transaction interop: {len(options)} rejected --source option combinations, "
          f"{len(compared) + len(phases) + len(attributes)} scenarios with equal store records across "
          f"{len(DRIVERS)} languages (unicode destination, source graphs with missing receipts, "
          f"{len(phases)} interrupted writers), {len(pairs)} separate-store coexistence pairs; Swift "
          f"{len(forward)} write and {len(RECOVERY_KILLS)} recovery SIGKILL boundaries, "
          f"{len(INCOMPLETE)} incomplete preparations, 2 same-generation repairs, oversize/read-only-tree "
          "regressions passed")


if __name__ == "__main__":
    main()
