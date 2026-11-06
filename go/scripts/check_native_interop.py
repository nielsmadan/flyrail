"""Compare Go's native locking, admission, observation and security behaviour with Python, Rust and (on macOS) Swift."""

import ctypes
import itertools
import json
import os
import resource
import select
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.append(str(ROOT / "swift/scripts"))

from harness import (
    ENV,
    GO,
    SWIFT,
    Application,
    available_drivers,
    build_go_driver,
    case_base,
    driver_name,
    kill,
    use_python_paths,
    wait_for,
    write_store_record,
)

use_python_paths("python/src")

from check_transaction_interop import first_difference, rust_available, swift_available
from flyrail import ResourceAuthority, ResourceKind, ResourceRef
from flyrail._codec import decode, encode
from flyrail._lifecycle import error_detail
from flyrail._resource_io import ancestors, observe
from flyrail._security import security
from flyrail._state import resource_key

MACOS = sys.platform == "darwin"
ORDERINGS = ["tree-first", "child-first", "both-reserved"]
TREE_WRITER = [sys.executable, "-B", str(Path(__file__).resolve()), "tree-writer"]
XATTR_NAME = "flyrail.interop-security" if MACOS else "user.flyrail-interop-security"
DIAGNOSTIC_XATTR = "flyrail.diagnostic-test" if MACOS else "user.flyrail-diagnostic-test"


def set_xattr(target, name, payload):
    if MACOS:
        descriptor = os.open(target, os.O_RDONLY | os.O_NOFOLLOW)
        try:
            assert ctypes.CDLL(None, use_errno=True).fsetxattr(descriptor, name.encode(), payload, len(payload), 0, 0) == 0
        finally:
            os.close(descriptor)
    else:
        os.setxattr(target, name, payload, follow_symlinks=False)


def remove_xattr(target, name):
    if MACOS:
        descriptor = os.open(target, os.O_RDONLY | os.O_NOFOLLOW)
        try:
            assert ctypes.CDLL(None, use_errno=True).fremovexattr(descriptor, name.encode(), 0) == 0
        finally:
            os.close(descriptor)
    else:
        os.removexattr(target, name, follow_symlinks=False)


def line(process):
    assert process.stdout is not None
    ready, _, _ = select.select([process.stdout], [], [], 30)
    assert ready, "worker output timed out"
    data = process.stdout.readline()
    assert data, ("worker exited early", process.poll(), process.stderr.read())
    return json.loads(data)


class Worker:
    def __init__(self, app, destination, kind, barrier):
        self.barrier = barrier
        self.process = subprocess.Popen(
            app.command("hold", "--destination", destination, "--kind", kind, "--barrier", barrier,
                        "--wait-timeout", "30"),
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=ENV, cwd=ROOT,
        )
        try:
            assert line(self.process) == {"status": "reserved"}, driver_name(app.driver)
        except BaseException:
            self.process.kill()
            self.process.wait()
            self.process.stdout.close()
            self.process.stderr.close()
            raise

    def release(self):
        self.barrier.write_bytes(b"release\n")

    def outcome(self):
        result = line(self.process)
        assert self.process.wait(timeout=30) == 0, (self.process.args, result, self.process.stderr.read())
        return result["status"]

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
            self.process.wait(timeout=10)
        self.process.stdout.close()
        self.process.stderr.close()


def reference(destination, kind):
    return ResourceRef(ResourceAuthority(destination).destination, ResourceKind(kind))


def reserve(state, *references):
    for ref in references:
        write_store_record(state, resource_key(ref.destination, "authority"), ref)


def pairs(drivers):
    return [pair for pair in itertools.product(drivers, repeat=2) if GO in pair]


def pair_name(first, second):
    return f"{driver_name(first)}->{driver_name(second)}"


