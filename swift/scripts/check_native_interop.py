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

from flyrail import ResourceAuthority, ResourceKind, ResourceRef
from flyrail._codec import decode, encode
from flyrail._lifecycle import error_detail
from flyrail._resource_io import ancestors, observe
from flyrail._security import security
from flyrail._state import resource_key

from harness import (
    ENV, PYTHON, ROOT, RUST, SWIFT, Application, build_rust_examples, case_base, compare_languages, driver_name,
    kill, use_python_paths, wait_for, write_store_record,
)

ORDERINGS = ["tree-first", "child-first", "both-reserved"]
RACE_DRIVERS = (SWIFT, PYTHON, RUST)
TREE_WRITER = [sys.executable, "-B", str(Path(__file__).resolve()), "tree-writer"]
WRITERS = [
    ("python", PYTHON, "document", []),
    ("rust", RUST, "document", []),
    ("python-tree", TREE_WRITER, "tree", []),
    ("swift", SWIFT, "document", []),
    ("swift-tree", SWIFT, "tree", ["--kind", "tree"]),
]


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
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=ENV,
            cwd=ROOT,
        )
        assert line(self.process) == {"status": "reserved"}

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


def race(app, ordering):
    tree = app.root / "tree"
    tree.mkdir()
    child = tree / "document"
    workers = []
    try:
        if ordering == "child-first":
            first = Worker(app, child, "document", app.root / "first")
            workers.append(first)
            first.release()
            assert (status := first.outcome()) == "admitted", (ordering, "first", status)
            second = Worker(app, tree, "tree", app.root / "second")
        else:
            first = Worker(app, tree, "tree", app.root / "first")
            workers.append(first)
            if ordering == "tree-first":
                first.release()
                assert (status := first.outcome()) == "admitted", (ordering, "first", status)
            second = Worker(app, child, "document", app.root / "second")
        workers.append(second)
        if ordering == "both-reserved":
            reserve(app.state, reference(tree, "tree"), reference(child, "document"))
            first.release()
        second.release()
        if ordering == "both-reserved":
            assert (status := first.outcome()) == "conflict", (ordering, "first", status)
        assert (status := second.outcome()) == "conflict", (ordering, "second", status)
    finally:
        for worker in workers:
            worker.close()
    records = app.records()
    for ref in [reference(tree, "tree"), reference(child, "document")]:
        raw = records[resource_key(ref.destination, "authority")]
        assert decode(raw) == ref, raw
    return app.normalized_records()


def alias_lock(app):
    document = app.root / "Rules"
    document.write_bytes(b"rules\n")
    alias = app.root / "rules"
    if not alias.exists():
        return False
    worker = Worker(app, document, "document", app.root / "release")
    try:
        result = subprocess.run(
            app.command("hold", "--destination", alias, "--barrier", app.root / "already-released",
                        "--lock-timeout", "0"),
            capture_output=True, env=ENV, cwd=ROOT, timeout=30,
        )
        assert result.returncode == 1, result
        assert b"state store lock is busy" in result.stdout + result.stderr, result
        worker.release()
        assert worker.outcome() == "admitted"
    finally:
        worker.close()
    second = Worker(app, alias, "document", app.root / "second-release")
    try:
        second.release()
        assert second.outcome() == "admitted"
    finally:
        second.close()
    return True


def lock_contention(app):
    destination = app.root / "held"
    release = app.root / "release"
    child = subprocess.Popen(
        app.command("hold", "--destination", destination, "--barrier", release, "--lock-timeout", "1"),
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=ENV, cwd=ROOT,
    )
    try:
        assert line(child) == {"status": "reserved"}
        result, value = app.write("write", destination, "--lock-timeout", "0.01")
        assert result.returncode == 0, (result, value)
        assert value["status"] == "failed" and value["error"]["code"] == "busy", value
        release.write_text("release\n")
        assert line(child) == {"status": "admitted"}
        stdout, stderr = child.communicate(timeout=10)
        assert child.returncode == 0, (stdout, stderr)
        result, value = app.write("write", destination)
        assert result.returncode == 0 and value["status"] == "applied" and value["error"] is None, value
    finally:
        if child.poll() is None:
            child.kill()
        child.communicate(timeout=10)


def observation_parity(root):
    tree = root / "tree"
    (tree / "nested").mkdir(parents=True)
    (tree / "nested/raw").write_bytes(b"\x00\xff\n")
    (tree / "empty").write_bytes(b"")
    (tree / "nested/raw").chmod(0o640)
    row = json.loads(run_swift("observe", "--destination", tree, "--kind", "tree"))
    assert row["error"] is None, row
    assert row["revision"] == json.loads(encode(observe(tree)))
    assert row["ancestors"] == [json.loads(encode(value)) for value in ancestors(tree)]
    assert [path.name for path in root.iterdir() if path.name.startswith(".flyrail-")] == []
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        library = ctypes.CDLL(None, use_errno=True)
        payload = b"ancestor\x00\xff"
        assert library.fsetxattr(
            descriptor, b"flyrail.interop-security", payload, len(payload), 0, 0
        ) == 0
        row = json.loads(run_swift("observe", "--destination", tree, "--kind", "tree"))
        assert row["error"] is None
        native = bytes.fromhex(row["ancestors"][-1]["fields"]["security"]["bytes"])
        assert native == security(root, ancestor=True)
    finally:
        os.close(descriptor)
    (tree / "nested/raw").unlink()
    (tree / "nested/raw").symlink_to(root / "outside")
    row = json.loads(run_swift("observe", "--destination", tree, "--kind", "tree"))
    assert row["error"]["code"] == "unsafe_path"


