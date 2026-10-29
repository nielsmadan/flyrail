import itertools
import json
import os
import signal
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from flyrail._codec import decode
from flyrail._resource_models import Receipt
from flyrail.authority import ResourceAuthority
from harness import ENV, PYTHON, ROOT, RUST, SWIFT, driver_error, run, tree_snapshot, use_python_paths

SOURCE_OPTIONS = ROOT / "swift/Tests/FlyrailTests/Fixtures/interop-source-options.json"


def invoke(driver, command, index, *arguments):
    return json.loads(run(driver, command, "--index", index, *arguments))


def success(value):
    assert value["status"] in {"applied", "unchanged"}, value
    assert value["error"] is None, value


def records(index, destination):
    values = [
        invoke(driver, "records", index, "--destination", destination)
        for driver in [SWIFT, PYTHON, RUST]
    ]
    assert values[0] == values[1] == values[2], values
    assert values[0]["records"]
    assert all(item["bytes"] == item["canonical"] for item in values[0]["records"])


def takeover(root, writer, reader):
    index, destination = root / "index", root / "document"
    destination.write_bytes(b"baseline\n")
    destination.chmod(0o600)
    success(invoke(writer, "write", index, "--destination", destination,
                   "--payload", "first\n", "--acquisition", "takeover"))
    records(index, destination)
    assert invoke(reader, "inspect", index)["is_current"]
    success(invoke(reader, "write", index, "--destination", destination,
                   "--version", "2", "--payload", "second\n"))
    assert destination.read_bytes() == b"second\n"
    records(index, destination)
    success(invoke(reader, "remove", index))
    assert destination.read_bytes() == b"baseline\n"
    assert destination.stat().st_mode & 0o777 == 0o600
    records(index, destination)


def adoption(root, writer, reader):
    index, destination = root / "index", root / "document"
    destination.write_bytes(b"equal bytes")
    identity = destination.stat().st_ino
    success(invoke(writer, "write", index, "--destination", destination,
                   "--payload", "equal bytes", "--acquisition", "adopt"))
    assert destination.stat().st_ino == identity
    receipt = decode((ResourceAuthority(destination).state_root / "receipt.json").read_bytes())
    assert isinstance(receipt, Receipt) and receipt.claims[0].disposition == "adopted"
    records(index, destination)
    success(invoke(reader, "remove", index))
    assert not destination.exists()


def retarget(root, writer, reader):
    index, first, second = root / "index", root / "first", root / "second"
    first.write_bytes(b"first baseline")
    second.write_bytes(b"second baseline")
    first.chmod(0o600)
    second.chmod(0o640)
    for driver, destination, version in [(writer, first, "1"), (reader, second, "2")]:
        success(invoke(driver, "write", index, "--destination", destination,
                       "--version", version, "--acquisition", "takeover"))
    assert first.read_bytes() == b"first baseline"
    assert invoke(writer, "inspect", index)["is_current"]
    success(invoke(writer, "remove", index))
    assert second.read_bytes() == b"second baseline"
    assert second.stat().st_mode & 0o777 == 0o640


def index_failure(root, writer, reader):
    index, destination = root / "index", root / "document"
    destination.write_bytes(b"baseline")
    success(invoke(writer, "write", index, "--destination", destination, "--acquisition", "takeover"))
    value = invoke(writer, "write", index, "--destination", destination, "--payload", "new",
                   "--version", "2", "--fail-index-completion")
    assert value["status"] == "partial" and value["error"]["code"] == "io_error", value
    observation = invoke(reader, "inspect", index)
    assert observation["pending"] and not observation["is_current"] and observation["version"] == "1", observation
    assert destination.read_bytes() == b"new"
    success(invoke(reader, "recover", index))
    assert invoke(reader, "inspect", index)["pending"]
    success(invoke(reader, "remove", index))
    assert destination.read_bytes() == b"baseline"


def start_barrier(driver, root, phase, command="write", extra=()):
    ready, release = root / "ready", root / "release"
    process = subprocess.Popen(
        driver + list(map(str, [
            command, "--index", root / "index", *extra,
            "--barrier-phase", phase, "--barrier-ready", ready,
            "--barrier-release", release, "--barrier-timeout", "45",
        ])),
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=ENV, cwd=ROOT,
    )
    deadline = time.monotonic() + 45
    while not ready.exists():
        if process.poll() is not None:
            raise AssertionError((phase, process.communicate()))
        if time.monotonic() > deadline:
            process.kill()
            raise AssertionError((phase, "barrier timed out", process.communicate()))
        time.sleep(0.01)
    assert process.poll() is None
    return process