def race(root, first_driver, second_driver, ordering):
    state = root / "state"
    first_app, second_app = Application(first_driver, root, state), Application(second_driver, root, state)
    tree = root / "tree"
    tree.mkdir()
    child = tree / "document"
    workers = []
    try:
        if ordering == "child-first":
            first = Worker(first_app, child, "document", root / "first")
            workers.append(first)
            first.release()
            assert (status := first.outcome()) == "admitted", (ordering, "first", status)
            second = Worker(second_app, tree, "tree", root / "second")
        else:
            first = Worker(first_app, tree, "tree", root / "first")
            workers.append(first)
            if ordering == "tree-first":
                first.release()
                assert (status := first.outcome()) == "admitted", (ordering, "first", status)
            second = Worker(second_app, child, "document", root / "second")
        workers.append(second)
        if ordering == "both-reserved":
            reserve(state, reference(tree, "tree"), reference(child, "document"))
            first.release()
        second.release()
        if ordering == "both-reserved":
            assert (status := first.outcome()) == "conflict", (ordering, "first", status)
        assert (status := second.outcome()) == "conflict", (ordering, "second", status)
    finally:
        for worker in workers:
            worker.close()
    reader = Application(GO, root, state)
    records = reader.records()
    for ref in [reference(tree, "tree"), reference(child, "document")]:
        raw = records[resource_key(ref.destination, "authority")]
        assert decode(raw) == ref, raw
    return reader.normalized_records()


def alias_lock(root, holder_driver, contender_driver):
    state = root / "state"
    holder, contender = Application(holder_driver, root, state), Application(contender_driver, root, state)
    document = root / "Rules"
    document.write_bytes(b"rules\n")
    alias = root / "rules"
    if not alias.exists():
        return False
    worker = Worker(holder, document, "document", root / "release")
    try:
        result = subprocess.run(
            contender.command("hold", "--destination", alias, "--barrier", root / "already-released",
                              "--lock-timeout", "0"),
            capture_output=True, env=ENV, cwd=ROOT, timeout=30, check=False,
        )
        assert result.returncode == 1, result
        assert b"state store lock is busy" in result.stdout + result.stderr, result
        worker.release()
        assert worker.outcome() == "admitted"
    finally:
        worker.close()
    second = Worker(contender, alias, "document", root / "second-release")
    try:
        second.release()
        assert second.outcome() == "admitted"
    finally:
        second.close()
    return True


def lock_contention(root, holder_driver, writer_driver):
    state = root / "state"
    holder, writer = Application(holder_driver, root, state), Application(writer_driver, root, state)
    destination = root / "held"
    release = root / "release"
    child = subprocess.Popen(
        holder.command("hold", "--destination", destination, "--barrier", release, "--lock-timeout", "1"),
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=ENV, cwd=ROOT,
    )
    try:
        assert line(child) == {"status": "reserved"}
        result, value = writer.write("write", destination, "--lock-timeout", "0.01")
        assert result.returncode == 0, (result, value)
        assert value["status"] == "failed" and value["error"]["code"] == "busy", value
        release.write_text("release\n")
        assert line(child) == {"status": "admitted"}
        stdout, stderr = child.communicate(timeout=10)
        assert child.returncode == 0, (stdout, stderr)
        result, value = writer.write("write", destination)
        assert result.returncode == 0 and value["status"] == "applied" and value["error"] is None, value
    finally:
        if child.poll() is None:
            child.kill()
        child.communicate(timeout=10)


def per_pair(base, name, drivers, check):
    counts = {}
    for first, second in pairs(drivers):
        scenario = base / name
        scenario.mkdir()
        try:
            counts[pair_name(first, second)] = check(scenario, first, second)
        except AssertionError as error:
            raise AssertionError(f"{name} {pair_name(first, second)}: {error}") from error
        finally:
            shutil.rmtree(scenario)
    return counts


def observed(driver, destination, kind="document", **options):
    result = subprocess.run(
        driver + ["observe", "--destination", str(destination), "--kind", kind],
        capture_output=True, env=ENV, cwd=ROOT, timeout=300, check=False, **options,
    )
    assert result.returncode == 0, (driver_name(driver), result.stdout, result.stderr)
    return json.loads(result.stdout)


def observe_rows(drivers, destination, kind="document"):
    rows = {driver_name(driver): observed(driver, destination, kind) for driver in drivers}
    go = rows.pop("go")
    for name, row in rows.items():
        assert row == go, ("observe", destination, name, first_difference(go, row))
    return go


