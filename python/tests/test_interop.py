import json
import os
import select
import shutil
import subprocess
import sys
import time
from functools import cache
from pathlib import Path
from typing import Any, cast

import pytest
from interop_vectors import REJECTED, fixture_records, prepared

from flyrail._codec import decode
from flyrail._resource_models import Receipt

pytestmark = pytest.mark.integration

ROOT = Path(__file__).resolve().parents[2]
DRIVER = ROOT / "python" / "scripts" / "interop_driver.py"
RUST = ROOT / "rust"
RUST_BINARY = RUST / "target" / "debug" / "examples" / "interop"
CARGO_HOME = ROOT / ".cache" / "cargo-home"
FIXTURE_RECORDS = fixture_records()
MAX_PAYLOAD_SIZE = 1024 * 1024


@cache
def rust_example() -> Path:
    cargo = shutil.which("cargo")
    if cargo is None:
        if os.environ.get("CI"):
            pytest.fail("cargo is required for interop in CI")
        pytest.skip("interop skipped: cargo is unavailable")
    environment = {**os.environ, "CARGO_HOME": str(CARGO_HOME)}
    result = subprocess.run(  # noqa: S603
        [
            cargo,
            "build",
            "--example",
            "interop",
            "--features",
            "interop-test-seams",
        ],
        cwd=RUST,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    return RUST_BINARY


def run_driver(payload: bytes) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(  # noqa: S603
        [sys.executable, str(DRIVER), "encode"],
        input=payload,
        capture_output=True,
        check=False,
    )


def run_rust(binary: Path, type_name: str, payload: bytes) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(  # noqa: S603
        [str(binary), "encode", "--type", type_name],
        input=payload,
        capture_output=True,
        check=False,
    )


def invoke(
    implementation: str, binary: Path, command: str, *arguments: str
) -> tuple[subprocess.CompletedProcess[bytes], dict[str, Any]]:
    executable = [sys.executable, str(DRIVER)] if implementation == "python" else [str(binary)]
    result = subprocess.run(  # noqa: S603
        [*executable, command, *arguments],
        capture_output=True,
        check=False,
    )
    value = cast(dict[str, Any], json.loads(result.stdout))
    return result, value


def assert_success(result: subprocess.CompletedProcess[bytes], value: dict[str, Any]) -> None:
    assert result.returncode == 0, (result.stderr, value)
    assert value["status"] in {"applied", "unchanged"}, value
    assert value["error"] is None


def assert_canonical_records(binary: Path, index: Path, destination: Path) -> None:
    envelopes = []
    for implementation in ("python", "rust"):
        result, value = invoke(
            implementation,
            binary,
            "records",
            "--index",
            str(index),
            "--destination",
            str(destination),
        )
        assert result.returncode == 0, (result.stderr, value)
        records = {
            (Path(item["path"]).name, item["type"]): item["bytes"] for item in value["records"]
        }
        assert records
        assert all(item["bytes"] == item["canonical"] for item in value["records"])
        envelopes.append(records)
    assert envelopes[0] == envelopes[1]


def executable(implementation: str, binary: Path) -> list[str]:
    return [sys.executable, str(DRIVER)] if implementation == "python" else [str(binary)]


def read_child_value(child: subprocess.Popen[bytes]) -> dict[str, Any]:
    assert child.stdout is not None
    readable, _, _ = select.select([child.stdout], [], [], 10)
    assert readable, "child did not acknowledge before timeout"
    return cast(dict[str, Any], json.loads(child.stdout.readline()))


def wait_for_path(path: Path, child: subprocess.Popen[bytes]) -> None:
    deadline = time.monotonic() + 10
    while not path.exists() and child.poll() is None and time.monotonic() < deadline:
        time.sleep(0.005)
    assert path.exists() and child.poll() is None


@pytest.mark.parametrize(
    "name,raw,type_name",
    FIXTURE_RECORDS,
    ids=[name for name, _, _ in FIXTURE_RECORDS],
)
def test_fixture_records_are_byte_identical(
    tmp_path: Path, name: str, raw: bytes, type_name: str
) -> None:
    binary = rust_example()
    vector = prepared(name, raw, tmp_path)
    python_result = run_driver(vector)
    rust_result = run_rust(binary, type_name, vector)
    assert python_result.returncode == 0, python_result.stderr
    assert rust_result.returncode == 0, rust_result.stderr
    assert python_result.stdout == vector
    assert rust_result.stdout == vector
    assert run_driver(rust_result.stdout).stdout == vector
    assert run_rust(binary, type_name, python_result.stdout).stdout == vector


def test_both_drivers_reject_an_unknown_state_record() -> None:
    binary = rust_example()
    python_result = run_driver(REJECTED)
    assert python_result.returncode == 1
    assert python_result.stdout == b""
    assert b"unknown state record" in python_result.stderr
    rust_result = run_rust(binary, "Receipt", REJECTED)
    assert rust_result.returncode == 1
    assert rust_result.stdout == b""
    assert b"decode Receipt: unknown state record" in rust_result.stderr


def test_rust_driver_rejects_a_record_of_another_type(tmp_path: Path) -> None:
    binary = rust_example()
    receipts = [item for item in FIXTURE_RECORDS if item[2] == "Receipt"]
    name, raw, _ = receipts[0]
    vector = prepared(name, raw, tmp_path)
    result = run_rust(binary, "Journal", vector)
    assert result.returncode == 1
    assert result.stdout == b""
    assert b"decode Journal:" in result.stderr


@pytest.mark.parametrize("writer", ["python", "rust"])
def test_cross_language_takeover_update_remove_and_record_bytes(
    tmp_path: Path, writer: str
) -> None:
    binary = rust_example()
    reader = "rust" if writer == "python" else "python"
    index = tmp_path / "index"
    destination = tmp_path / "AGENTS.md"
    original = b"original\n"
    destination.write_bytes(original)
    destination.chmod(0o600)
    written, value = invoke(
        writer,
        binary,
        "write",
        "--index",
        str(index),
        "--destination",
        str(destination),
        "--payload",
        "first\n",
        "--acquisition",
        "takeover",
    )
    assert_success(written, value)
    assert destination.read_bytes() == b"first\n"
    assert_canonical_records(binary, index, destination)

    inspected, observation = invoke(reader, binary, "inspect", "--index", str(index))
    assert inspected.returncode == 0, inspected.stderr
    assert observation["is_current"] is True
    assert observation["version"] == "1"
    updated, value = invoke(
        reader,
        binary,
        "update",
        "--index",
        str(index),
        "--destination",
        str(destination),
        "--version",
        "2",
        "--payload",
        "second\n",
    )
    assert_success(updated, value)
    assert destination.read_bytes() == b"second\n"
    assert_canonical_records(binary, index, destination)
    removed, value = invoke(reader, binary, "remove", "--index", str(index))
    assert_success(removed, value)
    assert destination.read_bytes() == original
    assert destination.stat().st_mode & 0o777 == 0o600
    assert_canonical_records(binary, index, destination)


@pytest.mark.parametrize("writer", ["python", "rust"])
def test_cross_language_equal_byte_adoption_preserves_identity_then_removes(
    tmp_path: Path, writer: str
) -> None:
    binary = rust_example()
    reader = "rust" if writer == "python" else "python"
    index = tmp_path / "index"
    destination = tmp_path / "adopted"
    destination.write_bytes(b"same bytes")
    identity = (destination.stat().st_dev, destination.stat().st_ino)
    written, value = invoke(
        writer,
        binary,
        "write",
        "--index",
        str(index),
        "--destination",
        str(destination),
        "--payload",
        "same bytes",
        "--acquisition",
        "adopt",
    )
    assert_success(written, value)
    assert (destination.stat().st_dev, destination.stat().st_ino) == identity
    receipt_path = next(tmp_path.glob(".*.state/receipt.json"))
    receipt = decode(receipt_path.read_bytes())
    assert isinstance(receipt, Receipt)
    assert receipt.claims[0].disposition == "adopted"
    assert_canonical_records(binary, index, destination)
    removed, value = invoke(reader, binary, "remove", "--index", str(index))
    assert_success(removed, value)
    assert destination.exists() is False


@pytest.mark.parametrize("writer", ["python", "rust"])
def test_cross_language_retarget_restores_each_baseline(tmp_path: Path, writer: str) -> None:
    binary = rust_example()
    reader = "rust" if writer == "python" else "python"
    index = tmp_path / "index"
    first = tmp_path / "first"
    second = tmp_path / "second"
    first.write_bytes(b"first baseline")
    second.write_bytes(b"second baseline")
    first.chmod(0o600)
    second.chmod(0o640)
    for destination, version, data in (
        (first, "1", "first generated"),
        (second, "2", "second generated"),
    ):
        result, value = invoke(
            writer,
            binary,
            "update",
            "--index",
            str(index),
            "--destination",
            str(destination),
            "--version",
            version,
            "--payload",
            data,
            "--acquisition",
            "takeover",
        )
        assert_success(result, value)
    assert first.read_bytes() == b"first baseline"
    assert first.stat().st_mode & 0o777 == 0o600
    assert second.read_bytes() == b"second generated"
    inspected, observation = invoke(reader, binary, "inspect", "--index", str(index))
    assert inspected.returncode == 0 and observation["version"] == "2"
    removed, value = invoke(reader, binary, "remove", "--index", str(index))
    assert_success(removed, value)
    assert second.read_bytes() == b"second baseline"
    assert second.stat().st_mode & 0o777 == 0o640


@pytest.mark.parametrize("phase", ["prepare", "receipt-committed"])
@pytest.mark.parametrize("writer", ["python", "rust"])
def test_cross_language_interrupted_writer_recovers_source_free(
    tmp_path: Path, writer: str, phase: str
) -> None:
    binary = rust_example()
    reader = "rust" if writer == "python" else "python"
    index = tmp_path / "index"
    destination = tmp_path / "large"
    destination.write_bytes(b"baseline")
    initial, value = invoke(
        reader,
        binary,
        "write",
        "--index",
        str(index),
        "--destination",
        str(destination),
        "--payload",
        "installed",
        "--acquisition",
        "takeover",
    )
    assert_success(initial, value)
    ready = tmp_path / "ready"
    release = tmp_path / "release"
    child = subprocess.Popen(  # noqa: S603
        [
            *executable(writer, binary),
            "update",
            "--index",
            str(index),
            "--destination",
            str(destination),
            "--version",
            "2",
            "--payload",
            "updated",
            "--barrier-phase",
            phase,
            "--barrier-ready",
            str(ready),
            "--barrier-release",
            str(release),
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        wait_for_path(ready, child)
        journal = next(tmp_path.glob(".*.state/transaction.json"))
        assert journal.is_file()
        competing, busy = invoke(
            reader,
            binary,
            "update",
            "--index",
            str(index),
            "--destination",
            str(destination),
            "--version",
            "3",
            "--payload",
            "competing",
            "--lock-timeout",
            "0.01",
        )
        assert competing.returncode == 0
        assert busy["status"] == "failed"
        assert busy["error"]["code"] == "busy"
        child.kill()
        child.communicate(timeout=10)
        assert destination.read_bytes() == (b"installed" if phase == "prepare" else b"updated")
        inspected, observation = invoke(reader, binary, "inspect", "--index", str(index))
        assert inspected.returncode == 0
        assert observation["pending"] is True
        assert observation["resources"][0]["recovery_paths"]
        recovered, value = invoke(reader, binary, "recover", "--index", str(index))
        assert_success(recovered, value)
        inspected, observation = invoke(reader, binary, "inspect", "--index", str(index))
        assert inspected.returncode == 0
        assert observation["pending"] is True
        assert observation["version"] == "1"
        assert observation["resources"][0]["recovery_paths"] == []
        assert destination.read_bytes() == (b"installed" if phase == "prepare" else b"updated")
        removed, value = invoke(reader, binary, "remove", "--index", str(index))
        assert_success(removed, value)
        assert destination.read_bytes() == b"baseline"
    finally:
        if child.poll() is None:
            child.kill()
        child.communicate(timeout=10)


@pytest.mark.parametrize("writer", ["python", "rust"])
def test_cross_language_index_failure_is_not_reported_clean(tmp_path: Path, writer: str) -> None:
    binary = rust_example()
    reader = "rust" if writer == "python" else "python"
    destination = tmp_path / "resource"
    destination.write_bytes(b"foreign")
    index = tmp_path / "index"
    initial, value = invoke(
        writer,
        binary,
        "write",
        "--index",
        str(index),
        "--destination",
        str(destination),
        "--payload",
        "first",
        "--acquisition",
        "takeover",
    )
    assert_success(initial, value)
    result, value = invoke(
        writer,
        binary,
        "update",
        "--index",
        str(index),
        "--destination",
        str(destination),
        "--version",
        "2",
        "--payload",
        "second",
        "--fail-index-completion",
    )
    assert result.returncode == 0
    assert value["status"] == "partial"
    assert value["error"]["code"] == "io_error"
    inspected, observation = invoke(reader, binary, "inspect", "--index", str(index))
    assert inspected.returncode == 0
    assert observation["is_current"] is False
    assert observation["pending"] is True
    assert observation["version"] == "1"
    assert destination.read_bytes() == b"second"
    recovered, value = invoke(reader, binary, "recover", "--index", str(index))
    assert_success(recovered, value)
    inspected, observation = invoke(reader, binary, "inspect", "--index", str(index))
    assert inspected.returncode == 0
    assert observation["is_current"] is False
    assert observation["pending"] is True
    assert observation["version"] == "1"
    assert observation["resources"][0]["recovery_paths"] == []
    assert destination.read_bytes() == b"second"
    removed, value = invoke(reader, binary, "remove", "--index", str(index))
    assert_success(removed, value)
    assert destination.read_bytes() == b"foreign"


@pytest.mark.parametrize("attribute", ["acl", "xattr"])
@pytest.mark.parametrize("writer", ["python", "rust"])
@pytest.mark.skipif(sys.platform != "darwin", reason="native macOS ACL and xattrs")
def test_both_writers_refuse_foreign_macos_security_before_publication(
    tmp_path: Path, writer: str, attribute: str
) -> None:
    binary = rust_example()
    destination = tmp_path / f"{writer}-{attribute}"
    destination.write_bytes(b"foreign")
    destination.chmod(0o640)
    cleanup: list[str]
    if attribute == "acl":
        command = ["chmod", "+a", "everyone allow read", str(destination)]
        cleanup = ["chmod", "-N", str(destination)]
    else:
        command = ["xattr", "-w", "flyrail.interop", "value", str(destination)]
        cleanup = ["xattr", "-d", "flyrail.interop", str(destination)]
    subprocess.run(command, check=True, capture_output=True)  # noqa: S603
    try:
        result, value = invoke(
            writer,
            binary,
            "write",
            "--index",
            str(tmp_path / f"{writer}-{attribute}-index"),
            "--destination",
            str(destination),
            "--payload",
            "generated",
            "--acquisition",
            "takeover",
        )
        assert result.returncode == 0
        assert value["status"] == "failed"
        assert value["error"]["code"] == "unsupported"
        assert destination.read_bytes() == b"foreign"
        assert destination.stat().st_mode & 0o777 == 0o640
    finally:
        subprocess.run(cleanup, check=True, capture_output=True)  # noqa: S603


@pytest.mark.skipif(sys.platform != "linux", reason="native Linux xattrs")
@pytest.mark.parametrize("writer", ["python", "rust"])
def test_both_writers_refuse_foreign_linux_xattrs(tmp_path: Path, writer: str) -> None:
    binary = rust_example()
    destination = tmp_path / writer
    destination.write_bytes(b"foreign")
    destination.chmod(0o640)
    try:
        os.setxattr(destination, "user.flyrail.interop", b"value")  # type: ignore[attr-defined]
    except OSError as error:
        pytest.skip(f"filesystem does not support user xattrs: {error}")
    result, value = invoke(
        writer,
        binary,
        "write",
        "--index",
        str(tmp_path / f"{writer}-index"),
        "--destination",
        str(destination),
        "--payload",
        "generated",
        "--acquisition",
        "takeover",
    )
    assert result.returncode == 0
    assert value["status"] == "failed"
    assert value["error"]["code"] == "unsupported"
    assert os.getxattr(destination, "user.flyrail.interop") == b"value"  # type: ignore[attr-defined]
    assert destination.read_bytes() == b"foreign"
    assert destination.stat().st_mode & 0o777 == 0o640


@pytest.mark.parametrize(
    "order,tree_worker",
    [
        ("tree-first", "python"),
        ("tree-first", "rust"),
        ("child-first", "python"),
        ("child-first", "rust"),
        ("simultaneous", "python"),
        ("simultaneous", "rust"),
    ],
)
def test_mixed_language_admission_orderings(tmp_path: Path, order: str, tree_worker: str) -> None:
    binary = rust_example()
    tree = tmp_path / "tree"
    tree.mkdir()
    child_worker = "rust" if tree_worker == "python" else "python"
    workers: dict[str, tuple[subprocess.Popen[bytes], Path]] = {}
    first = "tree" if order != "child-first" else "child"
    second = "child" if first == "tree" else "tree"

    def spawn(label: str) -> None:
        if label in workers:
            return
        implementation = tree_worker if label == "tree" else child_worker
        destination = tree if label == "tree" else tree / "child"
        kind = label if label == "tree" else "document"
        release = tmp_path / f"release-{label}"
        workers[label] = (
            subprocess.Popen(  # noqa: S603
                [
                    *executable(implementation, binary),
                    "hold",
                    "--destination",
                    str(destination),
                    "--kind",
                    kind,
                    "--barrier",
                    str(release),
                    "--lock-timeout",
                    "1",
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            ),
            release,
        )

    try:
        if order == "simultaneous":
            spawn("tree")
            spawn("child")
            assert read_child_value(workers["tree"][0]) == {"status": "reserved"}
            assert read_child_value(workers["child"][0]) == {"status": "reserved"}
            workers["tree"][1].write_text("release\n")
            workers["child"][1].write_text("release\n")
            expected = {"tree": "conflict", "child": "conflict"}
        else:
            spawn(first)
            assert read_child_value(workers[first][0]) == {"status": "reserved"}
            workers[first][1].write_text("release\n")
            assert read_child_value(workers[first][0]) == {"status": "admitted"}
            spawn(second)
            assert read_child_value(workers[second][0]) == {"status": "reserved"}
            workers[second][1].write_text("release\n")
            expected = {first: "admitted", second: "conflict"}
        for label, (process, _) in workers.items():
            if order != "simultaneous" and label == first:
                continue
            assert read_child_value(process) == {"status": expected[label]}
        for process, _ in workers.values():
            stdout, stderr = process.communicate(timeout=10)
            assert process.returncode == 0, (stdout, stderr)
    finally:
        for process, _ in workers.values():
            if process.poll() is None:
                process.kill()
            process.communicate(timeout=10)


@pytest.mark.parametrize("implementation", ["python", "rust"])
@pytest.mark.parametrize(
    "arguments",
    [
        ("remove", "--index"),
        ("remove", "--index", "--replace-modified"),
        ("remove", "--unknown", "value"),
        ("remove", "--index", "one", "--index", "two"),
        ("remove", "--index", "index", "--lock-timeout", "-1"),
        ("remove", "--index", "index", "--lock-timeout", "nan"),
        ("remove", "--index", "index", "--lock-timeout", "inf"),
        (
            "write",
            "--index",
            "index",
            "--destination",
            "destination",
            "--large",
            "-1",
        ),
        (
            "write",
            "--index",
            "index",
            "--destination",
            "destination",
            "--large",
            str(1 << 64),
        ),
    ],
)
def test_both_drivers_reject_invalid_arguments(
    tmp_path: Path, implementation: str, arguments: tuple[str, ...]
) -> None:
    binary = rust_example()
    resolved = tuple(
        str(tmp_path / item) if item in {"index", "destination"} else item for item in arguments
    )
    result = subprocess.run(  # noqa: S603
        [*executable(implementation, binary), *resolved],
        capture_output=True,
        check=False,
    )
    assert result.returncode != 0
    assert result.returncode != 101


@pytest.mark.parametrize("implementation", ["python", "rust"])
def test_both_drivers_bound_large_payloads_without_tracebacks(
    tmp_path: Path, implementation: str
) -> None:
    binary = rust_example()
    result = subprocess.run(  # noqa: S603
        [
            *executable(implementation, binary),
            "write",
            "--index",
            str(tmp_path / "index"),
            "--destination",
            str(tmp_path / "destination"),
            "--large",
            str(MAX_PAYLOAD_SIZE + 1),
        ],
        capture_output=True,
        check=False,
    )
    assert result.returncode == 1
    assert b"Traceback" not in result.stderr
    value = cast(dict[str, Any], json.loads(result.stdout))
    assert value["error"] == {
        "code": None,
        "message": f"large payload size must not exceed {MAX_PAYLOAD_SIZE}",
        "path": None,
    }


@pytest.mark.parametrize("writer", ["python", "rust"])
def test_both_drivers_accept_the_maximum_large_payload(tmp_path: Path, writer: str) -> None:
    binary = rust_example()
    reader = "rust" if writer == "python" else "python"
    index = tmp_path / "index"
    destination = tmp_path / "destination"
    result, value = invoke(
        writer,
        binary,
        "write",
        "--index",
        str(index),
        "--destination",
        str(destination),
        "--large",
        str(MAX_PAYLOAD_SIZE),
    )
    assert_success(result, value)
    data = destination.read_bytes()
    assert len(data) == MAX_PAYLOAD_SIZE
    assert data[:24] == b"flyrail-interop-payload\n"
    inspected, observation = invoke(reader, binary, "inspect", "--index", str(index))
    assert inspected.returncode == 0
    assert observation["is_current"] is True
    removed, value = invoke(reader, binary, "remove", "--index", str(index))
    assert_success(removed, value)
    assert destination.exists() is False


@pytest.mark.parametrize("implementation", ["python", "rust"])
def test_both_drivers_accept_boolean_flags_in_any_order(
    tmp_path: Path, implementation: str
) -> None:
    binary = rust_example()
    result, value = invoke(
        implementation,
        binary,
        "write",
        "--replace-modified",
        "--index",
        str(tmp_path / "index"),
        "--destination",
        str(tmp_path / "destination"),
    )
    assert_success(result, value)