def kill(process):
    try:
        os.kill(process.pid, signal.SIGKILL)
        stdout, stderr = process.communicate(timeout=15)
        assert process.returncode == -signal.SIGKILL, (stdout, stderr)
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate(timeout=15)


def interruption(root, writer, reader, phase):
    index, destination = root / "index", root / "document"
    destination.write_bytes(b"baseline")
    success(invoke(reader, "write", index, "--destination", destination,
                   "--payload", "first", "--acquisition", "takeover"))
    process = start_barrier(writer, root, phase, extra=(
        "--destination", destination, "--payload", "second", "--version", "2",
    ))
    try:
        busy = invoke(reader, "write", index, "--destination", destination, "--lock-timeout", "0.01")
        assert busy["status"] == "failed" and busy["error"]["code"] == "busy", busy
        records(index, destination)
    finally:
        kill(process)
    success(invoke(reader, "recover", index))
    success(invoke(reader, "recover", index))
    committed = phase in {
        "receipt-reread", "receipt-committed", "cleanup-node", "cleanup",
        "index-completion-prepared", "index-completion-reread", "index-completion-committed",
    }
    assert destination.read_bytes() == (b"second" if committed else b"first"), phase
    observation = invoke(reader, "inspect", index)
    assert all(not item["recovery_paths"] for item in observation["resources"]), observation
    success(invoke(reader, "remove", index))
    assert destination.read_bytes() == b"baseline"


def repeated_recovery(root, phase):
    index, destination = root / "index", root / "document"
    destination.write_bytes(b"baseline")
    success(invoke(SWIFT, "write", index, "--destination", destination,
                   "--payload", "first", "--acquisition", "takeover"))
    kill(start_barrier(SWIFT, root, "publish", extra=(
        "--destination", destination, "--payload", "second", "--version", "2",
    )))
    (root / "ready").unlink()
    kill(start_barrier(SWIFT, root, phase, command="recover"))
    success(invoke(SWIFT, "recover", index))
    success(invoke(PYTHON, "recover", index))
    success(invoke(RUST, "recover", index))
    assert destination.read_bytes() == b"first"
    success(invoke(RUST, "remove", index))
    assert destination.read_bytes() == b"baseline"

def incomplete_preparation(root, phase):
    index, destination = root / "index", root / "document"
    destination.write_bytes(b"baseline")
    success(invoke(SWIFT, "write", index, "--destination", destination,
                   "--payload", "first", "--acquisition", "takeover"))
    kill(start_barrier(SWIFT, root, phase, extra=(
        "--destination", destination, "--payload", "second", "--version", "2",
    )))

    def snapshot():
        return {
            str(path.relative_to(root)): (
                path.stat().st_ino, path.stat().st_mode,
                path.read_bytes() if path.is_file() else None,
            )
            for path in root.rglob("*")
        }

    def protected(records):
        return {
            path: record for path, record in records.items()
            if path != "document" and not {"staging", "backup"}.intersection(Path(path).parts)
        }

    before = snapshot()
    for reader in [SWIFT, PYTHON, RUST]:
        result = invoke(reader, "recover", index)
        assert result["status"] in {"failed", "incomplete"}, result
        assert result["error"]["code"] in {"invalid_state", "recovery_needed"}, result
        after = snapshot()
        if reader == SWIFT or phase != "receipt-created":
            assert after == before, (reader, phase)
        else:
            assert protected(after) == protected(before), (reader, phase)
            assert destination.read_bytes() in {b"first", b"second"}, (reader, phase)


def shared(root, writer, reader):
    section_driver = [sys.executable, "-B", str(Path(__file__).resolve()), "section-writer"]
    index, other_index, destination = root / "index", root / "other-index", root / "document"
    destination.write_bytes(b"foreign\n")
    success(invoke(writer, "write", index, "--destination", destination, "--section", "guide"))
    success(invoke(section_driver, "write", other_index, "--destination", destination))
    installed = destination.read_bytes()
    assert b"flyrail:other:start" in installed and b"flyrail:guide:start" in installed
    success(invoke(reader, "remove", index))
    assert b"flyrail:other:start" in destination.read_bytes()
    success(invoke(section_driver, "remove", other_index))
    assert destination.read_bytes() == b"foreign\n"