def observation_parity(root, observers):
    tree = root / "tree"
    (tree / "nested").mkdir(parents=True)
    (tree / "nested/raw").write_bytes(b"\x00\xff\n")
    (tree / "empty").write_bytes(b"")
    (tree / "nested/raw").chmod(0o640)
    row = observe_rows(observers, tree, "tree")
    assert row["error"] is None, row
    assert row["revision"] == json.loads(encode(observe(tree)))
    assert row["ancestors"] == [json.loads(encode(value)) for value in ancestors(tree)]
    assert [path.name for path in root.iterdir() if path.name.startswith(".flyrail-")] == []
    set_xattr(root, XATTR_NAME, b"ancestor\x00\xff")
    row = observe_rows(observers, tree, "tree")
    assert row["error"] is None, row
    native = bytes.fromhex(row["ancestors"][-1]["fields"]["security"]["bytes"])
    assert native == security(root, ancestor=True)
    assert XATTR_NAME.encode().hex().encode() in native, native
    remove_xattr(root, XATTR_NAME)
    (tree / "nested/raw").unlink()
    (tree / "nested/raw").symlink_to(root / "outside")
    row = observe_rows(observers, tree, "tree")
    assert row["error"] == {
        "code": "unsafe_path",
        "message": "managed symlinks, reparse points and special files are unsupported",
        "path": str(tree / "nested/raw"), "errno": None,
    }, row


def fixture_revision_parity(root, observers):
    fixture = json.loads((ROOT / "spec/fixtures/configurations.json").read_bytes())
    count = 0
    for item in fixture["journals"]:
        for key in ["before", "staged"]:
            destination = root / f"revision-{count}"
            record = item["record"]["fields"][key]
            nodes = record["fields"]["nodes"]
            for node in nodes:
                fields = node["fields"]
                path = destination / fields["path"] if fields["path"] else destination
                if fields["data"] is None:
                    path.mkdir()
                else:
                    path.write_bytes(bytes.fromhex(fields["data"]["bytes"]))
                path.chmod(fields["mode"])
            kind = "tree" if nodes and nodes[0]["fields"]["data"] is None else "document"
            row = observe_rows(observers, destination, kind)
            assert row["error"] is None, row
            assert row["revision"] == json.loads(encode(observe(destination)))
            count += 1
    return count


def tree_writer():
    use_python_paths("python/scripts")
    import interop_driver
    from flyrail import (
        Bundle,
        BundleEntry,
        BundleIdentity,
        Family,
        RenderedArtifact,
        RenderedBundle,
        SkillArtifact,
        TreeContent,
    )

    def content(arguments):
        return TreeContent([
            BundleEntry("empty"),
            BundleEntry("nested/file", interop_driver.payload(arguments),
                        arguments.mode == "executable"),
        ])

    def bundle(bundle_id, version):
        return Bundle.from_artifacts(
            BundleIdentity(bundle_id, version),
            [SkillArtifact("guide", "example", TreeContent([BundleEntry("SKILL.md", b"skill")]))],
        )

    def rendered(arguments):
        return RenderedBundle([
            RenderedArtifact("guide", Family.SKILLS, Path(arguments.destination), content(arguments)),
        ])

    interop_driver.bundle = bundle
    interop_driver.rendered = rendered
    del sys.argv[1]
    return interop_driver.main()


def revision_nodes(revision):
    return [(node.path, node.data, node.mode) for node in revision.nodes]