def run_swift(*arguments):
    result = subprocess.run(SWIFT + list(map(str, arguments)), capture_output=True, env=ENV, cwd=ROOT, timeout=300)
    assert result.returncode == 0, (result.stdout, result.stderr)
    return result.stdout


def fixture_revision_parity(root):
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
            row = json.loads(run_swift("observe", "--destination", destination, "--kind", kind))
            assert row["error"] is None, row
            assert row["revision"] == json.loads(encode(observe(destination)))
            count += 1
    return count


def tree_writer():
    use_python_paths("python/scripts")
    import interop_driver
    from flyrail import (
        Bundle, BundleEntry, BundleIdentity, Family, RenderedArtifact,
        RenderedBundle, SkillArtifact, TreeContent,
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
    assert files and all(node.mode == (0o644 if mode == "readable" else 0o755) for node in files), (kind, mode, phase, files)
    row = json.loads(run_swift("observe", "--destination", destination, "--kind", kind))
    assert row["error"] is None, row
    assert row["revision"] == json.loads(encode(observe(destination)))
    return {
        "records": app.normalized_records(),
        "staged": revision_nodes(journal.staged),
        "backup": revision_nodes(journal.backup),
    }


def transaction_parity(base):
    count = 0
    for mode, phase in itertools.product(["readable", "executable"], ["prepare", "receipt-committed"]):
        outcomes = {}
        case = base / "case"
        for name, driver, kind, extra in WRITERS:
            case.mkdir()
            try:
                outcomes[name] = interrupted_write(Application(driver, case, case / "state"), kind, extra, mode, phase)
            finally:
                shutil.rmtree(case)
            count += 1
        documents = [outcomes[name]["records"] for name in ["python", "rust", "swift"]]
        assert documents[0] == documents[1] == documents[2], (mode, phase, documents)
        trees = [{key: outcomes[name][key] for key in ["staged", "backup"]} for name in ["python-tree", "swift-tree"]]
        assert trees[0] == trees[1], (mode, phase, trees)
    return count


def descriptor_budget_parity(root):
    tree = root / "tree"
    tree.mkdir()
    for index in range(300):
        (tree / f"d{index:04}").mkdir()

    def limit_descriptors():
        resource.setrlimit(resource.RLIMIT_NOFILE, (128, 128))

    arguments = SWIFT + [
        "observe", "--destination", str(tree), "--kind", "tree", "--repeat", "140",
    ]
    for fail in [False, True]:
        if fail:
            (tree / "d0299/link").symlink_to(root / "outside")
        result = subprocess.run(
            arguments, capture_output=True, env=ENV, cwd=ROOT, timeout=300,
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
    descriptor = os.open(destination, os.O_RDONLY | os.O_NOFOLLOW)
    library = ctypes.CDLL(None, use_errno=True)
    try:
        for metadata in ["flags", "xattrs", "acl"]:
            if metadata == "flags":
                assert library.fchflags(descriptor, 0x8000) == 0
            elif metadata == "xattrs":
                assert library.fsetxattr(descriptor, b"flyrail.diagnostic-test", b"", 0, 0, 0) == 0
            else:
                subprocess.run(
                    ["chmod", "+a", "everyone allow read", str(destination)],
                    check=True, timeout=30,
                )
            error = reference_error(destination)
            expected = dict(code=error.code.value, message=error.message, path=str(error.path))
            if app.driver == SWIFT:
                swift = json.loads(run_swift("observe", "--destination", destination))["error"]
                assert swift == expected | {"errno": error.errno}, swift
            result, row = app.target_command("inspect")
            assert result.returncode == 0, (result, row)
            assert row["resources"][0]["error"] == expected, (driver_name(app.driver), metadata, row)
            if metadata == "flags":
                assert library.fchflags(descriptor, 0) == 0
            elif metadata == "xattrs":
                assert library.fremovexattr(descriptor, b"flyrail.diagnostic-test", 0) == 0
            else:
                subprocess.run(["chmod", "-N", str(destination)], check=True, timeout=30)
    finally:
        library.fchflags(descriptor, 0)
        os.close(descriptor)


def per_driver(root, name, check):
    for driver in RACE_DRIVERS:
        scenario = root / f"{name}-{driver_name(driver)}"
        scenario.mkdir()
        yield check(Application(driver, scenario, scenario / "state"))


def main():
    build_rust_examples("interop")
    base = case_base("swift-native-interop")
    try:
        for ordering in ORDERINGS:
            races = base / f"race-{ordering}"
            races.mkdir()
            compare_languages(races, lambda app, ordering=ordering: race(app, ordering), drivers=RACE_DRIVERS)
        aliases = sum(per_driver(base, "alias", alias_lock))
        list(per_driver(base, "lock", lock_contention))
        observation = base / "observation"
        observation.mkdir()
        observation_parity(observation)
        revisions = base / "fixtures"
        revisions.mkdir()
        count = fixture_revision_parity(revisions)
        transactions = base / "transactions"
        transactions.mkdir()
        transaction_count = transaction_parity(transactions)
        descriptors = base / "descriptors"
        descriptors.mkdir()
        descriptor_budget_parity(descriptors)
        list(per_driver(base, "diagnostics", security_diagnostic_parity))
    finally:
        shutil.rmtree(base)
    print(f"Native interop: {len(ORDERINGS)} admission orderings with equal store records across 3 languages, "
          f"{aliases} alias lock and 3 store lock contention checks, observation/security parity, "
          f"{count} fixture revisions, {transaction_count} interrupted transactions, "
          "low-descriptor success/failure release, exact Python diagnostics from 3 languages passed")


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "tree-writer":
        raise SystemExit(tree_writer())
    main()