def same_generation_repair(root, reader, interrupted):
    index, destination = root / "index", root / "document"
    destination.write_bytes(b"baseline\n")
    success(invoke(SWIFT, "write", index, "--destination", destination,
                   "--payload", "generated\n", "--acquisition", "takeover"))
    state = ResourceAuthority(destination).state_root
    previous = (state / "receipt.json").read_bytes()
    destination.write_bytes(b"local edited content\n")
    identity = destination.stat().st_ino
    arguments = ("--destination", destination, "--payload", "generated\n", "--replace-modified")
    if interrupted:
        kill(start_barrier(SWIFT, root, "backup", extra=arguments))
        journal = decode((state / "transaction.json").read_bytes())
        assert journal.previous.transaction_id != journal.receipt.transaction_id
        assert not destination.exists()
        assert (state / "backup/resource").read_bytes() == b"local edited content\n"
        assert (state / "staging/resource").read_bytes() == b"generated\n"
        records(index, destination)
    else:
        value = invoke(SWIFT, "write", index, *arguments, "--fault-phase", "backup")
        assert value["status"] == "failed" and value["resources"][0]["status"] == "failed", value
    for _ in range(2):
        success(invoke(reader, "recover", index))
        assert destination.read_bytes() == b"local edited content\n"
        assert destination.stat().st_ino == identity
        assert (state / "receipt.json").read_bytes() == previous
        assert sorted(path.name for path in state.iterdir()) == [
            "authority.json", "backup", "lock", "receipt.json", "staging",
        ]
        assert list((state / "staging").iterdir()) == []
        assert list((state / "backup").iterdir()) == []
    success(invoke(SWIFT, "write", index, *arguments))
    success(invoke(reader, "remove", index))
    assert destination.read_bytes() == b"baseline\n"


def predictable_oversize(root):
    index, destination = root / "index", root / "document"
    destination.write_bytes(b"a" * (3 * 1024 * 1024))
    identity = destination.stat().st_ino
    for _ in range(2):
        value = invoke(SWIFT, "write", index, "--destination", destination,
                       "--payload", "new\n", "--acquisition", "takeover")
        assert value["status"] == "failed" and value["error"]["code"] == "invalid_state", value
        assert value["error"]["message"] == "JSON input exceeds the byte limit", value
        state = ResourceAuthority(destination).state_root
        assert sorted(path.name for path in state.iterdir()) == ["authority.json", "lock"]
        assert sorted(path.name for path in index.iterdir()) == ["lock"]
        assert destination.stat().st_ino == identity
        assert destination.read_bytes() == b"a" * (3 * 1024 * 1024)
        for reader in [SWIFT, PYTHON, RUST]:
            success(invoke(reader, "recover", index))
            success(invoke(reader, "remove", index))
    destination.write_bytes(b"small baseline")
    success(invoke(SWIFT, "write", index, "--destination", destination, "--acquisition", "takeover"))
    success(invoke(PYTHON, "remove", index))
    assert destination.read_bytes() == b"small baseline"


def readonly_tree(root):
    index, destination = root / "index", root / "tree"
    readonly = destination / "readonly"
    readonly.mkdir(parents=True)
    (readonly / "old").write_bytes(b"baseline")
    readonly.chmod(0o555)
    try:
        for _ in range(2):
            value = invoke(SWIFT, "write", index, "--destination", destination,
                           "--kind", "tree", "--acquisition", "takeover")
            assert value["status"] == "failed" and value["error"]["code"] == "unsupported", value
            state = ResourceAuthority(destination).state_root
            assert sorted(path.name for path in state.iterdir()) == ["authority.json", "lock"]
            assert sorted(path.name for path in index.iterdir()) == ["lock"]
            assert (readonly / "old").read_bytes() == b"baseline"
            assert readonly.stat().st_mode & 0o777 == 0o555
            for reader in [SWIFT, PYTHON, RUST]:
                success(invoke(reader, "recover", index))
                success(invoke(reader, "remove", index))
    finally:
        readonly.chmod(0o755)
    success(invoke(SWIFT, "write", index, "--destination", destination,
                   "--kind", "tree", "--acquisition", "takeover"))
    success(invoke(PYTHON, "remove", index))
    assert (readonly / "old").read_bytes() == b"baseline"
    assert readonly.stat().st_mode & 0o777 == 0o755