def interrupted_write(app, kind, extra, mode, phase):
    destination = app.root / "resource"
    arguments = ["--mode", mode, *extra]
    result, initial = app.write("write", destination, *arguments, "--payload", "before")
    assert result.returncode == 0 and initial["status"] == "applied", (result, initial)
    ready = app.root / "ready"
    process = subprocess.Popen(
        app.command("write", "--destination", destination, *arguments, "--version", "2", "--payload", "after",
                    "--barrier-phase", phase, "--barrier-ready", ready,
                    "--barrier-release", app.root / "release"),
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=ENV, cwd=ROOT,
    )
    try:
        wait_for(ready, process)
    finally:
        kill(process)
    records = app.records()
    key = resource_key(reference(destination, kind).destination, "journal")
    journal = decode(records[key])
    preparation = decode(records[key.removesuffix("journal") + "preparation"])
    container = "staging" if phase == "prepare" else "backup"
    payload = Path(preparation.directory) / container / "resource"
    revision = journal.staged if phase == "prepare" else journal.backup
    assert observe(payload) == revision, (kind, mode, phase, payload)
    files = [node for node in revision.nodes if node.data is not None]
    assert files and all(node.mode == (0o644 if mode == "readable" else 0o755) for node in files), (
        kind, mode, phase, files)
    row = observed(GO, destination, kind)
    assert row["error"] is None, row
    assert row["revision"] == json.loads(encode(observe(destination)))
    return {
        "records": app.normalized_records(),
        "staged": revision_nodes(journal.staged),
        "backup": revision_nodes(journal.backup),
    }


def transaction_parity(base, drivers):
    documents = [(driver_name(driver), driver, []) for driver in drivers]
    trees = [("go", GO, ["--kind", "tree"]), ("python", TREE_WRITER, [])]
    if SWIFT in drivers:
        trees.append(("swift", SWIFT, ["--kind", "tree"]))
    counts = {}
    for mode, phase in itertools.product(["readable", "executable"], ["prepare", "receipt-committed"]):
        for kind, writers in [("document", documents), ("tree", trees)]:
            outcomes = {}
            case = base / "case"
            for name, driver, extra in writers:
                case.mkdir()
                try:
                    outcome = interrupted_write(Application(driver, case, case / "state"), kind, extra, mode, phase)
                except AssertionError as error:
                    raise AssertionError(f"interrupted {kind} {mode} {phase}: {name}: {error}") from error
                finally:
                    shutil.rmtree(case)
                outcomes[name] = outcome if kind == "document" else {
                    key: outcome[key] for key in ["staged", "backup"]}
            expected = outcomes.pop("go")
            for name, outcome in outcomes.items():
                assert outcome == expected, (kind, mode, phase, "go", name, first_difference(expected, outcome))
                counts[name] = counts.get(name, 0) + 1
    return counts


def descriptor_budget_parity(root):
    tree = root / "tree"
    tree.mkdir()
    for index in range(300):
        (tree / f"d{index:04}").mkdir()

    def limit_descriptors():
        resource.setrlimit(resource.RLIMIT_NOFILE, (128, 128))

    arguments = GO + ["observe", "--destination", str(tree), "--kind", "tree", "--repeat", "140"]
    for fail in [False, True]:
        if fail:
            (tree / "d0299/link").symlink_to(root / "outside")
        result = subprocess.run(
            arguments, capture_output=True, env=ENV, cwd=ROOT, timeout=300, check=False,
            preexec_fn=limit_descriptors,
        )
        assert result.returncode == 0, (result.stdout, result.stderr)
        rows = [json.loads(line) for line in result.stdout.splitlines()]
        assert len(rows) == 140
        if fail:
            expected = {
                "code": "unsafe_path",
                "message": "managed symlinks, reparse points and special files are unsupported",
                "path": str(tree / "d0299/link"), "errno": None,
            }
            assert all(row["error"] == expected for row in rows), rows
        else:
            expected = json.loads(encode(observe(tree)))
            assert all(row["error"] is None and row["revision"] == expected for row in rows), rows


def reference_error(destination):
    try:
        security(destination)
    except OSError as exception:
        return error_detail(exception)
    raise AssertionError("reference accepted unsupported security metadata")


