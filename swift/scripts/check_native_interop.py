import ctypes
import itertools
import json
import os
import resource
import select
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from flyrail._codec import decode, encode
from flyrail._resource_io import ancestors, observe, validate_state
from flyrail._resource_models import ResourceRef
from flyrail import ResourceKind
from flyrail._security import security
from flyrail.authority import ResourceAuthority

from harness import ENV, PYTHON, ROOT, RUST, SWIFT, build_rust_examples, run, use_python_paths


def line(process):
    assert process.stdout is not None
    ready, _, _ = select.select([process.stdout], [], [], 30)
    assert ready, "worker output timed out"
    data = process.stdout.readline()
    assert data, ("worker exited early", process.poll(), process.stderr.read())
    return json.loads(data)


class Worker:
    def __init__(self, driver, destination, kind, barrier):
        self.barrier = barrier
        self.process = subprocess.Popen(
            driver
            + [
                "hold",
                "--destination",
                str(destination),
                "--kind",
                kind,
                "--barrier",
                str(barrier),
                "--wait-timeout",
                "30",
            ],
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


def race(root, first_driver, second_driver, ordering):
    tree = root / "tree"
    tree.mkdir()
    child = tree / "document"
    workers = []
    try:
        if ordering == "child-first":
            first = Worker(first_driver, child, "document", root / "first")
            workers.append(first)
            first.release()
            assert first.outcome() == "admitted"
            second = Worker(second_driver, tree, "tree", root / "second")
        else:
            first = Worker(first_driver, tree, "tree", root / "first")
            workers.append(first)
            if ordering == "tree-first":
                first.release()
                assert first.outcome() == "admitted"
            second = Worker(second_driver, child, "document", root / "second")
        workers.append(second)
        second.release()
        if ordering == "both-reserved":
            first.release()
            assert first.outcome() == "conflict"
        assert second.outcome() == "conflict"
        for destination in [tree, child]:
            state = ResourceAuthority(destination).state_root
            assert state.is_dir(), state
            header = state / "authority.json"
            assert header.is_file(), header
            assert header.stat().st_mode & 0o777 == 0o600
        for destination in [tree, child]:
            arguments = ["records", "--index", root / "absent-index", "--destination", destination]
            records = [json.loads(run(driver, *arguments)) for driver in [SWIFT, PYTHON, RUST]]
            assert records[0] == records[1] == records[2], records
    finally:
        for worker in workers:
            worker.close()


def alias_lock(root, first_driver, second_driver):
    document = root / "Rules"
    document.write_bytes(b"rules\n")
    alias = root / "rules"
    if not alias.exists():
        return
    worker = Worker(first_driver, document, "document", root / "release")
    try:
        result = subprocess.run(
            second_driver
            + [
                "hold",
                "--destination",
                str(alias),
                "--barrier",
                str(root / "already-released"),
                "--lock-timeout",
                "0",
            ],
            capture_output=True,
            env=ENV,
            cwd=ROOT,
            timeout=30,
        )
        assert result.returncode == 1
        assert b"target lock is busy" in result.stdout + result.stderr
        worker.release()
        assert worker.outcome() == "admitted"
    finally:
        worker.close()
    replay = alias if second_driver == SWIFT else ResourceAuthority(alias).destination
    second = Worker(second_driver, replay, "document", root / "second-release")
    try:
        second.release()
        assert second.outcome() == "admitted"
    finally:
        second.close()


def observation_parity(root):
    tree = root / "tree"
    (tree / "nested").mkdir(parents=True)
    (tree / "nested/raw").write_bytes(b"\x00\xff\n")
    (tree / "empty").write_bytes(b"")
    (tree / "nested/raw").chmod(0o640)
    row = json.loads(run(SWIFT, "observe", "--destination", tree, "--kind", "tree"))
    assert row["error"] is None, row
    assert row["revision"] == json.loads(encode(observe(tree)))
    assert row["ancestors"] == [json.loads(encode(value)) for value in ancestors(tree)]
    assert row["receipt"] is None
    assert row["recovery_paths"] == []
    assert not ResourceAuthority(tree).state_root.exists()
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        library = ctypes.CDLL(None, use_errno=True)
        payload = b"ancestor\x00\xff"
        assert library.fsetxattr(
            descriptor, b"flyrail.interop-security", payload, len(payload), 0, 0
        ) == 0
        row = json.loads(run(SWIFT, "observe", "--destination", tree, "--kind", "tree"))
        assert row["error"] is None
        native = bytes.fromhex(row["ancestors"][-1]["fields"]["security"]["bytes"])
        assert native == security(root, ancestor=True)
    finally:
        os.close(descriptor)
    (tree / "nested/raw").unlink()
    (tree / "nested/raw").symlink_to(root / "outside")
    row = json.loads(run(SWIFT, "observe", "--destination", tree, "--kind", "tree"))
    assert row["error"]["code"] == "unsafe_path"


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
            row = json.loads(run(SWIFT, "observe", "--destination", destination, "--kind", kind))
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


def transaction_parity(root):
    tree_driver = [sys.executable, "-B", str(Path(__file__).resolve()), "tree-writer"]
    writers = [(PYTHON, "document"), (RUST, "document"), (tree_driver, "tree")]
    count = 0
    for (driver, kind), mode, phase in itertools.product(
        writers, ["readable", "executable"], ["prepare", "receipt-committed"]
    ):
        scenario = root / str(count)
        scenario.mkdir()
        destination = scenario / "resource"
        index = scenario / "index"
        arguments = ["write", "--destination", destination, "--index", index, "--mode", mode]
        initial = json.loads(run(driver, *arguments, "--payload", "before"))
        assert initial["status"] == "applied", initial
        ready = scenario / "ready"
        process = subprocess.Popen(
            driver + list(map(str, arguments + [
                "--version", "2", "--payload", "after",
                "--barrier-phase", phase, "--barrier-ready", ready,
                "--barrier-release", scenario / "release",
            ])),
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=ENV, cwd=ROOT,
        )
        try:
            deadline = time.monotonic() + 30
            while not ready.exists():
                assert process.poll() is None, process.communicate()
                assert time.monotonic() < deadline, "transaction barrier timed out"
                time.sleep(0.01)
        finally:
            if process.poll() is None:
                process.terminate()
            process.communicate(timeout=10)
        state = ResourceAuthority(destination).state_root
        journal = decode((state / "transaction.json").read_bytes())
        container = "staging" if phase == "prepare" else "backup"
        payload = state / container / "resource"
        revision = journal.staged if phase == "prepare" else journal.backup
        assert observe(payload) == revision
        files = [node for node in revision.nodes if node.data is not None]
        assert files and all(node.mode == (0o644 if mode == "readable" else 0o755) for node in files)
        row = json.loads(run(SWIFT, "observe", "--destination", destination, "--kind", kind))
        assert row["error"] is None, row
        assert row["revision"] == json.loads(encode(observe(destination)))
        assert row["receipt"] == json.loads(encode(decode((state / "receipt.json").read_bytes())))
        reference = ResourceRef(destination, ResourceKind(kind), state)
        assert row["recovery_paths"] == list(map(str, validate_state(reference)))
        assert row["state_revision"] == json.loads(encode(observe(state)))
        records = [
            json.loads(run(reader, "records", "--index", index, "--destination", destination))
            for reader in [SWIFT, PYTHON, RUST]
        ]
        assert records[0] == records[1] == records[2], records
        count += 1
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


def security_diagnostic_parity(root):
    destination = root / "single'quote"
    index = root / "index"
    result = json.loads(run(PYTHON, "write", "--destination", destination, "--index", index))
    assert result["status"] == "applied", result
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
            swift = json.loads(run(SWIFT, "observe", "--destination", destination))["error"]
            from flyrail._lifecycle import error_detail
            try:
                security(destination)
            except OSError as exception:
                error = error_detail(exception)
            else:
                raise AssertionError("reference accepted unsupported security metadata")
            assert swift == dict(code=error.code.value, message=error.message,
                                 path=str(error.path), errno=error.errno), swift
            for driver in [PYTHON, RUST]:
                row = json.loads(run(driver, "inspect", "--index", index))
                observed = row["resources"][0]["error"]
                assert observed == {key: swift[key] for key in ["code", "message", "path"]}, row
            if metadata == "flags":
                assert library.fchflags(descriptor, 0) == 0
            elif metadata == "xattrs":
                assert library.fremovexattr(descriptor, b"flyrail.diagnostic-test", 0) == 0
            else:
                subprocess.run(["chmod", "-N", str(destination)], check=True, timeout=30)
    finally:
        library.fchflags(descriptor, 0)
        os.close(descriptor)


def main():
    build_rust_examples("interop")
    base = ROOT / ".cache/swift-native-interop"
    base.mkdir(exist_ok=True)
    pairs = [*itertools.permutations([SWIFT, PYTHON, RUST], 2), (SWIFT, SWIFT)]
    with tempfile.TemporaryDirectory(dir=base) as temporary:
        root = Path(temporary)
        for index, ((first, second), ordering) in enumerate(
            itertools.product(pairs, ["tree-first", "child-first", "both-reserved"])
        ):
            scenario = root / f"race-{index}"
            scenario.mkdir()
            race(scenario, first, second, ordering)
        for index, (first, second) in enumerate(pairs):
            scenario = root / f"alias-{index}"
            scenario.mkdir()
            alias_lock(scenario, first, second)
        observation = root / "observation"
        observation.mkdir()
        observation_parity(observation)
        revisions = root / "fixtures"
        revisions.mkdir()
        count = fixture_revision_parity(revisions)
        transactions = root / "transactions"
        transactions.mkdir()
        transaction_count = transaction_parity(transactions)
        descriptors = root / "descriptors"
        descriptors.mkdir()
        descriptor_budget_parity(descriptors)
        diagnostics = root / "diagnostics"
        diagnostics.mkdir()
        security_diagnostic_parity(diagnostics)
    print(f"Native interop: {len(pairs) * 3} admission orderings, {len(pairs)} alias lock pairs, observation/security parity, "
          f"{count} fixture revisions, {transaction_count} interrupted transactions, "
          "low-descriptor success/failure release, exact Python/Rust diagnostics passed")


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "tree-writer":
        raise SystemExit(tree_writer())
    main()