def foreign_anchors(root):
    from flyrail import (
        Acquisition, Bundle, BundleIdentity, DocumentFormat, Family, InstallationTarget,
        InstructionArtifact, Key, Member, RenderedArtifact, RenderedBundle, Scalar,
        Selector, StructuredContent, freeze_value, sync,
    )

    index, destination = root / "index", root / "document"
    destination.write_bytes(b'{"items":[{"id":"a"},{"id":"x","v":1},{"id":"b"}]}')
    bundle = Bundle.from_artifacts(BundleIdentity("team", "1"), [InstructionArtifact("guide", "text")])
    rendered = RenderedBundle([RenderedArtifact(
        "guide", Family.INSTRUCTIONS, destination, StructuredContent(
            DocumentFormat.JSON, Selector([Key("items"), Member(Scalar("x"), ("id",))]),
            freeze_value({"id": "x", "v": 2}),
        ),
    )])
    assert sync(bundle, rendered, InstallationTarget(index), acquisition=Acquisition.TAKEOVER).error is None
    destination.write_bytes(destination.read_bytes().replace(b'"a"', b'"c"').replace(b'"b"', b'"d"'))
    before = tree_snapshot(root)
    for reader in [PYTHON, SWIFT]:
        observation = invoke(reader, "inspect", index)
        assert observation["is_current"], observation
        assert observation["resources"][0]["claims"][0]["disposition"] == "current", observation
        for _ in range(2):
            value = invoke(reader, "remove", index)
            assert value["status"] == "failed" and value["error"]["code"] == "conflict", value
            assert "no surviving anchors" in value["error"]["message"], value
            assert tree_snapshot(root) == before
    records(index, destination)


def section_writer():
    use_python_paths("python/scripts")
    import interop_driver
    from flyrail import (
        Bundle, BundleIdentity, Family, InstructionArtifact,
        RenderedArtifact, RenderedBundle, SectionContent,
    )

    interop_driver.bundle = lambda bundle_id, version: Bundle.from_artifacts(
        BundleIdentity(bundle_id, version), [InstructionArtifact("guide", "text")],
    )
    interop_driver.rendered = lambda arguments: RenderedBundle([
        RenderedArtifact("guide", Family.INSTRUCTIONS, Path(arguments.destination),
                         SectionContent("other", "other content")),
    ])
    del sys.argv[1]
    if "--bundle-id" not in sys.argv:
        sys.argv.extend(["--bundle-id", "other"])
    return interop_driver.main()


def source_project(root, manifest):
    index, project, source = root / "index", root / "project", root / "source"
    project.mkdir()
    source.mkdir()
    write_manifest(source, manifest)
    return index, project, source


def write_manifest(source, manifest):
    (source / "flyrail.json").write_text(json.dumps(manifest))


def source_arguments(project, source, agent):
    node = shutil.which("node")
    assert node
    return ("--destination", project, "--source", source, "--agent", agent, "--node", node)


def first_install(index, writer, reader, arguments):
    success(invoke(writer, "write", index, *arguments))
    first = invoke(reader, "inspect", index)
    assert first["is_current"], first
    for resource in first["resources"]:
        records(index, resource["destination"])
    return first


def missing_receipt(root, index, reader, observation):
    missing = Path(observation["resources"][0]["state_root"]) / "receipt.json"
    original = missing.read_bytes()
    original_mode = missing.stat().st_mode & 0o777
    missing.unlink()
    before = tree_snapshot(root)
    for command in ["inspect", "recover", "remove"]:
        value = invoke(reader, command, index)
        if command == "inspect":
            assert not value["is_current"], value
            assert value["resources"][0]["error"]["code"] == "invalid_state", value
        else:
            assert value["status"] == "failed" and value["error"]["code"] == "invalid_state", value
        assert tree_snapshot(root) == before
    descriptor = os.open(missing, os.O_WRONLY | os.O_CREAT | os.O_EXCL, original_mode)
    with os.fdopen(descriptor, "wb") as restored:
        restored.write(original)
        os.fchmod(restored.fileno(), original_mode)
    assert missing.stat().st_mode & 0o777 == original_mode