def security_diagnostic_parity(app):
    destination = app.root / "single'quote"
    result, value = app.write("write", destination)
    assert result.returncode == 0 and value["status"] == "applied", (result, value)
    descriptor = os.open(destination, os.O_RDONLY | os.O_NOFOLLOW) if MACOS else None
    library = ctypes.CDLL(None, use_errno=True) if MACOS else None
    try:
        for metadata in ["flags", "xattrs", "acl"] if MACOS else ["xattrs"]:
            if metadata == "flags":
                assert library.fchflags(descriptor, 0x8000) == 0
            elif metadata == "xattrs":
                set_xattr(destination, DIAGNOSTIC_XATTR, b"")
            else:
                subprocess.run(["chmod", "+a", "everyone allow read", str(destination)], check=True, timeout=30)
            error = reference_error(destination)
            expected = {"code": error.code.value, "message": error.message, "path": str(error.path)}
            if app.driver in (GO, SWIFT):
                row = observed(app.driver, destination)
                assert row["error"] == expected | {"errno": error.errno}, (metadata, row)
            result, row = app.target_command("inspect")
            assert result.returncode == 0, (result, row)
            assert row["resources"][0]["error"] == expected, (metadata, row)
            if metadata == "flags":
                assert library.fchflags(descriptor, 0) == 0
            elif metadata == "xattrs":
                remove_xattr(destination, DIAGNOSTIC_XATTR)
            else:
                subprocess.run(["chmod", "-N", str(destination)], check=True, timeout=30)
    finally:
        if MACOS:
            library.fchflags(descriptor, 0)
            os.close(descriptor)
    return 3 if MACOS else 1


def per_driver(base, name, drivers, check):
    counts = {}
    for driver in drivers:
        scenario = base / f"{name}-{driver_name(driver)}"
        scenario.mkdir()
        try:
            counts[driver_name(driver)] = check(Application(driver, scenario, scenario / "state"))
        except AssertionError as error:
            raise AssertionError(f"{name} {driver_name(driver)}: {error}") from error
        finally:
            shutil.rmtree(scenario)
    return counts


def main():
    build_go_driver()
    names = ["go", "python"] + (["rust"] if rust_available() else []) + (["swift"] if swift_available() else [])
    drivers = available_drivers(*names)
    assert [driver_name(driver) for driver in drivers] == names, ("missing interop driver", names)
    observers = [driver for driver in drivers if driver in (GO, SWIFT)]
    base = case_base("go-native-interop")
    counts = {name: 0 for name in names[1:]}

    def credit(pair, amount=1):
        for name in pair.split("->"):
            if name != "go":
                counts[name] += amount

    try:
        races = 0
        for ordering in ORDERINGS:
            outcomes = per_pair(base, f"race-{ordering}", drivers,
                                lambda root, first, second, ordering=ordering: race(root, first, second, ordering))
            expected = outcomes.pop("go->go")
            for pair, outcome in outcomes.items():
                assert outcome == expected, (ordering, pair, first_difference(expected, outcome))
                credit(pair)
            races += len(outcomes) + 1
        aliases = per_pair(base, "alias", drivers, alias_lock)
        for pair, checked in aliases.items():
            credit(pair, int(checked))
        contention = per_pair(base, "lock", drivers, lock_contention)
        for pair in contention:
            credit(pair)
        observation = base / "observation"
        observation.mkdir()
        observation_parity(observation, observers)
        revisions = base / "fixtures"
        revisions.mkdir()
        fixture_count = fixture_revision_parity(revisions, observers)
        transactions = base / "transactions"
        transactions.mkdir()
        for name, count in transaction_parity(transactions, drivers).items():
            counts[name] += count
        descriptors = base / "descriptors"
        descriptors.mkdir()
        descriptor_budget_parity(descriptors)
        diagnostics = per_driver(base, "diagnostics", drivers, security_diagnostic_parity)
    finally:
        shutil.rmtree(base)
    compared = ", ".join(f"Go/{name.capitalize()} {count}" for name, count in counts.items())
    alias_text = f"{sum(aliases.values())} alias lock" if any(aliases.values()) else "no alias lock (case-sensitive)"
    observing = "/".join(driver_name(driver).capitalize() for driver in observers)
    platform = f"security diagnostics from {len(diagnostics)} languages equal to Python's"
    print(f"Go native interop: {races} mixed-language admission races over {len(ORDERINGS)} orderings with equal "
          f"store records, {alias_text} and {len(contention)} store lock contention pairs, {observing} observation "
          f"equal to Python's ({fixture_count} fixture revisions, ancestors, unsafe paths), interrupted writes "
          f"equal, low-descriptor success/failure release; {platform}; per-language comparisons: {compared}")


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "tree-writer":
        raise SystemExit(tree_writer())
    main()