def upgrade(index, writer, reader, arguments):
    success(invoke(reader, "write", index, *arguments))
    upgraded = invoke(writer, "inspect", index)
    assert upgraded["is_current"], upgraded
    return upgraded


def partial_upgrade(index, writer, arguments):
    value = invoke(writer, "write", index, *arguments, "--fail-index-completion")
    assert value["status"] == "partial", value
    return value


def source_free_removal(index, writer, reader):
    success(invoke(reader, "remove", index))
    assert invoke(writer, "inspect", index)["resources"] == []


def asset_source_graph(root, writer, reader):
    manifest = json.loads((ROOT / "spec/fixtures/translations.json").read_bytes())["asset_manifest"]
    manifest |= {"id": "team", "version": "1"}
    manifest["artifacts"].append({"kind": "instruction", "id": "guide", "text": "first"})
    index, project, source = source_project(root, manifest)
    (source / "runtime/data").mkdir(parents=True)
    (source / "runtime/server.js").write_bytes(b'console.log("MCP");')
    (project / "AGENTS.md").write_bytes(b"foreign\n")
    arguments = source_arguments(project, source, "codex")
    first = first_install(index, writer, reader, arguments)
    assert list(project.rglob("server.js")), first
    missing_receipt(root, index, reader, first)
    manifest["version"] = "2"
    manifest["artifacts"][-1]["text"] = "updated"
    (source / "runtime/server.js").write_bytes(b'console.log("updated");')
    write_manifest(source, manifest)
    upgrade(index, writer, reader, arguments)
    manifest["version"] = "3"
    write_manifest(source, manifest)
    partial_upgrade(index, writer, arguments)
    shutil.rmtree(source)
    success(invoke(reader, "recover", index))
    source_free_removal(index, writer, reader)
    assert (project / "AGENTS.md").read_bytes() == b"foreign\n"
    assert not list(project.rglob("server.js"))


def hook_source_graph(root, writer, reader):
    manifest = {
        "schema_version": 2, "id": "team", "version": "1",
        "artifacts": [{
            "kind": "hook", "id": "start", "protocol_version": 1,
            "event": "session-start", "command": {"argv": ["/bin/echo", "synthetic"]},
            "outcomes": ["continue"], "timeout_ms": 1000,
        }],
    }
    index, project, source = source_project(root, manifest)
    arguments = source_arguments(project, source, "claude")
    first = first_install(index, writer, reader, arguments)
    stable = stable_hook_contract(first)
    config = project / ".claude/settings.json"
    config_identity = config.stat().st_ino, config.read_bytes()

    def launcher(observation):
        assert stable_hook_contract(observation) == stable
        assert (config.stat().st_ino, config.read_bytes()) == config_identity
        return stable[0][0].read_bytes()

    first_launcher = launcher(first)
    assert list(project.rglob("runner.mts")), first
    missing_receipt(root, index, reader, first)
    manifest["version"] = "2"
    manifest["artifacts"][0]["command"]["argv"][-1] = "synthetic-updated"
    write_manifest(source, manifest)
    second_launcher = launcher(upgrade(index, writer, reader, arguments))
    assert second_launcher != first_launcher
    manifest["version"] = "3"
    manifest["artifacts"][0]["command"]["argv"][-1] = "synthetic-partial"
    write_manifest(source, manifest)
    partial_launcher = launcher(partial_upgrade(index, writer, arguments)["observation"])
    assert partial_launcher != second_launcher
    shutil.rmtree(source)
    success(invoke(reader, "recover", index))
    assert launcher(invoke(writer, "inspect", index)) == partial_launcher
    source_free_removal(index, writer, reader)
    assert not list(project.rglob("runner.mts"))


def stable_hook_contract(observation):
    receipts = [
        decode((Path(resource["state_root"]) / "receipt.json").read_bytes())
        for resource in observation["resources"]
    ]
    stable = tuple(pair for receipt in receipts for claim in receipt.claims
                   for pair in claim.stable_requirements)
    assert len(stable) == 1, stable
    launcher, claim_id = stable[0]
    routing = next(receipt for receipt in receipts if receipt.resource.destination == launcher)
    owned = next(claim for claim in routing.claims if claim.claim_id == claim_id)
    assert owned.requirements and owned.stable_requirements == ()
    assert launcher.is_file()
    return stable


def source_option_validation(root):
    source = root / "source"
    source.mkdir()
    (source / "flyrail.json").write_text(json.dumps({
        "schema_version": 2, "id": "team", "version": "1",
        "artifacts": [{"kind": "instruction", "id": "guide", "text": "safe"}],
    }))
    options = json.loads(SOURCE_OPTIONS.read_bytes())["cases"]
    assert options
    cases = [(option, value, f"{option} cannot be combined with --source") for option, value in options]
    cases.append(("--bundle-id", "other", "source bundle id must match --bundle-id"))
    for driver, (option, value, expected) in itertools.product([SWIFT, PYTHON, RUST], cases):
        before = tree_snapshot(root, modes=True)
        result = subprocess.run(
            driver + list(map(str, ["write", "--index", root / "index",
                                   "--destination", root / "project", "--source", source, option, value])),
            env=ENV, cwd=ROOT, capture_output=True, timeout=15,
        )
        assert result.returncode == 1, (driver, option, result)
        message = driver_error(driver, result)
        assert message == expected, (driver, message)
        assert tree_snapshot(root, modes=True) == before
        assert not (root / "index").exists() and not (root / "project").exists()
    return len(cases)


def main():
    base = ROOT / ".cache/swift-transaction-interop"
    base.mkdir(exist_ok=True)
    pairs = list(itertools.permutations([SWIFT, PYTHON, RUST], 2))
    operational = 0
    interruptions = 0
    with tempfile.TemporaryDirectory(dir=base) as temporary:
        root = Path(temporary)
        validation = root / "source-option-validation"
        validation.mkdir()
        option_cases = source_option_validation(validation)
        for number, ((writer, reader), scenario) in enumerate(
            itertools.product(pairs, [asset_source_graph, hook_source_graph])
        ):
            path = root / f"source-graph-{number}"
            path.mkdir()
            scenario(path, writer, reader)
        for (writer, reader), scenario in itertools.product(pairs, [takeover, adoption, retarget, index_failure]):
            path = root / f"operation-{operational}"
            path.mkdir()
            scenario(path, writer, reader)
            operational += 1
        for (writer, reader), phase in itertools.product(pairs, ["prepare", "receipt-committed"]):
            path = root / f"pair-kill-{interruptions}"
            path.mkdir()
            interruption(path, writer, reader, phase)
            interruptions += 1
        for reader, phase in itertools.product([PYTHON, RUST], [
            "index-pending-prepared", "index-pending-committed", "journal-prepared", "journal-committed",
            "backup", "publish", "receipt-prepared", "receipt-reread", "cleanup-node", "cleanup",
            "index-completion-prepared", "index-completion-committed",
        ]):
            path = root / f"phase-kill-{interruptions}"
            path.mkdir()
            interruption(path, SWIFT, reader, phase)
            interruptions += 1
        for phase in ["rollback-stage", "rollback-backup", "cleanup-node", "cleanup"]:
            path = root / f"recovery-kill-{phase}"
            path.mkdir()
            repeated_recovery(path, phase)
        for phase in ["stage-node", "index-pending-created", "journal-created", "receipt-created",
                      "index-completion-created"]:
            path = root / f"incomplete-{phase}"
            path.mkdir()
            incomplete_preparation(path, phase)
        for reader in [PYTHON, RUST]:
            path = root / f"shared-{len(list(root.iterdir()))}"
            path.mkdir()
            shared(path, SWIFT, reader)
        for reader, interrupted in itertools.product([SWIFT, PYTHON, RUST], [False, True]):
            path = root / f"same-generation-{len(list(root.iterdir()))}"
            path.mkdir()
            same_generation_repair(path, reader, interrupted)
        for scenario in [predictable_oversize, readonly_tree, foreign_anchors]:
            path = root / scenario.__name__
            path.mkdir()
            scenario(path)
    print(f"Transaction interop: {option_cases} rejected --source option combinations across 3 drivers, "
          f"{operational} operational pairs, {interruptions} SIGKILL phase pairs, "
          "4 interrupted recovery retries, 5 retained incomplete preparations across 3 readers, "
          "12 asset/reference/shared-document and stable-hook graph pairs with missing receipts, partials "
          "and source-free removal; 2 shared-document removals, 6 same-generation repair recoveries, "
          "oversize/read-only-tree/foreign-anchor regressions, canonical state records passed")


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "section-writer":
        raise SystemExit(section_writer())
    main()
